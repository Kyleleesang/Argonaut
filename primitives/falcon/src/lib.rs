//! Falcon-512 signature types, host-function interface, and `Verify`/`IdentifyAccount`
//! wiring (architecture guide §2).
//!
//! Sign over `blake3(scale_encoded_payload)` (32 bytes) — keeps host-call payloads
//! small and matches the account-id hash function.
//!
//! **Native only:** the actual Falcon verify runs via the `FalconCrypto` host function
//! (registered as `falcon_crypto::HostFunctions` in the node executor). Nothing in
//! `falcon_crypto` executes inside wasm; calls compile to host-function dispatch there.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use parity_scale_codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_core::{bounded::BoundedVec, crypto::AccountId32, ConstU32};
use sp_runtime::traits::{IdentifyAccount, Lazy, Verify};
use sp_runtime_interface::runtime_interface;

pub const FALCON512_PK_LEN: usize = 897;
pub const FALCON512_SIG_MAX_LEN: usize = 752;

/// A Falcon-512 public key (897 bytes, BoundedVec for SCALE safety).
#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, PartialEq, Eq, Debug)]
pub struct FalconPublic(pub BoundedVec<u8, ConstU32<897>>);

/// A Falcon-512 detached signature (up to 752 bytes).
#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, PartialEq, Eq, Debug)]
pub struct FalconSignature(pub BoundedVec<u8, ConstU32<752>>);

/// Runtime-facing signature envelope — supports bootstrap mode and registry mode.
#[derive(Encode, Decode, TypeInfo, Clone, PartialEq, Eq, Debug)]
pub enum ArgonautSignature {
    /// Signature + inline public key (bootstrap mode).
    /// Runtime: check blake3(pk) == signer, then verify.
    FalconWithKey { sig: FalconSignature, pk: FalconPublic },

    /// Signature only; pk resolved from on-chain registry.
    /// Saves ~897 bytes per tx. Checked in the `CheckFalconSignature` TransactionExtension,
    /// not here: `Verify` has no storage access, so this variant always fails `verify()`
    /// and must never be accepted by the default `Checkable` signature check alone.
    Falcon { sig: FalconSignature },

    /// Dev-only escape hatch (sr25519 bootstrap signing, e.g. `//Alice`-style dev
    /// accounts). Gated on the dedicated `dev-signatures` feature -- *not*
    /// `#[cfg(feature = "std")]`, which is what caused a real, live bug: `Verify`
    /// for `sr25519::Signature` (`sp_runtime::traits`) dispatches to the
    /// `sp_io::crypto` host function and works fine in wasm, same as any other
    /// Substrate chain's sr25519 accounts, so there was never a technical need to
    /// gate this out of wasm specifically. But gating it on `std` did exactly
    /// that (wasm is built `no_std`) while every *native* caller (chain specs,
    /// benchmarking, CLI tooling -- all built with `std`) kept emitting it,
    /// silently disagreeing with the wasm runtime about this enum's variant
    /// count. Every sr25519-signed extrinsic became SCALE-undecodable on-chain
    /// -- found live, via `TaggedTransactionQueue_validate_transaction` trapping
    /// with `wasm unreachable` on a perfectly well-formed signed extrinsic.
    ///
    /// `dev-signatures` fixes both problems at once: it's a single feature
    /// applied identically wherever this crate is compiled (so wasm and native
    /// never disagree on the enum shape), and it's off by default so a
    /// production wasm runtime doesn't accept this non-post-quantum signature
    /// scheme at all unless a caller deliberately opts in (see
    /// `primitives/falcon/Cargo.toml` and `runtime/Cargo.toml` for how the
    /// feature is (deliberately not) threaded into the wasm build).
    #[cfg(feature = "dev-signatures")]
    Sr25519(sp_core::sr25519::Signature),
}

/// Signer counterpart to [`ArgonautSignature`], used only to derive an `AccountId`.
/// Also gated on `dev-signatures`: it never itself crosses the wasm boundary
/// (`AccountId` resolves to the plain `AccountId32` associated type, not this
/// enum), so this gate has no wire-format-mismatch risk the way
/// `ArgonautSignature::Sr25519`'s did -- gated anyway, for one single feature
/// flag to control "is sr25519 dev tooling available," not two.
#[derive(Encode, Decode, TypeInfo, Clone, PartialEq, Eq, Debug)]
pub enum ArgonautSigner {
    Falcon(FalconPublic),
    #[cfg(feature = "dev-signatures")]
    Sr25519(sp_core::sr25519::Public),
}

/// Lets dev/testnet chain specs derive accounts from well-known sr25519 seeds
/// (e.g. `//Alice`) the same way they always have, via `AccountPublic::from(..)`.
#[cfg(feature = "dev-signatures")]
impl From<sp_core::sr25519::Public> for ArgonautSigner {
    fn from(pk: sp_core::sr25519::Public) -> Self {
        ArgonautSigner::Sr25519(pk)
    }
}

