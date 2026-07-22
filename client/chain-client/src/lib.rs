//! Thin native-only chain client (D.11), shared by the D3 provider daemon and
//! any CLI tooling that needs to submit real signed extrinsics or read
//! storage. Depends on `argonaut-runtime` directly (no subxt metadata churn,
//! per the spec's own rationale) and talks plain JSON-RPC over a persistent
//! WebSocket connection, so both request/response calls and the
//! `chain_subscribeFinalizedHeads` subscription share one connection.
//!
//! Deliberately small (D.11: "keep it < ~600 lines"): extrinsic building
//! lives in [`extrinsic`], storage-key computation in [`keys`], head
//! subscription in [`subscribe`]. This module is just the connection +
//! request glue.

pub mod channels;
pub mod extrinsic;
pub mod keys;
pub mod subscribe;

use argonaut_runtime::{self as runtime, RuntimeCall, UncheckedExtrinsic};
use extrinsic::{build_and_sign, SignMode, SigningContext};
use jsonrpsee::core::client::ClientT;
use jsonrpsee::rpc_params;
use jsonrpsee::ws_client::{WsClient, WsClientBuilder};
use parity_scale_codec::{Decode, Encode};
use rand_core::{CryptoRng, RngCore};
use sp_core::crypto::{AccountId32, Ss58Codec};
use sp_core::H256;

