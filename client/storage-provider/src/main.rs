//! D.9 `argonaut-provider`: the storage provider daemon binary. Registers
//! (if not already), starts serving the D4 shard-push/retrieval protocols
//! over a real iroh endpoint, then runs the epoch-driven challenge-response
//! loop (`responder.rs`) forever.
//!
//! D3/D4 scope (see `blobs.rs`'s module doc): held assignments are
//! configured via `--assignment deal_id:shard_idx` flags, not
//! auto-discovered by watching `DealCreated` events and deciding whether to
//! bid -- that negotiation flow is the client-side wizard's job (frontend
//! spec §7.2), not this daemon's job. Once accepted, shard bytes arrive via
//! the shard-push protocol (client -> provider) rather than a local file --
//! see `--assignment`'s doc below for the one exception (pre-seeding).

use argonaut_chain_client::extrinsic::SignMode;
use argonaut_primitives_storage::{BlobRoot, DealId, ShardIndex};
use argonaut_runtime::AccountId;
use argonaut_signer_core::keystore::FalconKeystoreFile;
use argonaut_storage_provider::blobs::{BlobStore, DiskBlobStore};
use argonaut_storage_provider::chain::ChainHandle;
use argonaut_storage_provider::protocol::{
    RetrievalHandler, ShardAuthority, ShardPushHandler, RETRIEVAL_ALPN, SHARD_PUSH_ALPN,
};
use argonaut_storage_provider::responder::{HeldAssignment, Responder};
use clap::{Parser, Subcommand};
use iroh::endpoint::presets;
use iroh::protocol::Router;
use iroh::{Endpoint, RelayMode, SecretKey};
use rand_core::OsRng;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "argonaut-provider", about = "Argonaut storage provider daemon (Implementation D §D.9/D4)")]
struct Cli {
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the long-lived daemon: register if needed, serve shard-push +
    /// retrieval, answer challenges forever.
    Run(RunArgs),
    /// One-shot: submit `accept_shard(deal_id, shard_idx)` and exit. D3's
    /// daemon deliberately doesn't auto-bid on deals (see the module doc),
    /// so a provider operator (a human, or a future bidding policy this
    /// isn't) runs this once per shard it decides to hold, before the
    /// client can push bytes to it (the shard-push protocol's
    /// `ShardAuthority` check requires an `Accepted`-state assignment to
    /// already exist on-chain).
    AcceptShard {
        #[arg(long)]
        keystore: PathBuf,
        #[arg(long)]
        passphrase: Option<String>,
        #[arg(long, default_value = "ws://127.0.0.1:9944")]
        rpc_url: String,
        #[arg(long)]
        deal_id: DealId,
        #[arg(long)]
        shard_idx: ShardIndex,
    },
}

#[derive(Parser)]
struct RunArgs {
    /// Path to a `.falcon.json` keystore file (see `argonaut-node falcon-key generate`).
    #[arg(long)]
    keystore: PathBuf,
    #[arg(long)]
    passphrase: Option<String>,
    /// Node RPC WebSocket URL.
    #[arg(long, default_value = "ws://127.0.0.1:9944")]
    rpc_url: String,
    /// Host to advertise in the on-chain `relay_hint`, combined with the
    /// iroh endpoint's actual bound port. iroh binds to the wildcard
    /// address by default, which isn't itself connectable -- something
    /// routable has to be advertised instead. Defaults to loopback for a
    /// same-machine devnet; a real multi-host deployment needs this set to
    /// this machine's actual reachable address.
    #[arg(long, default_value = "127.0.0.1")]
    advertise_host: String,
    /// Directory to store shard bytes and this daemon's persistent iroh
    /// identity in (see `blobs::DiskBlobStore`).
    #[arg(long)]
    data_dir: PathBuf,
    /// `deal_id:shard_idx` pairs this daemon is responsible for. Normally
    /// nothing needs pre-loading: once `accept_shard` lands, the client
    /// pushes the actual bytes over `/argonaut/shard-push/1` before or
    /// shortly after. If `<data-dir>/shards/<deal_id>-<shard_idx>.bin`
    /// happens to already exist (e.g. this provider is also the encoder, or
    /// a devnet test pre-seeded it), it's loaded at startup instead of
    /// waiting for the push.
    #[arg(long = "assignment", value_parser = parse_assignment)]
    assignments: Vec<(DealId, ShardIndex)>,
    /// Registration stake, in plancks. Only used if not already registered.
    #[arg(long, default_value_t = 1_000_000_000_000_000)]
    stake: u128,
    /// Advertised price per shard-epoch, in plancks.
    #[arg(long, default_value_t = 1_000_000_000)]
    price_per_shard_epoch: u128,
    /// Price per retrieval stride (`VOUCHER_STRIDE` bytes), in plancks --
    /// an off-chain-agreed rate (spec §D.9 gate.rs; storage deals price
    /// per-shard-epoch on-chain, but nothing in D prices retrieval
    /// bandwidth on-chain, so this is this provider's own advertised term,
    /// out of band, same category as `relay_hint`).
    #[arg(long, default_value_t = 100_000_000)]
    price_per_stride: u128,
    /// Compiled-in default is 100 (`pallet_storage_proofs::parameters::EPOCH_DURATION_BLOCKS`);
    /// override to match whatever devnet chain-spec you're pointed at.
    #[arg(long, default_value_t = 100)]
    epoch_duration_blocks: u64,
    #[arg(long, default_value_t = 5)]
    poll_interval_secs: u64,
}

