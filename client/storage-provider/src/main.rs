//! D.9 `argonaut-provider`: the storage provider daemon binary. Registers
//! (if not already), then runs the epoch-driven challenge-response loop
//! (`responder.rs`) forever.
//!
//! D3 scope (see `blobs.rs`'s module doc): held assignments are configured
//! via `--assignment deal_id:shard_idx` flags plus local shard files loaded
//! into the blob store at startup, not auto-discovered by watching
//! `DealCreated` events and deciding whether to bid -- that negotiation flow
//! is the client-side wizard's job (frontend spec §7.2) and D4 territory,
//! not D3's "answer known challenges reliably" acceptance bar (D.12).

use argonaut_storage_provider::blobs::{BlobStore, DiskBlobStore};
use argonaut_storage_provider::chain::ChainHandle;
use argonaut_storage_provider::responder::{HeldAssignment, Responder};
use argonaut_chain_client::extrinsic::SignMode;
use argonaut_primitives_storage::{DealId, ShardIndex};
use argonaut_runtime::AccountId;
use argonaut_signer_core::keystore::FalconKeystoreFile;
use clap::Parser;
use rand_core::OsRng;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "argonaut-provider", about = "Argonaut storage provider daemon (Implementation D §D.9)")]
struct Args {
    /// Path to a `.falcon.json` keystore file (see `argonaut-node falcon-key generate`).
    #[arg(long)]
    keystore: PathBuf,
    #[arg(long)]
    passphrase: Option<String>,
    /// Node RPC WebSocket URL.
    #[arg(long, default_value = "ws://127.0.0.1:9944")]
    rpc_url: String,
    /// Directory to store shard bytes in (see `blobs::DiskBlobStore`).
    #[arg(long)]
    data_dir: PathBuf,
    /// `deal_id:shard_idx` pairs this daemon is responsible for. For each,
    /// a file named `<data-dir>/shards/<deal_id>-<shard_idx>.bin` must exist
    /// (the client-side deal-creation flow, D4, is what populates these in
    /// production; for now, copy the shard bytes there yourself).
    #[arg(long = "assignment", value_parser = parse_assignment)]
    assignments: Vec<(DealId, ShardIndex)>,
    /// Registration stake, in plancks. Only used if not already registered.
    #[arg(long, default_value_t = 1_000_000_000_000_000)]
    stake: u128,
    /// Advertised price per shard-epoch, in plancks.
    #[arg(long, default_value_t = 1_000_000_000)]
    price_per_shard_epoch: u128,
    /// This node's iroh NodeId, hex-encoded (32 bytes). Placeholder-only in
    /// D3 (no real iroh serving yet, see `blobs.rs`); pass 32 zero bytes if
    /// you have no real iroh identity yet.
    #[arg(long, default_value = "0000000000000000000000000000000000000000000000000000000000000000")]
    iroh_node_id: String,
    #[arg(long, default_value = "")]
    relay_hint: String,
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();
    let args = Args::parse();

    let json = std::fs::read_to_string(&args.keystore)?;
    let keystore: FalconKeystoreFile = serde_json::from_str(&json)?;
    let signing_key = keystore
        .open(args.passphrase.as_deref())
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let account = AccountId::from(argonaut_signer_core::account_id(&keystore.public_key));

    log::info!("provider account: {}", sp_core::crypto::Ss58Codec::to_ss58check(&account));

    let chain = ChainHandle::connect(&args.rpc_url).await?;

    if chain.provider_info(&account).await?.is_none() {
        log::info!("not yet registered, registering (bootstrap mode)");
        let pk = argonaut_primitives_falcon::FalconPublic(
            sp_core::bounded::BoundedVec::try_from(keystore.public_key.clone())
                .map_err(|_| anyhow::anyhow!("public key is the wrong length"))?,
        );
        let node_id_bytes = hex::decode(&args.iroh_node_id)?;
        let mut iroh_node_id = [0u8; 32];
        if node_id_bytes.len() == 32 {
            iroh_node_id.copy_from_slice(&node_id_bytes);
        }
        let mut rng = OsRng;
        chain
            .register_provider(
                args.stake,
                iroh_node_id,
                args.relay_hint.into_bytes(),
                args.price_per_shard_epoch,
                SignMode::Bootstrap { pk },
                &signing_key,
                &account,
                &mut rng,
            )
            .await?;
        log::info!("registration submitted");
    } else {
        log::info!("already registered");
    }

    let blobs = DiskBlobStore::new(args.data_dir.join("shards"))?;
    for (deal_id, shard_idx) in &args.assignments {
        let path = args.data_dir.join("shards").join(format!("{deal_id}-{shard_idx}.bin"));
        if path.exists() {
            let (root, size) = blobs.add_path(&path).await?;
            log::info!("loaded held shard deal={deal_id} shard={shard_idx} root={} size={size}", hex::encode(root));
        } else {
            log::warn!(
                "no local file for deal={deal_id} shard={shard_idx} at {} -- this daemon cannot activate or prove it until the file exists",
                path.display()
            );
        }
    }

    let held: Vec<HeldAssignment> = args
        .assignments
        .into_iter()
        .map(|(deal_id, shard_idx)| HeldAssignment { deal_id, shard_idx })
        .collect();

    let responder = Responder {
        chain,
        blobs: Arc::new(blobs),
        held,
        signer_account: account,
        signing_key,
        epoch_duration_blocks: args.epoch_duration_blocks,
        poll_interval: Duration::from_secs(args.poll_interval_secs),
    };

    log::info!("responder loop starting (poll every {}s)", args.poll_interval_secs);
    responder.run_forever().await
}