#[derive(thiserror::Error, Debug)]
pub enum ClientError {
    #[error("RPC error: {0}")]
    Rpc(#[from] jsonrpsee::core::ClientError),
    #[error("subscription item decode error: {0}")]
    SubscriptionDecode(#[from] serde_json::Error),
    #[error("hex decode error: {0}")]
    Hex(#[from] hex::FromHexError),
    #[error("SCALE decode error: {0}")]
    Scale(#[from] parity_scale_codec::Error),
    #[error("extrinsic construction error: {0}")]
    Extrinsic(String),
    #[error("unexpected RPC response shape for {method}: {detail}")]
    BadResponse { method: &'static str, detail: String },
}

fn decode_hash(s: &str) -> Result<H256, ClientError> {
    let bytes = hex::decode(s.trim_start_matches("0x"))?;
    Ok(H256::from_slice(&bytes))
}

fn decode_hex_bytes(s: &str) -> Result<Vec<u8>, ClientError> {
    Ok(hex::decode(s.trim_start_matches("0x"))?)
}

fn encode_hex(bytes: &[u8]) -> String {
    format!("0x{}", hex::encode(bytes))
}

/// A persistent connection to one Argonaut node. Not `Clone` (jsonrpsee's
/// `Client` manages a drop-triggered background-task shutdown channel that
/// doesn't support cheap duplication) -- a consumer that needs the chain
/// from two places concurrently (e.g. the D4 daemon's `Responder` loop and
/// its iroh protocol handlers) should open a second `connect()`, not try to
/// share one.
pub struct ChainClient {
    ws: WsClient,
}

impl ChainClient {
    pub async fn connect(url: &str) -> Result<Self, ClientError> {
        let ws = WsClientBuilder::default().build(url).await?;
        Ok(Self { ws })
    }

    /// The finalized head's hash and block number -- used both as the
    /// `CheckEra` checkpoint and as a general chain-tip query. There is no
    /// separate "best" to track: Prism-backed blocks import
    /// `finalized: true` and never reorg (guide §3.1), so best == finalized
    /// always on this chain.
    pub async fn finalized_head(&self) -> Result<(H256, u64), ClientError> {
        let hash_hex: String = self.ws.request("chain_getFinalizedHead", rpc_params![]).await?;
        let hash = decode_hash(&hash_hex)?;
        let header: serde_json::Value =
            self.ws.request("chain_getHeader", rpc_params![hash_hex.clone()]).await?;
        let number_hex = header.get("number").and_then(|v| v.as_str()).ok_or_else(|| {
            ClientError::BadResponse { method: "chain_getHeader", detail: "missing .number".into() }
        })?;
        let number = u64::from_str_radix(number_hex.trim_start_matches("0x"), 16).map_err(|e| {
            ClientError::BadResponse { method: "chain_getHeader", detail: e.to_string() }
        })?;
        Ok((hash, number))
    }

    pub async fn genesis_hash(&self) -> Result<H256, ClientError> {
        let hash_hex: String =
            self.ws.request("chain_getBlockHash", rpc_params![0u32]).await?;
        decode_hash(&hash_hex)
    }

    pub async fn account_nonce(&self, account: &AccountId32) -> Result<u32, ClientError> {
        let ss58 = account.to_ss58check();
        let nonce: u64 = self.ws.request("system_accountNextIndex", rpc_params![ss58]).await?;
        Ok(nonce as u32)
    }

    /// A `SigningContext` populated from live chain state -- everything
    /// [`extrinsic::build_and_sign`] needs, fetched in one round trip's worth
    /// of calls (three small requests; no caching here, callers that submit
    /// many extrinsics in a row should cache the genesis hash themselves,
    /// it never changes).
    pub async fn signing_context(&self, account: &AccountId32) -> Result<SigningContext, ClientError> {
        let genesis_hash = self.genesis_hash().await?;
        let (checkpoint_hash, checkpoint_number) = self.finalized_head().await?;
        let nonce = self.account_nonce(account).await?;
        Ok(SigningContext {
            genesis_hash,
            checkpoint_hash,
            checkpoint_number,
            spec_version: runtime::VERSION.spec_version,
            tx_version: runtime::VERSION.transaction_version,
            nonce,
        })
    }

    /// Build, sign, and submit `call` in `mode`. Returns the transaction hash
    /// reported by `author_submitExtrinsic` (pool-accepted, not yet
    /// necessarily included -- callers that need inclusion should watch
    /// [`subscribe::finalized_heads`] and match by extrinsic hash, or use
    /// `author_submitAndWatchExtrinsic` in a future revision if that
    /// granularity turns out to matter).
    pub async fn submit_call(
        &self,
        call: RuntimeCall,
        mode: SignMode,
        signing_key: &[u8],
        signer_account: &AccountId32,
        rng: &mut (impl RngCore + CryptoRng),
    ) -> Result<H256, ClientError> {
        let ctx = self.signing_context(signer_account).await?;
        let xt = build_and_sign(call, mode, &ctx, signing_key, rng)
            .map_err(ClientError::Extrinsic)?;
        self.submit_extrinsic(&xt).await
    }

    /// Lower-level: submit an already-built extrinsic (e.g. one signed
    /// out-of-process, or a bootstrap-mode registration built once and
    /// resubmitted after a nonce race).
    pub async fn submit_extrinsic(&self, xt: &UncheckedExtrinsic) -> Result<H256, ClientError> {
        let xt_hex = encode_hex(&xt.encode());
        let hash_hex: String =
            self.ws.request("author_submitExtrinsic", rpc_params![xt_hex]).await?;
        decode_hash(&hash_hex)
    }

    /// Raw storage read at a manually computed key (see [`keys`]). Returns
    /// `None` if the key has no value (matches `OptionQuery` storage items;
    /// for `ValueQuery` items the caller should treat `None` as the type's
    /// default, same as the runtime does).
    pub async fn read_storage<V: Decode>(&self, key: &[u8]) -> Result<Option<V>, ClientError> {
        let key_hex = encode_hex(key);
        let value_hex: Option<String> =
            self.ws.request("state_getStorage", rpc_params![key_hex]).await?;
        match value_hex {
            None => Ok(None),
            Some(hex_str) => {
                let bytes = decode_hex_bytes(&hex_str)?;
                Ok(Some(V::decode(&mut &bytes[..])?))
            }
        }
    }

    /// List up to `count` keys under `prefix` (e.g. a `StorageMap`'s
    /// pallet+item prefix from [`keys::storage_prefix`]) -- used to discover
    /// which entries exist without knowing every key in advance (e.g.
    /// "which epochs have a beacon yet").
    pub async fn storage_keys_paged(
        &self,
        prefix: &[u8],
        count: u32,
        start_after: Option<&[u8]>,
    ) -> Result<Vec<Vec<u8>>, ClientError> {
        let prefix_hex = encode_hex(prefix);
        let start_hex = start_after.map(encode_hex);
        let keys_hex: Vec<String> = self
            .ws
            .request("state_getKeysPaged", rpc_params![prefix_hex, count, start_hex])
            .await?;
        keys_hex.iter().map(|k| decode_hex_bytes(k)).collect()
    }
}
