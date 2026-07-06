//! `falcon-key` CLI subcommand (architecture guide §2.4).
//!
//! Generates a Falcon-512 keypair, derives its AccountId (`blake3(pk)`), and writes a
//! keystore file `{address}.falcon.json`. Substrate's stock `sc-cli key generate` is
//! hardcoded to ed25519/sr25519/ecdsa (`CryptoScheme` is a closed enum in
//! `sc-cli`) and cannot be extended, so this is a bespoke subcommand rather than a
//! plugin into the stock one.
//!
//! If `--passphrase` is given, the signing key is encrypted at rest (ChaCha20-Poly1305,
//! key derived from the passphrase via `blake3::derive_key`) before being written.
//! Without one, the file is written in the clear with a loud warning — convenient for
//! throwaway devnet accounts, unsafe for anything real.

use argonaut_primitives_falcon::blake3_hash;
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Nonce,
};
use clap::Parser;
use fn_dsa::{
    sign_key_size, vrfy_key_size, KeyPairGenerator, KeyPairGeneratorStandard, FN_DSA_LOGN_512,
};
use rand_core::{OsRng, RngCore, SeedableRng};
use serde::{Deserialize, Serialize};
use sp_core::crypto::{AccountId32, Ss58Codec};
use std::path::PathBuf;

const KDF_CONTEXT: &str = "argonaut-falcon-keystore-v1";

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

#[derive(Clone, Serialize, Deserialize)]
pub struct FalconKeystoreFile {
    /// SS58-encoded AccountId (`blake3(public_key)`).
    pub address: String,
    #[serde(with = "hex::serde")]
    pub public_key: Vec<u8>,
    /// `true` if `secret_key` is ChaCha20-Poly1305-encrypted (see `kdf_salt`/`nonce`),
    /// `false` if it's the raw signing key.
    pub encrypted: bool,
    #[serde(with = "hex::serde")]
    pub secret_key: Vec<u8>,
    #[serde(with = "hex::serde")]
    pub kdf_salt: Vec<u8>,
    #[serde(with = "hex::serde")]
    pub nonce: Vec<u8>,
}

impl FalconKeyGenerateCmd {
    pub fn run(&self) -> Result<(), String> {
        let mut seed = [0u8; 32];
        OsRng.fill_bytes(&mut seed);
        let mut rng = rand_chacha::ChaCha20Rng::from_seed(seed);

        let mut kg = KeyPairGeneratorStandard::default();
        let mut sign_key = vec![0u8; sign_key_size(FN_DSA_LOGN_512)];
        let mut vrfy_key = vec![0u8; vrfy_key_size(FN_DSA_LOGN_512)];
        kg.keygen(FN_DSA_LOGN_512, &mut rng, &mut sign_key, &mut vrfy_key);

        let account = AccountId32::from(blake3_hash(&vrfy_key));
        let address = account.to_ss58check();

        let (secret_key, kdf_salt, nonce, encrypted) = match &self.passphrase {
            Some(passphrase) => {
                let mut salt = [0u8; 16];
                OsRng.fill_bytes(&mut salt);
                let mut nonce_bytes = [0u8; 12];
                OsRng.fill_bytes(&mut nonce_bytes);

                let mut key_material = salt.to_vec();
                key_material.extend_from_slice(passphrase.as_bytes());
                let derived_key = blake3::derive_key(KDF_CONTEXT, &key_material);

                let cipher = ChaCha20Poly1305::new((&derived_key).into());
                let ciphertext = cipher
                    .encrypt(Nonce::from_slice(&nonce_bytes), sign_key.as_ref())
                    .map_err(|_| "failed to encrypt signing key".to_string())?;

                (ciphertext, salt.to_vec(), nonce_bytes.to_vec(), true)
            },
            None => {
                eprintln!(
                    "warning: no --passphrase given; writing the signing key UNENCRYPTED. \
                     Only use this for throwaway devnet accounts."
                );
                (sign_key, Vec::new(), Vec::new(), false)
            },
        };

        let file = FalconKeystoreFile {
            address: address.clone(),
            public_key: vrfy_key,
            encrypted,
            secret_key,
            kdf_salt,
            nonce,
        };

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
