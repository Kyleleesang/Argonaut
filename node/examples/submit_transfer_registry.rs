//! A-3: registry-mode counterpart to `submit_transfer.rs` -- builds and submits a
//! `Balances::transfer_keep_alive` extrinsic signed via `CheckFalconSignature::RegistryMode`
//! (a `General`-preamble extrinsic, no inline public key) instead of bootstrap mode's
//! classic `Signed` preamble. Same RPC-shelling-to-curl approach and devnet-only scope
//! as `submit_transfer.rs`.
//!
//! Prerequisite: the signer's Falcon public key must already be registered on-chain
//! (`pallet_falcon_registry::register_key`, called via a bootstrap-mode extrinsic signed
//! with the same key -- `node/src/falcon_sign.rs` + `submit_transfer.rs`'s own signing
//! plumbing already cover building that, just with `register_key` as the call instead of
//! a transfer) and funded enough to cover `KeyDeposit` + fees. This tool only covers the
//! registry-mode *transfer*, per the fix plan's own A-3 scope.
//!
//! # Signing payload (why this isn't `SignedPayload::from_raw`)
//! `CheckFalconSignature` runs first in `SignedExtra` and authorizes the origin itself, so
//! for a `General` preamble there's no outer `Signed`-style payload to reuse -- the
//! extension computes its own message from `inherited_implication`. Working through
//! `sp_runtime`'s tuple `TransactionExtension::validate` (see
//! `substrate/primitives/runtime/src/traits/transaction_extension/{mod,dispatch_transaction}.rs`
//! in this project's polkadot-sdk pin) for the extension at position 0 in an N-tuple:
//! the bytes it hashes are `(extension_version, call).encode() ++ tail.encode() ++
//! tail_implicit.encode()`, where `tail`/`tail_implicit` are the *explicit*/*implicit* data
//! of every extension *after* position 0 (positions 1..N-1), and `extension_version` is the
//! constant `UncheckedExtrinsic::new_transaction` always stamps (0, in this polkadot-sdk
//! pin). This is structurally close to (but not byte-identical with -- the version byte
//! moves and `CheckFalconSignature` itself drops out) the old `SignedPayload` shape
//! `submit_transfer.rs` uses for bootstrap mode.
use argonaut_runtime as runtime;
use fn_dsa::{signature_size, SigningKey, SigningKeyStandard, DOMAIN_NONE, HASH_ID_RAW};
use parity_scale_codec::Encode;
use rand_core::OsRng;
use serde::Deserialize;
use serde_json::{json, Value};
use sp_core::bounded::BoundedVec;
use sp_core::crypto::{AccountId32, Ss58Codec};
use std::process::Command;

/// Same on-disk shape as `node/src/keygen.rs::FalconKeystoreFile` -- deliberately not
/// importing that type directly (this is a separate `[[example]]` binary target, not
/// linked against the node's own binary crate), just its JSON shape.
#[derive(Deserialize)]
struct FalconKeystoreFile {
    #[serde(with = "hex::serde")]
    public_key: Vec<u8>,
    encrypted: bool,
    #[serde(with = "hex::serde")]
    secret_key: Vec<u8>,
    #[serde(with = "hex::serde")]
    kdf_salt: Vec<u8>,
    #[serde(with = "hex::serde")]
    nonce: Vec<u8>,
}

fn rpc(url: &str, method: &str, params: Value) -> Value {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
    let output = Command::new("curl")
        .args(["-s", "-H", "Content-Type: application/json", "-d", &body, url])
        .output()
        .expect("failed to run curl -- is it installed?");
    let parsed: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!("bad RPC response for {method}: {e}\nraw: {:?}", String::from_utf8_lossy(&output.stdout))
    });
    if let Some(err) = parsed.get("error") {
        panic!("RPC error calling {method}: {err}");
    }
    parsed["result"].clone()
}

