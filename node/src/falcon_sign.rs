//! `falcon-sign` CLI subcommand — offline bootstrap-mode transaction signing (architecture
//! guide §2.4's "argonaut-sign" tool).
//!
//! Deviation from the doc, noted per its own guidance (§8: "adapt the internals, noting
//! the deviation"): implemented as a node subcommand rather than a separate
//! `argonaut-sign` binary/workspace-member. Same functionality, less scaffolding; the
//! keystore format is shared with `falcon-key generate` (`keygen.rs`).
//!
//! Bootstrap-mode only: outputs a SCALE-encoded `ArgonautSignature::FalconWithKey`.
//! Registry-mode signing needs a full `General`-preamble extrinsic assembled against a
//! specific runtime spec/genesis (nonce, era, tip, ...), which is a transaction-builder
//! task, not a "sign this payload" one -- see `node/examples/submit_transfer_registry.rs`.
//!
//! Keystore decryption and signing live in `argonaut-signer-core`
//! (`wallet/signer-core`), shared byte-for-byte with the browser signer
//! (`wallet/signer-wasm`); this module is the clap shell + file I/O.

use argonaut_signer_core::{keystore::FalconKeystoreFile, sign_bootstrap};
use clap::Parser;
use rand_core::OsRng;
use std::path::PathBuf;

#[derive(Debug, Parser)]
pub struct FalconSignCmd {
    /// Path to a `.falcon.json` keystore file (see `falcon-key generate`).
    pub keystore_path: PathBuf,

    /// Passphrase to decrypt the keystore, if it was written with one.
    #[arg(long)]
    pub passphrase: Option<String>,

    /// Hex-encoded payload to sign -- typically the SCALE encoding of
    /// `(call, extra, additional_signed_data)`, matching what `Checkable::check`
    /// hashes for a `Signed`-preamble extrinsic.
    #[arg(long)]
    pub payload_hex: String,
}

impl FalconSignCmd {
    pub fn run(&self) -> Result<(), String> {
        let json = std::fs::read_to_string(&self.keystore_path).map_err(|e| e.to_string())?;
        let file: FalconKeystoreFile = serde_json::from_str(&json).map_err(|e| e.to_string())?;
        let sign_key = file.open(self.passphrase.as_deref())?;

        let payload = hex::decode(self.payload_hex.trim_start_matches("0x"))
            .map_err(|e| format!("invalid --payload-hex: {e}"))?;

        let envelope = sign_bootstrap(&sign_key, &file.public_key, &payload, &mut OsRng)?;
        println!("0x{}", hex::encode(envelope));
        Ok(())
    }
}
