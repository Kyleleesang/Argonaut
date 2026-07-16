//! `falcon-key` CLI subcommand (architecture guide §2.4).
//!
//! Generates a Falcon-512 keypair, derives its AccountId (`blake3(pk)`), and writes a
//! keystore file `{address}.falcon.json`. Substrate's stock `sc-cli key generate` is
//! hardcoded to ed25519/sr25519/ecdsa (`CryptoScheme` is a closed enum in
//! `sc-cli`) and cannot be extended, so this is a bespoke subcommand rather than a
//! plugin into the stock one.
//!
//! Keygen, keystore sealing (ChaCha20-Poly1305 under a `blake3::derive_key`-derived
//! key when `--passphrase` is given), and address derivation live in
//! `argonaut-signer-core` (`wallet/signer-core`), shared byte-for-byte with the
//! browser signer (`wallet/signer-wasm`); this module is the clap shell + file I/O.

use argonaut_signer_core::{generate_keypair, keystore};
use clap::Parser;
use rand_core::{OsRng, RngCore, SeedableRng};
use std::path::PathBuf;

pub use argonaut_signer_core::keystore::FalconKeystoreFile;

#[derive(Debug, clap::Subcommand)]
pub enum FalconKeySubcommand {
    /// Generate a new Falcon-512 keypair and write a keystore file.
    Generate(FalconKeyGenerateCmd),
    /// Print the address encoded in an existing keystore file, without decrypting it.
    Inspect(FalconKeyInspectCmd),
}

impl FalconKeySubcommand {
    pub fn run(&self) -> Result<(), String> {
        match self {
            Self::Generate(cmd) => cmd.run(),
            Self::Inspect(cmd) => cmd.run(),
        }
    }
}

#[derive(Debug, Parser)]
pub struct FalconKeyGenerateCmd {
    /// Directory to write the keystore file into.
    #[arg(long, default_value = ".")]
    pub output_dir: PathBuf,

    /// Encrypt the signing key with this passphrase. Without it, the keystore file is
    /// written in the clear (fine for throwaway devnet accounts only).
    #[arg(long)]
    pub passphrase: Option<String>,
}

#[derive(Debug, Parser)]
pub struct FalconKeyInspectCmd {
    /// Path to a `.falcon.json` keystore file.
    pub keystore_path: PathBuf,
}

impl FalconKeyGenerateCmd {
    pub fn run(&self) -> Result<(), String> {
        let mut seed = [0u8; 32];
        OsRng.fill_bytes(&mut seed);
        let mut rng = rand_chacha::ChaCha20Rng::from_seed(seed);
        let keypair = generate_keypair(&mut rng);

        if self.passphrase.is_none() {
            eprintln!(
                "warning: no --passphrase given; writing the signing key UNENCRYPTED. \
                 Only use this for throwaway devnet accounts."
            );
        }
        let file = keystore::seal(&keypair, self.passphrase.as_deref(), &mut OsRng)?;
        let address = file.address.clone();

        std::fs::create_dir_all(&self.output_dir).map_err(|e| e.to_string())?;
        let path = self.output_dir.join(format!("{address}.falcon.json"));
        let json = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
        std::fs::write(&path, json).map_err(|e| e.to_string())?;

        println!("Generated Falcon-512 keypair");
        println!("  Address:  {address}");
        println!("  Keystore: {}", path.display());
        Ok(())
    }
}

impl FalconKeyInspectCmd {
    pub fn run(&self) -> Result<(), String> {
        let json = std::fs::read_to_string(&self.keystore_path).map_err(|e| e.to_string())?;
        let file: FalconKeystoreFile = serde_json::from_str(&json).map_err(|e| e.to_string())?;
        println!("Address:    {}", file.address);
        println!("Public key: {}", hex::encode(&file.public_key));
        println!("Encrypted:  {}", file.encrypted);
        Ok(())
    }
}
