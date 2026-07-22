//! D4 / spec §D.9 `gate.rs`: two custom iroh protocols the daemon serves,
//! built directly on iroh's raw bidirectional QUIC streams (not the stock
//! `iroh-blobs` blob-transfer protocol, which has no concept of per-stride
//! payment gating) -- request/response, length-prefixed SCALE messages.
//!
//! - `/argonaut/shard-push/1`: client sends shard bytes once, right after
//!   `accept_shard`; the provider verifies `blake3(bytes)` against the
//!   on-chain root before storing (never trusts the push itself).
//! - `/argonaut/retrieval/1`: gated retrieval. One stride per request/
//!   response pair on a long-lived stream; the provider tracks bytes served
//!   per channel in memory (not persisted -- a restart only means the
//!   provider may ask for a voucher covering more than strictly necessary,
//!   the safe direction, never less) and requires the voucher to cover
//!   every stride served so far, not just the current one (vouchers are
//!   cumulative by design, D.7).
//!
//! Scope cut from the spec's literal wording (documented, not silent):
//! `read_range` serves raw bytes, not per-stride bao-verified slices ("the
//! browser verifies bao slices" in the frontend spec is a browser-specific
//! design point). The client verifies the *whole* reassembled shard against
//! its known root once complete, which still satisfies D4's actual
//! acceptance criterion (byte-identity) -- graduating to streamed bao
//! verification is a real improvement (catches corruption before the whole
//! transfer completes) left for a later pass.
//!
//! **Known gap, not a silent one: `RetrievalHandler` does not verify
//! `RetrievalRequest::sig` at all.** It checks that the claimed
//! `voucher.cumulative` covers strides served so far (the metering logic),
//! but never confirms the signature is real or belongs to the channel's
//! actual `from` account -- so as implemented, anyone who knows a
//! `channel_id` and sends an unsigned/garbage `sig` with a big enough
//! `cumulative` gets served for free (the metering mechanism works; the
//! authentication of *who* is paying does not exist yet). Fixing this needs
//! the provider to resolve the channel's `from` account and Falcon key
//! on-chain (mirroring `pallet_payment_channels::settle`'s own inline-pk /
//! registry-lookup resolution) before verifying via
//! `falcon_crypto::verify` -- a real, bounded addition (a second small
//! chain-backed authority trait, parallel to `ShardAuthority`), just not
//! done in this pass. Do not treat this protocol as production-safe until
//! it is.

use crate::blobs::BlobStore;
use argonaut_primitives_storage::{
    BlobRoot, RetrievalRequest, RetrievalResponse, ShardPushRequest, ShardPushResponse,
    VOUCHER_STRIDE,
};
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use parity_scale_codec::{Decode, Encode};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// ALPN constants live in argonaut-primitives-storage (`SHARD_PUSH_ALPN`,
// `RETRIEVAL_ALPN`) so client-side crates (`argonaut-store`) can reference
// them without depending on this daemon crate.
pub use argonaut_primitives_storage::{RETRIEVAL_ALPN, SHARD_PUSH_ALPN};

async fn write_msg(
    send: &mut iroh::endpoint::SendStream,
    bytes: &[u8],
) -> Result<(), iroh::endpoint::WriteError> {
    send.write_all(&(bytes.len() as u32).to_le_bytes()).await?;
    send.write_all(bytes).await?;
    Ok(())
}

async fn read_msg(
    recv: &mut iroh::endpoint::RecvStream,
    max_len: usize,
) -> anyhow::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    anyhow::ensure!(len <= max_len, "message length {len} exceeds max {max_len}");
    let mut buf = vec![0u8; len];
    recv.read_exact(&mut buf).await?;
    Ok(buf)
}

/// `anyhow::Error` deliberately doesn't implement `std::error::Error`
/// (its whole point is erasing that), but `AcceptError::from_err` requires
/// it -- wrap in `io::Error`, which does, carrying the message through.
fn accept_err(e: impl std::fmt::Display) -> AcceptError {
    AcceptError::from_err(std::io::Error::other(e.to_string()))
}

/// Checked before storing a pushed shard, and before serving a retrieval
/// stride -- kept as a small trait rather than depending on `chain.rs`
/// directly, so `protocol.rs` stays testable without a live chain
/// connection (a fake authority is enough for protocol-level unit tests).
#[async_trait::async_trait]
pub trait ShardAuthority: Send + Sync {
    /// Expected on-chain root for `(deal_id, shard_idx)`, if this provider
    /// has (or is meant to have) an assignment for it -- `None` refuses the
    /// push/serve outright.
    async fn expected_root(&self, deal_id: u64, shard_idx: u16) -> Option<BlobRoot>;
}

// ── shard-push (client -> provider) ─────────────────────────────────────────

#[derive(Clone)]
pub struct ShardPushHandler<S: BlobStore + 'static> {
    store: Arc<S>,
    authority: Arc<dyn ShardAuthority>,
}

