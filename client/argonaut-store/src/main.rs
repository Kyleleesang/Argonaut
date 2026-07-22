//! `argonaut-store` CLI (Implementation D §D.10). `encode` (D1) is pure
//! local computation; `deal-create`/`fetch` (D4) talk to a live chain + iroh
//! providers. `repair` (D5) isn't implemented yet.

use argonaut_chain_client::extrinsic::SignMode;
use argonaut_chain_client::ChainClient;
use argonaut_primitives_storage::{DealId, Manifest};
use argonaut_runtime::AccountId;
use argonaut_signer_core::keystore::FalconKeystoreFile;
use clap::{Parser, Subcommand};
use parity_scale_codec::Decode;
use rand_core::OsRng;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "argonaut-store", about = "Argonaut storage client (Implementation D)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Erasure-encode a file into k-of-n shards + a manifest (D.3.4).
    Encode {
        /// Input file to encode.
        file: PathBuf,
        /// RS data shards (1 <= k < n).
        #[arg(long, default_value_t = 10)]
        k: u8,
        /// Total shards (<= 64).
        #[arg(long, default_value_t = 16)]
        n: u8,
        /// Output directory for shard blobs + manifest (default: input's directory).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Create a deal from a directory already produced by `encode`, then
    /// push each shard directly to a chosen provider (D4).
    DealCreate {
        /// Directory containing `manifest.scale` + `shard_NN.bin` files
        /// (an `encode` output directory).
        dir: PathBuf,
        #[arg(long)]
        keystore: PathBuf,
        #[arg(long)]
        passphrase: Option<String>,
        #[arg(long, default_value = "ws://127.0.0.1:9944")]
        rpc_url: String,
        #[arg(long, default_value_t = 1000)]
        duration_epochs: u32,
        #[arg(long, default_value_t = 1_000_000_000)]
        price_per_shard_epoch: u128,
        #[arg(long, default_value_t = 50_000)]
        challenge_rate_ppm: u32,
    },
    /// Fetch a deal's shards from their assigned providers and reassemble
    /// the original file (D4). Requires the local `manifest.scale` from the
    /// original `encode` (manifest distribution over the network isn't
    /// implemented yet -- see `fetch.rs`'s module doc).
    Fetch {
        #[arg(long)]
        deal_id: DealId,
        /// Local manifest.scale from the original `encode` call.
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        keystore: PathBuf,
        #[arg(long)]
        passphrase: Option<String>,
        #[arg(long, default_value = "ws://127.0.0.1:9944")]
        rpc_url: String,
        /// Must match the target providers' own `--price-per-stride`
        /// (out-of-band agreed rate; see `fetch.rs`'s module doc).
        #[arg(long, default_value_t = 100_000_000)]
        price_per_stride: u128,
    },
}

fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s
}