fn decode_hash(hex_str: &str) -> sp_core::H256 {
    sp_core::H256::from_slice(&hex::decode(hex_str.trim_start_matches("0x")).expect("valid hex"))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let keystore_path = args.get(1).expect("usage: submit_transfer_registry <keystore.falcon.json> [passphrase] [rpc_url] [value]");
    let passphrase = args.get(2).filter(|s| !s.is_empty());
    let url = args.get(3).map(String::as_str).unwrap_or("http://127.0.0.1:9944");
    let value: u128 = args.get(4).map(|s| s.parse().expect("value must be a u128")).unwrap_or(1_000_000_000_000u128);

    let json = std::fs::read_to_string(keystore_path).expect("failed to read keystore file");
    let file: FalconKeystoreFile = serde_json::from_str(&json).expect("invalid keystore JSON");
    let sign_key = if file.encrypted {
        use chacha20poly1305::{aead::{Aead, KeyInit}, ChaCha20Poly1305, Nonce};
        let passphrase = passphrase.expect("keystore is encrypted; pass the passphrase as the 2nd argument");
        let mut key_material = file.kdf_salt.clone();
        key_material.extend_from_slice(passphrase.as_bytes());
        let derived_key = blake3::derive_key("argonaut-falcon-keystore-v1", &key_material);
        let cipher = ChaCha20Poly1305::new((&derived_key).into());
        cipher
            .decrypt(Nonce::from_slice(&file.nonce), file.secret_key.as_ref())
            .expect("failed to decrypt signing key: wrong passphrase?")
    } else {
        file.secret_key.clone()
    };

    let sender_account = AccountId32::from(argonaut_primitives_falcon::blake3_hash(&file.public_key));
    let sender_ss58 = sender_account.to_ss58check();
    let dest = sp_keyring::Sr25519Keyring::Bob.to_account_id();
    let dest_ss58 = dest.to_ss58check();

    let genesis_hash = decode_hash(rpc(url, "chain_getBlockHash", json!([0])).as_str().expect("genesis hash"));
    let best_hash_hex = rpc(url, "chain_getFinalizedHead", json!([])).as_str().expect("finalized head").to_string();
    let best_hash = decode_hash(&best_hash_hex);
    let header = rpc(url, "chain_getHeader", json!([best_hash_hex]));
    let best_number = u64::from_str_radix(
        header["number"].as_str().expect("header number").trim_start_matches("0x"),
        16,
    )
    .expect("valid hex block number");
    let nonce = rpc(url, "system_accountNextIndex", json!([sender_ss58]))
        .as_u64()
        .expect("nonce") as u32;

    let call: runtime::RuntimeCall =
        runtime::BalancesCall::transfer_keep_alive { dest: dest.into(), value }.into();

    let period = runtime::BlockHashCount::get().checked_next_power_of_two().map(|c| c / 2).unwrap_or(2) as u64;
    let era = sp_runtime::generic::Era::mortal(period, best_number);

    // The 8 extensions *after* `CheckFalconSignature` in `SignedExtra` -- identical
    // shape/values to bootstrap mode's `extra` in `submit_transfer.rs`, just without
    // `CheckFalconSignature` itself as the first element (it isn't part of its own
    // signing payload -- see this file's module doc).
    let tail_extra = (
        frame_system::CheckNonZeroSender::<runtime::Runtime>::new(),
        frame_system::CheckSpecVersion::<runtime::Runtime>::new(),
        frame_system::CheckTxVersion::<runtime::Runtime>::new(),
        frame_system::CheckGenesis::<runtime::Runtime>::new(),
        frame_system::CheckEra::<runtime::Runtime>::from(era),
        frame_system::CheckNonce::<runtime::Runtime>::from(nonce),
        frame_system::CheckWeight::<runtime::Runtime>::new(),
        pallet_transaction_payment::ChargeTransactionPayment::<runtime::Runtime>::from(0),
    );
    let tail_implicit = (
        (),
        runtime::VERSION.spec_version,
        runtime::VERSION.transaction_version,
        genesis_hash,
        best_hash, // CheckEra's implicit: the era's checkpoint block hash
        (),
        (),
        (),
    );

    const EXTENSION_VERSION: u8 = 0; // matches `UncheckedExtrinsic::new_transaction`'s constant
    let signing_payload = (EXTENSION_VERSION, &call, &tail_extra, &tail_implicit).encode();
    let msg_hash = argonaut_primitives_falcon::blake3_hash(&signing_payload);

    let mut sk = SigningKeyStandard::decode(&sign_key).expect("keystore signing key is corrupt or wrong length");
    let mut sig_bytes = vec![0u8; signature_size(sk.get_logn())];
    sk.sign(&mut OsRng, &DOMAIN_NONE, &HASH_ID_RAW, &msg_hash, &mut sig_bytes);

    let extra: runtime::SignedExtra = (
        runtime::CheckFalconSignature::registry_mode(
            argonaut_primitives_falcon::FalconSignature(
                BoundedVec::try_from(sig_bytes).expect("signature too long"),
            ),
            sender_account.clone(),
        ),
        tail_extra.0,
        tail_extra.1,
        tail_extra.2,
        tail_extra.3,
        tail_extra.4,
        tail_extra.5,
        tail_extra.6,
        tail_extra.7,
    );

    let xt = runtime::UncheckedExtrinsic::new_transaction(call, extra);

    let xt_hex = format!("0x{}", hex::encode(xt.encode()));
    println!(
        "submitting registry-mode extrinsic ({sender_ss58} -> {dest_ss58}, value {value}, nonce {nonce}, {} bytes): {xt_hex}",
        xt.encode().len()
    );
    let result = rpc(url, "author_submitExtrinsic", json!([xt_hex]));
    println!("tx hash: {result}");
}
