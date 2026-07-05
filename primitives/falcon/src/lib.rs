//! Falcon-512 signature types and host-function interface.
//!
//! Sign over `blake3(scale_encoded_payload)` (32 bytes) — keeps host-call payloads
//! small and matches the account-id hash function.
//!
//! **Native only:** the actual Falcon verify runs via the `FalconCrypto` host function.
//! Nothing here executes inside wasm.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::vec::Vec;
use parity_scale_codec::{Decode, Encode};
use scale_info::TypeInfo;

pub const FALCON512_PK_LEN: usize = 897;
pub const FALCON512_SIG_MAX_LEN: usize = 752;

/// A Falcon-512 public key (897 bytes, BoundedVec for SCALE safety).
#[derive(Encode, Decode, TypeInfo, Clone, PartialEq, Eq, Debug)]
pub struct FalconPublic(pub Vec<u8>);

/// A Falcon-512 detached signature (up to 752 bytes).
#[derive(Encode, Decode, TypeInfo, Clone, PartialEq, Eq, Debug)]
pub struct FalconSignature(pub Vec<u8>);

/// Runtime-facing signature envelope — supports bootstrap mode and registry mode.
#[derive(Encode, Decode, TypeInfo, Clone, PartialEq, Eq, Debug)]
pub enum ArgonautSignature {
    /// Signature + inline public key (bootstrap mode).
    /// Runtime: check blake3(pk) == signer, then verify.
    FalconWithKey { sig: FalconSignature, pk: FalconPublic },

    /// Signature only; pk resolved from on-chain registry.
    /// Saves ~897 bytes per tx. Checked in CheckFalconSignature TransactionExtension.
    Falcon { sig: FalconSignature },

    /// Dev-only escape hatch — feature-gated so it never ships in release.
    #[cfg(feature = "std")]
    Sr25519(sp_core::sr25519::Signature),
}

/// Compute blake3 over arbitrary bytes. Used for payload signing and account-id derivation.
pub fn blake3_hash(data: &[u8]) -> [u8; 32] {
    *blake3::hash(data).as_bytes()
}

/// Host-function interface for Falcon-512 crypto (native-only; no wasm impl).
///
/// Registered via `falcon_crypto::HostFunctions` in node/src/service.rs executor.
pub mod falcon_crypto {
    /// Verify a Falcon-512 detached signature.
    /// msg should be blake3(scale_encoded_payload) — 32 bytes.
    pub fn verify(sig: &[u8], msg: &[u8], pk: &[u8]) -> bool {
        #[cfg(feature = "std")]
        {
            use fn_dsa_vrfy::{verify_key_size, verify_signature, FalconVariant};
            if pk.len() != super::FALCON512_PK_LEN {
                return false;
            }
            if !verify_key_size(FalconVariant::Falcon512, pk) {
                return false;
            }
            verify_signature(FalconVariant::Falcon512, pk, msg, sig).unwrap_or(false)
        }
        #[cfg(not(feature = "std"))]
        {
            let _ = (sig, msg, pk);
            false
        }
    }

    /// Compute blake3(pk) → 32-byte account id.
    pub fn pk_hash(pk: &[u8]) -> [u8; 32] {
        super::blake3_hash(pk)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blake3_hash_is_deterministic() {
        let a = blake3_hash(b"hello");
        let b = blake3_hash(b"hello");
        assert_eq!(a, b);
    }

    #[test]
    fn blake3_hash_differs_on_different_input() {
        let a = blake3_hash(b"hello");
        let b = blake3_hash(b"world");
        assert_ne!(a, b);
    }

    #[test]
    fn pk_hash_matches_blake3() {
        let pk = vec![0u8; 897];
        let via_fn = falcon_crypto::pk_hash(&pk);
        let direct = blake3_hash(&pk);
        assert_eq!(via_fn, direct);
    }
}
