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
//! task, not a "sign this payload" one -- left as follow-up tooling.

use crate::keygen::FalconKeystoreFile;
use argonaut_primitives_falcon::{blake3_hash, ArgonautSignature, FalconPublic, FalconSignature};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Nonce,
};
use clap::Parser;
use fn_dsa::{signature_size, SigningKey, SigningKeyStandard, DOMAIN_NONE, HASH_ID_RAW};
use parity_scale_codec::Encode;
use rand_core::OsRng;
use sp_core::bounded::BoundedVec;
use std::path::PathBuf;

const KDF_CONTEXT: &str = "argonaut-falcon-keystore-v1";

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

        let sign_key = if file.encrypted {
            let passphrase = self
                .passphrase
                .as_ref()
                .ok_or("keystore is encrypted; --passphrase is required")?;
            let mut key_material = file.kdf_salt.clone();
            key_material.extend_from_slice(passphrase.as_bytes());
            let derived_key = blake3::derive_key(KDF_CONTEXT, &key_material);
            let cipher = ChaCha20Poly1305::new((&derived_key).into());
            cipher
                .decrypt(Nonce::from_slice(&file.nonce), file.secret_key.as_ref())
                .map_err(|_| "failed to decrypt signing key: wrong passphrase?".to_string())?
        } else {
            file.secret_key.clone()
        };

        let payload = hex::decode(self.payload_hex.trim_start_matches("0x"))
            .map_err(|e| format!("invalid --payload-hex: {e}"))?;
        let msg_hash = blake3_hash(&payload);

        let mut sk = SigningKeyStandard::decode(&sign_key)
            .ok_or("keystore signing key is corrupt or wrong length")?;
        let mut sig_bytes = vec![0u8; signature_size(sk.get_logn())];
        sk.sign(&mut OsRng, &DOMAIN_NONE, &HASH_ID_RAW, &msg_hash, &mut sig_bytes);

        let signature = ArgonautSignature::FalconWithKey {
            sig: FalconSignature(
                BoundedVec::try_from(sig_bytes).map_err(|_| "signature too long")?,
            ),
            pk: FalconPublic(
                BoundedVec::try_from(file.public_key.clone())
                    .map_err(|_| "public key too long")?,
            ),
        };

        println!("0x{}", hex::encode(signature.encode()));
        Ok(())
    }
}
