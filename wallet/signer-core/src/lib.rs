//! Falcon-512 keystore + transaction-signing core, shared by the node CLI
//! (`argonaut-node falcon-key` / `falcon-sign`) and the browser signer
//! (`wallet/signer-wasm` → `@argonaut/signer`) — docs/FRONTEND_DAPP_SPEC.md §4,
//! binding D2: one Rust implementation of key handling, two frontends, zero drift.
//!
//! No Substrate dependencies: everything here must compile for
//! `wasm32-unknown-unknown`, so the SS58/account/signature-envelope encodings
//! are implemented locally and pinned byte-for-byte against `sp-core` /
//! `argonaut-primitives-falcon` in this crate's tests (which do link those,
//! natively).
//!
//! Signing convention (must match `primitives/falcon`): signatures are over
//! `blake3(payload)` (32 bytes); `AccountId = blake3(public_key)`.

pub mod keystore;
pub mod ss58;

use fn_dsa::{
    sign_key_size, signature_size, vrfy_key_size, KeyPairGenerator, KeyPairGeneratorStandard,
    SigningKey, SigningKeyStandard, DOMAIN_NONE, FN_DSA_LOGN_512, HASH_ID_RAW,
};
use parity_scale_codec::Encode;
use rand_core::{CryptoRng, RngCore};

/// Falcon-512 public key length (fn-dsa encoding), as enforced on-chain.
pub const FALCON512_PK_LEN: usize = 897;

/// A Falcon-512 keypair in fn-dsa's encodings (1281-byte signing key,
/// 897-byte verifying key).
pub struct Keypair {
    pub signing_key: Vec<u8>,
    pub verifying_key: Vec<u8>,
}

pub fn generate_keypair(rng: &mut (impl RngCore + CryptoRng)) -> Keypair {
    let mut kg = KeyPairGeneratorStandard::default();
    let mut signing_key = vec![0u8; sign_key_size(FN_DSA_LOGN_512)];
    let mut verifying_key = vec![0u8; vrfy_key_size(FN_DSA_LOGN_512)];
    kg.keygen(FN_DSA_LOGN_512, rng, &mut signing_key, &mut verifying_key);
    Keypair { signing_key, verifying_key }
}

/// `blake3(public_key)` — the chain's `AccountId32` bytes for this key
/// (`ArgonautSigner::into_account`).
pub fn account_id(verifying_key: &[u8]) -> [u8; 32] {
    *blake3::hash(verifying_key).as_bytes()
}

/// Raw Falcon-512 signature over `blake3(payload)`.
pub fn sign_payload(
    signing_key: &[u8],
    payload: &[u8],
    rng: &mut (impl RngCore + CryptoRng),
) -> Result<Vec<u8>, String> {
    sign_digest(signing_key, blake3::hash(payload).as_bytes(), rng)
}

/// Raw Falcon-512 signature over an already-hashed 32-byte digest, with no
/// further hashing. Needed wherever the on-chain check verifies a signature
/// against a digest that was computed by something other than "hash exactly
/// what `sign_payload` would hash" -- e.g. `pallet_payment_channels::settle`
/// verifies against `voucher_signing_payload(&voucher)`, which is itself
/// already `blake3(DS_VOUCHER ++ scale(voucher))`; calling `sign_payload` on
/// top of that would hash it a second time and produce a signature the
/// chain rejects. `sign_payload` is defined in terms of this function, not
/// the other way around, so there is exactly one place the Falcon `sign`
/// call itself happens.
pub fn sign_digest(
    signing_key: &[u8],
    msg_hash: &[u8; 32],
    rng: &mut (impl RngCore + CryptoRng),
) -> Result<Vec<u8>, String> {
    let mut sk = SigningKeyStandard::decode(signing_key)
        .ok_or_else(|| "keystore signing key is corrupt or wrong length".to_string())?;
    let mut sig = vec![0u8; signature_size(sk.get_logn())];
    sk.sign(rng, &DOMAIN_NONE, &HASH_ID_RAW, msg_hash, &mut sig);
    Ok(sig)
}

/// Local mirror of `ArgonautSignature`'s SCALE encoding (`primitives/falcon`).
/// `BoundedVec<u8, _>` encodes identically to `Vec<u8>`; byte-equality with the
/// real enum is pinned in this crate's tests.
#[derive(Encode)]
enum SignatureEnvelope {
    #[codec(index = 0)]
    FalconWithKey { sig: Vec<u8>, pk: Vec<u8> },
    #[codec(index = 1)]
    Falcon { sig: Vec<u8> },
}

/// SCALE-encoded `ArgonautSignature::FalconWithKey { sig, pk }` (bootstrap mode).
pub fn bootstrap_envelope(sig: Vec<u8>, pk: Vec<u8>) -> Vec<u8> {
    SignatureEnvelope::FalconWithKey { sig, pk }.encode()
}

/// SCALE-encoded `ArgonautSignature::Falcon { sig }` (registry mode — no inline
/// public key; the signing payload for it is the `General`-preamble construction
/// shown in `node/examples/submit_transfer_registry.rs`).
pub fn registry_envelope(sig: Vec<u8>) -> Vec<u8> {
    SignatureEnvelope::Falcon { sig }.encode()
}

/// Sign `payload` and wrap in the bootstrap-mode envelope — the exact output of
/// `argonaut-node falcon-sign`.
pub fn sign_bootstrap(
    signing_key: &[u8],
    verifying_key: &[u8],
    payload: &[u8],
    rng: &mut (impl RngCore + CryptoRng),
) -> Result<Vec<u8>, String> {
    let sig = sign_payload(signing_key, payload, rng)?;
    Ok(bootstrap_envelope(sig, verifying_key.to_vec()))
}

#[cfg(test)]
mod tests;