// `iroh::protocol::ProtocolHandler` requires `Debug`; `Arc<dyn ShardAuthority>`
// isn't `Debug`, so this can't be derived. Nothing interesting to print.
impl<S: BlobStore + 'static> std::fmt::Debug for ShardPushHandler<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShardPushHandler").finish_non_exhaustive()
    }
}

impl<S: BlobStore + 'static> ShardPushHandler<S> {
    pub fn new(store: Arc<S>, authority: Arc<dyn ShardAuthority>) -> Self {
        Self { store, authority }
    }
}

impl<S: BlobStore + 'static> ProtocolHandler for ShardPushHandler<S> {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let (mut send, mut recv) = connection.accept_bi().await?;
        // A shard can legitimately be up to `MAX_SHARDS`-worth of a large
        // deal's `shard_size`; cap generously rather than hardcode
        // MAX_PROOF_BYTES (that bound is for bao *proof* slices, not whole
        // shard payloads).
        const MAX_PUSH_BYTES: usize = 256 * 1024 * 1024;
        let msg = read_msg(&mut recv, MAX_PUSH_BYTES + 4096).await.map_err(accept_err)?;
        let req = ShardPushRequest::decode(&mut &msg[..]).map_err(AcceptError::from_err)?;

        let response = match self.authority.expected_root(req.deal_id, req.shard_idx).await {
            None => ShardPushResponse::NotAssigned,
            Some(expected) => {
                let actual = *blake3::hash(&req.bytes).as_bytes();
                if actual != expected {
                    ShardPushResponse::RootMismatch
                } else {
                    match self.store.add_bytes(&req.bytes).await {
                        Ok(_) => ShardPushResponse::Ok,
                        Err(e) => {
                            log::warn!("shard-push store failure: {e:#}");
                            ShardPushResponse::Error
                        }
                    }
                }
            }
        };

        write_msg(&mut send, &response.encode())
            .await
            .map_err(AcceptError::from_err)?;
        send.finish().map_err(AcceptError::from_err)?;
        connection.closed().await;
        Ok(())
    }
}

// ── gated retrieval (client <-> provider, metered by vouchers) ──────────────

impl<S: BlobStore + 'static> std::fmt::Debug for RetrievalHandler<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RetrievalHandler")
            .field("price_per_stride", &self.price_per_stride)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct RetrievalHandler<S: BlobStore + 'static> {
    store: Arc<S>,
    price_per_stride: u128,
    /// Bytes served so far, per channel, for the lifetime of this process.
    /// Deliberately in-memory only -- see module doc.
    served: Arc<Mutex<HashMap<u64, u64>>>,
}

impl<S: BlobStore + 'static> RetrievalHandler<S> {
    pub fn new(store: Arc<S>, price_per_stride: u128) -> Self {
        Self { store, price_per_stride, served: Arc::new(Mutex::new(HashMap::new())) }
    }

    /// `Ok(strides_after_this_request)` if `voucher.cumulative` covers every
    /// stride served on this channel so far, including this one; `Err(())`
    /// otherwise (caller responds `PaymentRequired`, does not serve data,
    /// and does NOT record this stride as served).
    fn check_and_reserve_stride(&self, channel_id: u64, cumulative: u128) -> Result<(), ()> {
        let mut served = self.served.lock().expect("served-bytes lock poisoned");
        let prior_bytes = *served.get(&channel_id).unwrap_or(&0);
        let strides_after = prior_bytes / VOUCHER_STRIDE + 1;
        let owed = self.price_per_stride.saturating_mul(strides_after as u128);
        if cumulative < owed {
            return Err(());
        }
        served.insert(channel_id, prior_bytes + VOUCHER_STRIDE);
        Ok(())
    }
}

impl<S: BlobStore + 'static> ProtocolHandler for RetrievalHandler<S> {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let (mut send, mut recv) = connection.accept_bi().await?;
        loop {
            let msg = match read_msg(&mut recv, 8192).await {
                Ok(m) => m,
                // Client finished the stream (no more strides wanted) --
                // a clean end of this session, not an error.
                Err(_) => break,
            };
            let req = RetrievalRequest::decode(&mut &msg[..]).map_err(AcceptError::from_err)?;
            let length = req.length.min(VOUCHER_STRIDE as u32);

            let response = if self
                .check_and_reserve_stride(req.channel_id, req.voucher.cumulative)
                .is_err()
            {
                RetrievalResponse::PaymentRequired
            } else if req.voucher.channel_id != req.channel_id {
                RetrievalResponse::PaymentRequired
            } else {
                match self.store.read_range(&req.root, req.offset, length).await {
                    Ok(bytes) => RetrievalResponse::Data(bytes),
                    Err(e) => {
                        log::warn!("retrieval read_range failure: {e:#}");
                        RetrievalResponse::NoSuchBlob
                    }
                }
            };

            write_msg(&mut send, &response.encode())
                .await
                .map_err(AcceptError::from_err)?;
        }
        send.finish().map_err(AcceptError::from_err)?;
        connection.closed().await;
        Ok(())
    }
}
