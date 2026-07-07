//! Devnet-only utility for B3 live verification: builds and submits a real
//! signed `Balances::transfer_keep_alive` extrinsic against a running node
//! over plain JSON-RPC. Shells out to `curl` rather than pulling in an RPC
//! client dependency -- this is throwaway devnet tooling, not shipped code.
//!
//! Bootstrap-mode signing (`CheckFalconSignature::disabled()`), same shape as
//! `node/src/benchmarking.rs::create_benchmark_extrinsic` -- reused here
//! parameterized by RPC-fetched chain state (genesis hash, best/finalized
//! hash+number, sender nonce) instead of an in-process client handle, since
//! this talks to an already-running node over the wire.
//!
//! Usage: `cargo run --release --example submit_transfer -- [rpc_url] [value]`
use argonaut_runtime as runtime;
use parity_scale_codec::Encode;
use serde_json::{json, Value};
use sp_core::crypto::Ss58Codec;
use sp_core::Pair;
use sp_keyring::Sr25519Keyring;
use sp_runtime::generic::Era;
use std::process::Command;

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
    let url = args.get(1).map(String::as_str).unwrap_or("http://127.0.0.1:9944");
    let value: u128 = args.get(2).map(|s| s.parse().expect("value must be a u128")).unwrap_or(1_000_000_000_000u128);

    let sender = Sr25519Keyring::Alice.pair();
    let sender_account_id = Sr25519Keyring::Alice.to_account_id();
    let sender_ss58 = sender_account_id.to_ss58check();
    let dest = Sr25519Keyring::Bob.to_account_id();
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

    let call: runtime::RuntimeCall = runtime::BalancesCall::transfer_keep_alive {
        dest: dest.into(),
        value,
    }
    .into();

    let period = runtime::BlockHashCount::get().checked_next_power_of_two().map(|c| c / 2).unwrap_or(2) as u64;
    let extra: runtime::SignedExtra = (
        // Bootstrap-mode (classic `Signed` preamble): the signature is
        // already checked via `Verify::verify` before extensions run, so
        // Falcon registry-mode checking is disabled here.
        runtime::CheckFalconSignature::disabled(),
        frame_system::CheckNonZeroSender::<runtime::Runtime>::new(),
        frame_system::CheckSpecVersion::<runtime::Runtime>::new(),
        frame_system::CheckTxVersion::<runtime::Runtime>::new(),
        frame_system::CheckGenesis::<runtime::Runtime>::new(),
        frame_system::CheckEra::<runtime::Runtime>::from(Era::mortal(period, best_number)),
        frame_system::CheckNonce::<runtime::Runtime>::from(nonce),
        frame_system::CheckWeight::<runtime::Runtime>::new(),
        pallet_transaction_payment::ChargeTransactionPayment::<runtime::Runtime>::from(0),
    );

    let raw_payload = runtime::SignedPayload::from_raw(
        call.clone(),
        extra.clone(),
        (
            (),
            (),
            runtime::VERSION.spec_version,
            runtime::VERSION.transaction_version,
            genesis_hash,
            best_hash,
            (),
            (),
            (),
        ),
    );
    let signature = raw_payload.using_encoded(|e| sender.sign(e));

    let xt = runtime::UncheckedExtrinsic::new_signed(
        call,
        sp_runtime::AccountId32::from(sender.public()).into(),
        runtime::Signature::Sr25519(signature),
        extra,
    );

    let xt_hex = format!("0x{}", hex::encode(xt.encode()));
    println!("submitting extrinsic ({sender_ss58} -> {dest_ss58}, value {value}, nonce {nonce}): {xt_hex}");
    let result = rpc(url, "author_submitExtrinsic", json!([xt_hex]));
    println!("tx hash: {result}");
}
