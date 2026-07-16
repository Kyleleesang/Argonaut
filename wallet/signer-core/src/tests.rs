//! Byte-compat pins against the real chain types: everything the wasm build
//! can't link (sp-core, the falcon primitives) is cross-checked here natively,
//! so the browser signer and the node can never silently disagree.

use super::*;
use crate::{keystore, ss58};
use argonaut_primitives_falcon::{
    blake3_hash, falcon_crypto, ArgonautSignature, FalconPublic, FalconSignature,
};
use parity_scale_codec::Decode;
use rand_core::SeedableRng;
use sp_core::bounded::BoundedVec;
use sp_core::crypto::{AccountId32, Ss58AddressFormat, Ss58Codec};
use sp_runtime::traits::Verify;

fn rng(seed: u8) -> rand_chacha::ChaCha20Rng {
    rand_chacha::ChaCha20Rng::from_seed([seed; 32])
}

#[test]
fn account_id_matches_primitives_blake3() {
    let kp = generate_keypair(&mut rng(1));
    assert_eq!(kp.verifying_key.len(), FALCON512_PK_LEN);
    assert_eq!(account_id(&kp.verifying_key), blake3_hash(&kp.verifying_key));
}

#[test]
fn ss58_matches_sp_core() {
    let kp = generate_keypair(&mut rng(2));
    let account = AccountId32::from(account_id(&kp.verifying_key));
    assert_eq!(
        ss58::ss58_address(&account_id(&kp.verifying_key)),
        account.to_ss58check()
    );
    // Two-byte prefix form too, in case the chain ever registers its own.
    assert_eq!(
        ss58::ss58_address_with_prefix(&account_id(&kp.verifying_key), 2254),
        account.to_ss58check_with_version(Ss58AddressFormat::custom(2254))
    );
}

#[test]
fn envelope_bytes_match_argonaut_signature() {
    use parity_scale_codec::Encode;

    let sig = vec![7u8; 666];
    let pk = vec![9u8; 897];

    let real = ArgonautSignature::FalconWithKey {
        sig: FalconSignature(BoundedVec::try_from(sig.clone()).unwrap()),
        pk: FalconPublic(BoundedVec::try_from(pk.clone()).unwrap()),
    };
    assert_eq!(bootstrap_envelope(sig.clone(), pk), real.encode());

    let real_registry = ArgonautSignature::Falcon {
        sig: FalconSignature(BoundedVec::try_from(sig.clone()).unwrap()),
    };
    assert_eq!(registry_envelope(sig), real_registry.encode());
}

/// End-to-end through the runtime's exact acceptance path: decode the envelope
/// as `ArgonautSignature`, then `Verify::verify` — which recomputes
/// `blake3(pk) == signer` and calls the `falcon_crypto::verify` host function.
#[test]
fn signed_envelope_passes_runtime_verification() {
    let kp = generate_keypair(&mut rng(3));
    let payload = b"scale-encoded call + extra";
    let envelope =
        sign_bootstrap(&kp.signing_key, &kp.verifying_key, payload, &mut rng(4)).unwrap();

    let decoded = ArgonautSignature::decode(&mut &envelope[..]).unwrap();
    let account = AccountId32::from(account_id(&kp.verifying_key));
    assert!(decoded.verify(&payload[..], &account));

    // Wrong account must fail.
    assert!(!decoded.verify(&payload[..], &AccountId32::from([0u8; 32])));
}

#[test]
fn raw_signature_passes_host_function_verify() {
    let kp = generate_keypair(&mut rng(5));
    let payload = b"registry-mode signing payload";
    let sig = sign_payload(&kp.signing_key, payload, &mut rng(6)).unwrap();

    assert!(falcon_crypto::verify(&sig, &blake3_hash(payload), &kp.verifying_key));
    assert!(!falcon_crypto::verify(&sig, &blake3_hash(b"other payload"), &kp.verifying_key));
}

#[test]
fn keystore_seal_open_round_trip_encrypted() {
    let kp = generate_keypair(&mut rng(7));
    let file = keystore::seal(&kp, Some("hunter2"), &mut rng(8)).unwrap();
    assert!(file.encrypted);
    assert_eq!(file.open(Some("hunter2")).unwrap(), kp.signing_key);
    assert!(file.open(Some("wrong")).is_err());
    assert!(file.open(None).is_err());
}

#[test]
fn keystore_plaintext_round_trip() {
    let kp = generate_keypair(&mut rng(9));
    let file = keystore::seal(&kp, None, &mut rng(10)).unwrap();
    assert!(!file.encrypted);
    assert_eq!(file.open(None).unwrap(), kp.signing_key);
    assert_eq!(file.address, AccountId32::from(account_id(&kp.verifying_key)).to_ss58check());
}

/// Pin the JSON field set so browser- and CLI-written keystores stay
/// interchangeable.
#[test]
fn keystore_json_format_is_stable() {
    let kp = generate_keypair(&mut rng(11));
    let file = keystore::seal(&kp, Some("pw"), &mut rng(12)).unwrap();
    let value: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&file).unwrap()).unwrap();
    let mut keys: Vec<&str> = value.as_object().unwrap().keys().map(|s| s.as_str()).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["address", "encrypted", "kdf_salt", "nonce", "public_key", "secret_key"]
    );
}

/// Golden vectors: pin fn-dsa 0.2.0's key/signature byte behavior
/// (FRONTEND_DAPP_SPEC §4.4). If a dependency bump changes these, existing
/// keys/signatures are no longer byte-compatible — that's the planned FN-DSA
/// pre-standard migration event (fn-dsa README: no backward compatibility
/// before 1.0), not a test to quietly "fix".
#[test]
fn golden_vectors_pin_fn_dsa_behavior() {
    let kp = generate_keypair(&mut rng(42));
    let payload = b"argonaut golden vector payload v1";
    let envelope =
        sign_bootstrap(&kp.signing_key, &kp.verifying_key, payload, &mut rng(43)).unwrap();

    assert_eq!(
        [
            hex::encode(blake3_hash(&kp.verifying_key)),
            hex::encode(blake3_hash(&kp.signing_key)),
            hex::encode(blake3_hash(&envelope)),
        ],
        [
            "86b666a601501c7f90055e87deedbfdf81ba722b6ac48e523722898b22ea0b09",
            "8cdb2c86c1d8d4e054108d19b801ba2ffcbc3414b5337d5480e8e254395d3c14",
            "b4305eab3acdfa7f77db0bb94d6562e8b66860c1d556f2644abea232cea21b3f",
        ]
    );
}