impl IdentifyAccount for ArgonautSigner {
    type AccountId = AccountId32;

    fn into_account(self) -> AccountId32 {
        match self {
            ArgonautSigner::Falcon(pk) => AccountId32::from(falcon_crypto::pk_hash(&pk.0)),
            #[cfg(feature = "dev-signatures")]
            ArgonautSigner::Sr25519(pk) => AccountId32::from(pk),
        }
    }
}

impl Verify for ArgonautSignature {
    type Signer = ArgonautSigner;

    fn verify<L: Lazy<[u8]>>(&self, mut msg: L, signer: &AccountId32) -> bool {
        let who: [u8; 32] = *signer.as_ref();
        match self {
            ArgonautSignature::FalconWithKey { sig, pk } => {
                if falcon_crypto::pk_hash(&pk.0) != who {
                    return false;
                }
                let msg_hash = blake3_hash(msg.get());
                falcon_crypto::verify(&sig.0, &msg_hash, &pk.0)
            },
            // Registry-mode carries no inline pk, so `Verify` alone cannot resolve it
            // (no storage access here). Always false: acceptance is decided exclusively
            // by the `CheckFalconSignature` TransactionExtension, which does have
            // storage access to look the key up.
            ArgonautSignature::Falcon { .. } => false,
            #[cfg(feature = "dev-signatures")]
            ArgonautSignature::Sr25519(sig) => sig.verify(msg, &who.into()),
        }
    }
}

/// Compute blake3 over arbitrary bytes. Used for payload signing and account-id derivation.
pub fn blake3_hash(data: &[u8]) -> [u8; 32] {
    *blake3::hash(data).as_bytes()
}

/// Host-function interface for Falcon-512 crypto (native-only; no wasm impl).
///
/// Register `falcon_crypto::HostFunctions` in the `ExtendHostFunctions` tuple in the
/// node executor (`node/src/service.rs`).
#[runtime_interface]
pub trait FalconCrypto {
    /// Verify a Falcon-512 detached signature.
    /// `msg` should be blake3(scale_encoded_payload) — 32 bytes.
    fn verify(sig: &[u8], msg: &[u8], pk: &[u8]) -> bool {
        use fn_dsa_vrfy::{VerifyingKey, VerifyingKeyStandard, DOMAIN_NONE, HASH_ID_RAW};
        if pk.len() != FALCON512_PK_LEN {
            return false;
        }
        match VerifyingKeyStandard::decode(pk) {
            Some(vk) => vk.verify(sig, &DOMAIN_NONE, &HASH_ID_RAW, msg),
            None => false,
        }
    }

    /// Compute blake3(pk) → 32-byte account id.
    fn pk_hash(pk: &[u8]) -> [u8; 32] {
        blake3_hash(pk)
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
        let pk = alloc::vec![0u8; 897];
        let via_fn = falcon_crypto::pk_hash(&pk);
        let direct = blake3_hash(&pk);
        assert_eq!(via_fn, direct);
    }

    /// Real Falcon-512 keypair: sign with `fn-dsa` (dev-dependency, full signing half),
    /// verify with the same `falcon_crypto::verify` host function the runtime calls.
    fn keypair(seed: u8) -> (alloc::vec::Vec<u8>, alloc::vec::Vec<u8>) {
        use fn_dsa::{sign_key_size, vrfy_key_size, KeyPairGenerator, KeyPairGeneratorStandard, FN_DSA_LOGN_512};
        let mut kg = KeyPairGeneratorStandard::default();
        let mut sign_key = alloc::vec![0u8; sign_key_size(FN_DSA_LOGN_512)];
        let mut vrfy_key = alloc::vec![0u8; vrfy_key_size(FN_DSA_LOGN_512)];
        kg.keygen(FN_DSA_LOGN_512, &mut rand_seed(seed), &mut sign_key, &mut vrfy_key);
        (sign_key, vrfy_key)
    }

    /// Minimal deterministic PRNG satisfying `fn_dsa`'s `RngCore` bound, seeded for
    /// reproducible tests (no OS randomness dependency).
    fn rand_seed(seed: u8) -> impl rand_core::RngCore + rand_core::CryptoRng {
        use rand_core::SeedableRng;
        rand_chacha::ChaCha20Rng::from_seed([seed; 32])
    }

    #[test]
    fn falcon_sign_verify_round_trip() {
        use fn_dsa::{SigningKey, SigningKeyStandard, DOMAIN_NONE, HASH_ID_RAW};

        let (sign_key, vrfy_key) = keypair(1);
        let mut sk = SigningKeyStandard::decode(&sign_key).unwrap();
        let msg = b"argonaut bootstrap-mode transaction payload";
        let mut sig = alloc::vec![0u8; fn_dsa::signature_size(sk.get_logn())];
        sk.sign(&mut rand_seed(2), &DOMAIN_NONE, &HASH_ID_RAW, msg, &mut sig);

        assert!(falcon_crypto::verify(&sig, msg, &vrfy_key));
    }

