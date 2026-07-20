//! `chain_subscribeFinalizedHeads` (D.11) -- the daemon's block ticker.
//! Because Prism-backed blocks import `finalized: true` and never reorg
//! (guide §3.1), this is also the "new best head" stream; there is no
//! separate best-vs-finalized split to reconcile.

use crate::ClientError;
use futures::Stream;
use jsonrpsee::core::client::{Subscription, SubscriptionClientT};
use jsonrpsee::rpc_params;
use jsonrpsee::ws_client::WsClient;
use serde::Deserialize;

/// Just the field callers of this crate actually need. `number` arrives as a
/// `0x`-prefixed hex string in the raw RPC payload (Substrate header JSON
/// convention) and is decoded to a plain `u64` here. The header notification
/// itself carries no hash (that's not part of a Substrate header's own JSON
/// encoding) -- callers that need the hash for a given number should call
/// [`crate::ChainClient::finalized_head`] or `chain_getBlockHash` directly;
/// deliberately not duplicated here to avoid a second, easy-to-desync source
/// of truth for "what does finalized mean right now."
#[derive(Debug, Clone)]
pub struct FinalizedHead {
    pub number: u64,
}

#[derive(Deserialize)]
struct RawHeader {
    number: String,
}

/// Opens `chain_subscribeFinalizedHeads` and returns a `Stream` of decoded
/// headers. The daemon's responder loop (D.9) drives its epoch-boundary
/// checks off this.
pub async fn finalized_heads(
    ws: &WsClient,
) -> Result<impl Stream<Item = Result<FinalizedHead, ClientError>> + '_, ClientError> {
    let sub: Subscription<RawHeader> = ws
        .subscribe(
            "chain_subscribeFinalizedHeads",
            rpc_params![],
            "chain_unsubscribeFinalizedHeads",
        )
        .await?;

    Ok(futures::StreamExt::map(sub, |item| {
        let raw = item.map_err(ClientError::SubscriptionDecode)?;
        let number = u64::from_str_radix(raw.number.trim_start_matches("0x"), 16)
            .map_err(|e| ClientError::BadResponse {
                method: "chain_subscribeFinalizedHeads",
                detail: e.to_string(),
            })?;
        Ok(FinalizedHead { number })
    }))
}