fn load_signer(
    keystore: &PathBuf,
    passphrase: &Option<String>,
) -> anyhow::Result<(Vec<u8>, Vec<u8>, AccountId)> {
    let json = std::fs::read_to_string(keystore)?;
    let file: FalconKeystoreFile = serde_json::from_str(&json)?;
    let signing_key = file.open(passphrase.as_deref()).map_err(|e| anyhow::anyhow!("{e}"))?;
    let account = AccountId::from(argonaut_signer_core::account_id(&file.public_key));
    Ok((signing_key, file.public_key, account))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();
    match Cli::parse().cmd {
        Cmd::Encode { file, k, n, out } => {
            let bytes = std::fs::read(&file)
                .map_err(|e| anyhow::anyhow!("reading {}: {e}", file.display()))?;
            let enc = argonaut_store::encode(&bytes, k, n)?;

            let dir = out.unwrap_or_else(|| {
                file.parent().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
            });
            std::fs::create_dir_all(&dir)?;
            for (i, shard) in enc.shards.iter().enumerate() {
                std::fs::write(dir.join(format!("shard_{i:02}.bin")), shard)?;
            }
            std::fs::write(dir.join("manifest.scale"), &enc.manifest_bytes)?;

            println!("manifest_root : {}", hex(&enc.manifest_root));
            println!(
                "file_size={}  k={}  n={}  shard_size={}",
                enc.manifest.file_size, k, n, enc.manifest.shard_size
            );
            for (i, r) in enc.manifest.shards.iter().enumerate() {
                println!("  shard {i:02} root : {}", hex(r));
            }
            println!("wrote {} shard(s) + manifest.scale to {}", n, dir.display());
            Ok(())
        }

        Cmd::DealCreate { dir, keystore, passphrase, rpc_url, duration_epochs, price_per_shard_epoch, challenge_rate_ppm } => {
            let manifest_bytes = std::fs::read(dir.join("manifest.scale"))?;
            let manifest = Manifest::decode(&mut &manifest_bytes[..])?;
            let mut shard_bytes = Vec::with_capacity(manifest.n as usize);
            for i in 0..manifest.n {
                shard_bytes.push(std::fs::read(dir.join(format!("shard_{i:02}.bin")))?);
            }

            let (signing_key, pk_bytes, account) = load_signer(&keystore, &passphrase)?;
            let client = ChainClient::connect(&rpc_url).await?;
            // Registry mode if this account already has a key registered
            // (cheaper); bootstrap mode (and implicit registration) otherwise.
            let registered = client
                .read_storage::<()>(&argonaut_chain_client::keys::map_key(
                    "FalconRegistry",
                    "Keys",
                    argonaut_chain_client::keys::Hasher::Blake2_128Concat,
                    &account,
                ))
                .await
                .unwrap_or(None)
                .is_some();
            let mode = if registered {
                SignMode::Registry { account: account.clone() }
            } else {
                let pk = argonaut_primitives_falcon::FalconPublic(
                    sp_core::bounded::BoundedVec::try_from(pk_bytes)
                        .map_err(|_| anyhow::anyhow!("public key is the wrong length"))?,
                );
                SignMode::Bootstrap { pk }
            };

            let mut rng = OsRng;
            let deal_id = argonaut_store::deal::create_deal(
                &client,
                &manifest,
                &manifest_bytes,
                duration_epochs,
                price_per_shard_epoch,
                challenge_rate_ppm,
                mode,
                &signing_key,
                &account,
                &mut rng,
            )
            .await?;
            println!("deal_id (best-effort, race-prone -- see deal.rs): {deal_id}, mined.");
            println!("Waiting for a provider to accept each shard slot, e.g. (run once per shard, any active provider may claim any open slot):");
            for i in 0..manifest.n {
                println!(
                    "  argonaut-provider accept-shard --keystore <provider-keystore> --deal-id {deal_id} --shard-idx {i} --rpc-url {rpc_url}"
                );
            }

            argonaut_store::deal::push_shards_for_deal(&client, deal_id, &manifest, &shard_bytes)
                .await?;
            println!("all {} shard(s) accepted + pushed for deal {deal_id}", manifest.n);
            Ok(())
        }

        Cmd::Fetch { deal_id, manifest, out, keystore, passphrase, rpc_url, price_per_stride } => {
            let manifest_bytes = std::fs::read(&manifest)?;
            let manifest = Manifest::decode(&mut &manifest_bytes[..])?;
            let (signing_key, _pk_bytes, account) = load_signer(&keystore, &passphrase)?;
            let client = ChainClient::connect(&rpc_url).await?;

            let mut available: Vec<Option<Vec<u8>>> = vec![None; manifest.n as usize];
            let mut have = 0u8;
            for (idx, root) in manifest.shards.iter().enumerate() {
                if have >= manifest.k {
                    break;
                }
                let mode = SignMode::Registry { account: account.clone() };
                let mut rng = OsRng;
                match argonaut_store::fetch::fetch_shard(
                    &client,
                    deal_id,
                    idx as u16,
                    root,
                    manifest.shard_size,
                    price_per_stride,
                    mode,
                    &signing_key,
                    &account,
                    &mut rng,
                )
                .await
                {
                    Ok(bytes) => {
                        println!("fetched shard {idx} ({} bytes)", bytes.len());
                        available[idx] = Some(bytes);
                        have += 1;
                    }
                    Err(e) => {
                        println!("shard {idx} unavailable: {e:#} (continuing, need only k={})", manifest.k);
                    }
                }
            }
            anyhow::ensure!(have >= manifest.k, "only retrieved {have}/{} needed shards", manifest.k);

            let file_bytes = argonaut_store::decode(&manifest, &available)?;
            std::fs::write(&out, &file_bytes)?;
            println!("wrote {} bytes to {}", file_bytes.len(), out.display());
            Ok(())
        }
    }
}
