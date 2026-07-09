//! `argonaut-store` CLI (Implementation D §D.10). D1 ships the `encode`
//! subcommand; `deal create` / `fetch` / `repair` land at D3–D5 (they need iroh).

use clap::{Parser, Subcommand};
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
}

fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s
}

fn main() -> anyhow::Result<()> {
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
    }
}
