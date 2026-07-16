//! `@argonaut/signer` scaffold — Falcon-512 signing in the browser
//! (docs/FRONTEND_DAPP_SPEC.md §4, milestones F0/F1).
//!
//! wasm-bindgen shell over `argonaut-signer-core`: the same Rust that backs
//! `argonaut-node falcon-key` / `falcon-sign`, compiled to wasm32 — one
//! signing implementation, two frontends (spec binding D2: JS never signs).
//!
//! Scaffold scope: a stateless string API at CLI parity (keystore generate /
//! inspect / bootstrap-mode payload signing). Still to come (F2+): the
//! extrinsic builder (nonce/era/tip/genesis assembly and registry mode — see
//! `node/examples/submit_transfer_registry.rs` for the reference payload
//! construction) and in-memory key handles + approval UI (custody ladder §9.1).

use argonaut_signer_core::{
    account_id, generate_keypair, keystore, sign_bootstrap, ss58, FALCON512_PK_LEN,
};
use rand_core::OsRng;
use wasm_bindgen::prelude::*;

fn err(e: String) -> JsError {
    JsError::new(&e)
}

/// Generate a Falcon-512 keypair and return the keystore file as JSON — the
/// same `.falcon.json` format `falcon-key generate` writes, so keys move
/// freely between browser and CLI.
#[wasm_bindgen]
pub fn generate_keystore(passphrase: Option<String>) -> Result<String, JsError> {
    let keypair = generate_keypair(&mut OsRng);
    let file = keystore::seal(&keypair, passphrase.as_deref(), &mut OsRng).map_err(err)?;
    serde_json::to_string_pretty(&file).map_err(|e| err(e.to_string()))
}

/// The SS58 address stored in a keystore JSON (no decryption needed).
#[wasm_bindgen]
pub fn keystore_address(keystore_json: &str) -> Result<String, JsError> {
    let file: keystore::FalconKeystoreFile =
        serde_json::from_str(keystore_json).map_err(|e| err(e.to_string()))?;
    Ok(file.address)
}

/// Derive the SS58 address for a hex-encoded Falcon-512 public key.
#[wasm_bindgen]
pub fn derive_address(public_key_hex: &str) -> Result<String, JsError> {
    let pk = hex::decode(public_key_hex.trim_start_matches("0x"))
        .map_err(|e| err(format!("invalid public key hex: {e}")))?;
    if pk.len() != FALCON512_PK_LEN {
        return Err(err(format!(
            "public key must be {FALCON512_PK_LEN} bytes, got {}",
            pk.len()
        )));
    }
    Ok(ss58::ss58_address(&account_id(&pk)))
}

/// Sign a hex payload in bootstrap mode; returns the 0x-hex SCALE-encoded
/// `ArgonautSignature::FalconWithKey` — byte-identical to what
/// `argonaut-node falcon-sign` prints for the same inputs.
#[wasm_bindgen]
pub fn sign_payload(
    keystore_json: &str,
    passphrase: Option<String>,
    payload_hex: &str,
) -> Result<String, JsError> {
    let file: keystore::FalconKeystoreFile =
        serde_json::from_str(keystore_json).map_err(|e| err(e.to_string()))?;
    let sign_key = file.open(passphrase.as_deref()).map_err(err)?;
    let payload = hex::decode(payload_hex.trim_start_matches("0x"))
        .map_err(|e| err(format!("invalid payload hex: {e}")))?;
    let envelope =
        sign_bootstrap(&sign_key, &file.public_key, &payload, &mut OsRng).map_err(err)?;
    Ok(format!("0x{}", hex::encode(envelope)))
}