fn parse_assignment(s: &str) -> Result<(DealId, ShardIndex), String> {
    let (deal, shard) = s.split_once(':').ok_or_else(|| "expected deal_id:shard_idx".to_string())?;
    Ok((
        deal.parse().map_err(|e| format!("bad deal_id: {e}"))?,
        shard.parse().map_err(|e| format!("bad shard_idx: {e}"))?,
    ))
}

/// Load this daemon's iroh identity from `<data_dir>/iroh_secret.key`,
/// generating and persisting a fresh one on first run. A stable identity
/// across restarts matters here the same way it does for the node's own
/// libp2p key (B-10): re-registering under a new `iroh_node_id` every
/// restart would strand any in-flight deals pointed at the old one.
fn load_or_generate_iroh_secret(data_dir: &std::path::Path) -> anyhow::Result<SecretKey> {
    let path = data_dir.join("iroh_secret.key");
    if let Ok(bytes) = std::fs::read(&path) {
        let arr: [u8; 32] =
            bytes.try_into().map_err(|_| anyhow::anyhow!("{} is not 32 bytes", path.display()))?;
        return Ok(SecretKey::from_bytes(&arr));
    }
    let key = SecretKey::generate();
    std::fs::create_dir_all(data_dir)?;
    std::fs::write(&path, key.to_bytes())?;
    Ok(key)
}

/// Checks a shard push/serve against real on-chain state: is there an
/// assignment for `(deal_id, shard_idx)` naming this account as provider,
/// and if so, what root should it match? Backed by `ChainHandle` directly
/// (a live RPC round trip per check) -- correctness over caching for a
/// first pass; a busy provider might want to cache deal params locally
/// later.
struct ChainBackedAuthority {
    chain: ChainHandle,
    me: AccountId,
}