    #[test]
    fn falcon_verify_rejects_corrupted_signature() {
        use fn_dsa::{SigningKey, SigningKeyStandard, DOMAIN_NONE, HASH_ID_RAW};

        let (sign_key, vrfy_key) = keypair(3);
        let mut sk = SigningKeyStandard::decode(&sign_key).unwrap();
        let msg = b"argonaut bootstrap-mode transaction payload";
        let mut sig = alloc::vec![0u8; fn_dsa::signature_size(sk.get_logn())];
        sk.sign(&mut rand_seed(4), &DOMAIN_NONE, &HASH_ID_RAW, msg, &mut sig);
        sig[50] ^= 0x01;

        assert!(!falcon_crypto::verify(&sig, msg, &vrfy_key));
    }

    #[test]
    fn falcon_verify_rejects_corrupted_pk() {
        use fn_dsa::{SigningKey, SigningKeyStandard, DOMAIN_NONE, HASH_ID_RAW};

        let (sign_key, mut vrfy_key) = keypair(5);
        let mut sk = SigningKeyStandard::decode(&sign_key).unwrap();
        let msg = b"argonaut bootstrap-mode transaction payload";
        let mut sig = alloc::vec![0u8; fn_dsa::signature_size(sk.get_logn())];
        sk.sign(&mut rand_seed(6), &DOMAIN_NONE, &HASH_ID_RAW, msg, &mut sig);
        vrfy_key[10] ^= 0x01;

        assert!(!falcon_crypto::verify(&sig, msg, &vrfy_key));
    }

    #[test]
    fn falcon_verify_rejects_wrong_message() {
        use fn_dsa::{SigningKey, SigningKeyStandard, DOMAIN_NONE, HASH_ID_RAW};

        let (sign_key, vrfy_key) = keypair(7);
        let mut sk = SigningKeyStandard::decode(&sign_key).unwrap();
        let msg = b"argonaut bootstrap-mode transaction payload";
        let mut sig = alloc::vec![0u8; fn_dsa::signature_size(sk.get_logn())];
        sk.sign(&mut rand_seed(8), &DOMAIN_NONE, &HASH_ID_RAW, msg, &mut sig);

        assert!(!falcon_crypto::verify(&sig, b"a different payload entirely", &vrfy_key));
    }

    #[test]
    fn argonaut_signature_verify_bootstrap_mode() {
        use fn_dsa::{SigningKey, SigningKeyStandard, DOMAIN_NONE, HASH_ID_RAW};

        let (sign_key, vrfy_key) = keypair(9);
        let mut sk = SigningKeyStandard::decode(&sign_key).unwrap();
        let payload = b"scale-encoded call + extra";
        let msg_hash = blake3_hash(payload);
        let mut sig_bytes = alloc::vec![0u8; fn_dsa::signature_size(sk.get_logn())];
        sk.sign(&mut rand_seed(10), &DOMAIN_NONE, &HASH_ID_RAW, &msg_hash, &mut sig_bytes);

        let pk = FalconPublic(BoundedVec::try_from(vrfy_key).unwrap());
        let signer_account = AccountId32::from(blake3_hash(&pk.0));
        let sig = ArgonautSignature::FalconWithKey {
            sig: FalconSignature(BoundedVec::try_from(sig_bytes).unwrap()),
            pk,
        };

        assert!(sig.verify(&payload[..], &signer_account));
    }

    #[test]
    fn argonaut_signature_verify_rejects_mismatched_account() {
        use fn_dsa::{SigningKey, SigningKeyStandard, DOMAIN_NONE, HASH_ID_RAW};

        let (sign_key, vrfy_key) = keypair(11);
        let mut sk = SigningKeyStandard::decode(&sign_key).unwrap();
        let payload = b"scale-encoded call + extra";
        let msg_hash = blake3_hash(payload);
        let mut sig_bytes = alloc::vec![0u8; fn_dsa::signature_size(sk.get_logn())];
        sk.sign(&mut rand_seed(12), &DOMAIN_NONE, &HASH_ID_RAW, &msg_hash, &mut sig_bytes);

        let pk = FalconPublic(BoundedVec::try_from(vrfy_key).unwrap());
        let sig = ArgonautSignature::FalconWithKey {
            sig: FalconSignature(BoundedVec::try_from(sig_bytes).unwrap()),
            pk,
        };

        assert!(!sig.verify(&payload[..], &AccountId32::from([0u8; 32])));
    }

    #[test]
    fn argonaut_signature_registry_mode_never_verifies_directly() {
        let sig = ArgonautSignature::Falcon {
            sig: FalconSignature(BoundedVec::try_from(alloc::vec![0u8; 100]).unwrap()),
        };
        assert!(!sig.verify(&b"anything"[..], &AccountId32::from([0u8; 32])));
    }
}