#[async_trait::async_trait]
impl ShardAuthority for ChainBackedAuthority {
    async fn expected_root(&self, deal_id: DealId, shard_idx: ShardIndex) -> Option<BlobRoot> {
        let assignment = self.chain.assignment(deal_id, shard_idx).await.ok()??;
        if assignment.provider != self.me {
            return None;
        }
        let deal = self.chain.deal(deal_id).await.ok()??;
        deal.params.shards.get(shard_idx as usize).map(|s| s.root)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();
    match Cli::parse().cmd {
        Command::Run(args) => run(args).await,
        Command::AcceptShard { keystore, passphrase, rpc_url, deal_id, shard_idx } => {
            accept_shard_once(keystore, passphrase, rpc_url, deal_id, shard_idx).await
        }
    }
}

async fn accept_shard_once(
    keystore: PathBuf,
    passphrase: Option<String>,
    rpc_url: String,
    deal_id: DealId,
    shard_idx: ShardIndex,
) -> anyhow::Result<()> {
    let json = std::fs::read_to_string(&keystore)?;
    let keystore: FalconKeystoreFile = serde_json::from_str(&json)?;
    let signing_key = keystore.open(passphrase.as_deref()).map_err(|e| anyhow::anyhow!("{e}"))?;
    let account = AccountId::from(argonaut_signer_core::account_id(&keystore.public_key));

    let chain = ChainHandle::connect(&rpc_url).await?;
    let mode = if chain.provider_info(&account).await?.is_some() {
        SignMode::Registry { account: account.clone() }
    } else {
        let pk = argonaut_primitives_falcon::FalconPublic(
            sp_core::bounded::BoundedVec::try_from(keystore.public_key.clone())
                .map_err(|_| anyhow::anyhow!("public key is the wrong length"))?,
        );
        SignMode::Bootstrap { pk }
    };
    let mut rng = OsRng;
    let hash = chain
        .accept_shard(deal_id, shard_idx, mode, &signing_key, &account, &mut rng)
        .await?;
    println!("accept_shard submitted: deal={deal_id} shard={shard_idx} tx={hash:?}");
    Ok(())
}

async fn run(args: RunArgs) -> anyhow::Result<()> {
    let json = std::fs::read_to_string(&args.keystore)?;
    let keystore: FalconKeystoreFile = serde_json::from_str(&json)?;
    let signing_key = keystore
        .open(args.passphrase.as_deref())
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let account = AccountId::from(argonaut_signer_core::account_id(&keystore.public_key));

    log::info!("provider account: {}", sp_core::crypto::Ss58Codec::to_ss58check(&account));

    // ── iroh endpoint: no relay, no DNS discovery -- a devnet's providers
    // and clients register each other's addresses directly (out of band,
    // via on-chain `relay_hint` -- see the client side for the other half).
    // `presets::Minimal` sets only the mandatory crypto provider.
    let iroh_secret = load_or_generate_iroh_secret(&args.data_dir)?;
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(iroh_secret)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await?;
    let my_iroh_id = endpoint.id();
    let bound: SocketAddr = *endpoint
        .bound_sockets()
        .first()
        .ok_or_else(|| anyhow::anyhow!("iroh endpoint bound no sockets"))?;
    // `bound` is the wildcard bind address (e.g. 0.0.0.0:PORT) -- not
    // itself connectable. Advertise `--advertise-host` with the real port.
    let my_addr: SocketAddr = format!("{}:{}", args.advertise_host, bound.port()).parse()?;
    log::info!("iroh endpoint {my_iroh_id} bound {bound}, advertising {my_addr}");

    let chain = ChainHandle::connect(&args.rpc_url).await?;

    if chain.provider_info(&account).await?.is_none() {
        log::info!("not yet registered, registering (bootstrap mode)");
        let pk = argonaut_primitives_falcon::FalconPublic(
            sp_core::bounded::BoundedVec::try_from(keystore.public_key.clone())
                .map_err(|_| anyhow::anyhow!("public key is the wrong length"))?,
        );
        let iroh_node_id: [u8; 32] = *std::borrow::Borrow::<[u8; 32]>::borrow(&my_iroh_id);
        let mut rng = OsRng;
        chain
            .register_provider(
                args.stake,
                iroh_node_id,
                my_addr.to_string().into_bytes(),
                args.price_per_shard_epoch,
                SignMode::Bootstrap { pk },
                &signing_key,
                &account,
                &mut rng,
            )
            .await?;
        log::info!("registration submitted (iroh_node_id={my_iroh_id}, relay_hint={my_addr})");
    } else {
        log::info!("already registered");
    }

    let blobs = Arc::new(DiskBlobStore::new(args.data_dir.join("shards"))?);
    for (deal_id, shard_idx) in &args.assignments {
        let path = args.data_dir.join("shards").join(format!("{deal_id}-{shard_idx}.bin"));
        if path.exists() {
            let (root, size) = blobs.add_path(&path).await?;
            log::info!("pre-loaded held shard deal={deal_id} shard={shard_idx} root={} size={size}", hex::encode(root));
        }
    }

    // A second, independent connection -- `ChainClient` isn't `Clone` (see
    // its own doc comment), and the authority check runs concurrently with
    // the responder loop's own chain calls.
    let authority_chain = ChainHandle::connect(&args.rpc_url).await?;
    let authority = Arc::new(ChainBackedAuthority { chain: authority_chain, me: account.clone() });
    let shard_push = ShardPushHandler::new(blobs.clone(), authority);
    let retrieval = RetrievalHandler::new(blobs.clone(), args.price_per_stride);
    let router = Router::builder(endpoint)
        .accept(SHARD_PUSH_ALPN, shard_push)
        .accept(RETRIEVAL_ALPN, retrieval)
        .spawn();
    log::info!("serving shard-push + retrieval protocols");

    let held: Vec<HeldAssignment> = args
        .assignments
        .into_iter()
        .map(|(deal_id, shard_idx)| HeldAssignment { deal_id, shard_idx })
        .collect();

    let responder = Responder {
        chain,
        blobs,
        held,
        signer_account: account,
        signing_key,
        epoch_duration_blocks: args.epoch_duration_blocks,
        poll_interval: Duration::from_secs(args.poll_interval_secs),
    };

    log::info!("responder loop starting (poll every {}s)", args.poll_interval_secs);
    let result = responder.run_forever().await;
    router.shutdown().await?;
    result
}
