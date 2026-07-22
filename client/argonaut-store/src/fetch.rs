//! D4 `fetch` (spec §D.10): retrieve a deal's shards from their assigned
//! providers over `/argonaut/retrieval/1`, metered by payment-channel
//! vouchers, then hand the collected shards to `decode` (D1).
//!
//! Scope cuts, documented (matches this session's pattern throughout):
//! - Manifest distribution isn't automated -- the caller supplies the local
//!   `manifest.scale` from encoding (or one obtained out of band), rather
//!   than this fetching it from a provider first. The spec's "obtain
//!   manifest (any provider)" step is a real remaining piece.
//! - `price_per_stride` is an out-of-band-agreed rate (same as the
//!   provider daemon's own `--price-per-stride`), not discovered on-chain
//!   -- nothing in D prices retrieval bandwidth on-chain (see
//!   `storage-provider/src/main.rs`'s doc comment on the same field).
//! - One payment channel per shard fetched, not one per provider reused
//!   across shards -- simplicity over efficiency for this pass.
//! - `RetrievalHandler` (the provider side) does not verify the voucher
//!   signature against a real on-chain identity yet (see its own doc
//!   comment) -- signing here is still done correctly, so nothing here
//!   needs to change when that gap is closed.

use crate::net::{connect_to_provider, read_msg, write_msg};
use argonaut_chain_client::channels::sign_voucher;
use argonaut_chain_client::extrinsic::SignMode;
use argonaut_chain_client::keys::{double_map_key, map_key, storage_prefix, Hasher};
use argonaut_chain_client::ChainClient;
use argonaut_primitives_storage::{
    BlobRoot, ChannelId, DealId, RetrievalRequest, RetrievalResponse, ShardIndex, Voucher,
    RETRIEVAL_ALPN, VOUCHER_STRIDE,
};
use argonaut_runtime::{AccountId, Balance};
use pallet_provider_registry::ProviderInfo;
use pallet_storage_deals::{Assignment, ShardState};
use parity_scale_codec::{Decode, Encode};
use rand_core::{CryptoRng, RngCore};

/// Fetch one shard's full bytes from its assigned, `Activated` provider.
/// Opens a fresh payment channel sized to `price_per_stride * strides`
/// (rounded up), sends one `RetrievalRequest` per stride with an
/// ever-larger cumulative voucher, and stops at the first short read (EOF).
#[allow(clippy::too_many_arguments)]
pub async fn fetch_shard(
    client: &ChainClient,
    deal_id: DealId,
    shard_idx: ShardIndex,
    root: &BlobRoot,
    size: u64,
    price_per_stride: u128,
    mode: SignMode,
    signing_key: &[u8],
    signer_account: &AccountId,
    rng: &mut (impl RngCore + CryptoRng),
) -> anyhow::Result<Vec<u8>> {
    let assignment_key = double_map_key(
        "StorageDeals",
        "ShardAssignments",
        Hasher::Blake2_128Concat,
        &deal_id,
        Hasher::Twox64Concat,
        &shard_idx,
    );
    let assignment: Assignment<AccountId> = client
        .read_storage(&assignment_key)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no assignment for deal {deal_id} shard {shard_idx}"))?;
    anyhow::ensure!(
        assignment.state == ShardState::Activated,
        "shard {shard_idx} of deal {deal_id} is not Activated yet"
    );

    let provider_key =
        map_key("ProviderRegistry", "Providers", Hasher::Blake2_128Concat, &assignment.provider);
    let provider: ProviderInfo<Balance> = client
        .read_storage(&provider_key)
        .await?
        .ok_or_else(|| anyhow::anyhow!("assigned provider not found in registry"))?;

    let strides = size.div_ceil(VOUCHER_STRIDE).max(1);
    let deposit = price_per_stride.saturating_mul(strides as u128);

    let next_channel_id_key = storage_prefix("PaymentChannels", "NextChannelId");
    let channel_id: ChannelId = client.read_storage(&next_channel_id_key).await?.unwrap_or(0);

    client
        .open_channel(
            assignment.provider.clone(),
            deposit,
            10_000, // ttl_blocks -- generous; devnet-scale, not tuned
            mode,
            signing_key,
            signer_account,
            rng,
        )
        .await?;

    let endpoint = crate::net::client_endpoint().await?;
    let conn = connect_to_provider(&endpoint, &provider.iroh_node_id, &provider.relay_hint, RETRIEVAL_ALPN)
        .await?;
    let (mut send, mut recv) = conn.open_bi().await?;

    let mut collected = Vec::with_capacity(size as usize);
    let mut stride_number: u64 = 0;
    while (collected.len() as u64) < size {
        stride_number += 1;
        let cumulative = price_per_stride.saturating_mul(stride_number as u128);
        let voucher = Voucher { channel_id, cumulative };
        let sig = sign_voucher(signing_key, &voucher, rng).map_err(|e| anyhow::anyhow!("{e}"))?;
        let remaining = size - collected.len() as u64;
        let req = RetrievalRequest {
            channel_id,
            root: *root,
            offset: collected.len() as u64,
            length: remaining.min(VOUCHER_STRIDE) as u32,
            voucher,
            sig,
        };
        write_msg(&mut send, &req.encode()).await?;
        let resp_bytes = read_msg(&mut recv, (VOUCHER_STRIDE as usize) + 4096).await?;
        match RetrievalResponse::decode(&mut &resp_bytes[..])? {
            RetrievalResponse::Data(bytes) => {
                let short_read = (bytes.len() as u64) < remaining.min(VOUCHER_STRIDE);
                collected.extend_from_slice(&bytes);
                if short_read {
                    break; // provider hit EOF
                }
            }
            other => anyhow::bail!("retrieval stride {stride_number} rejected: {other:?}"),
        }
    }
    conn.close(0u32.into(), b"done");

    let actual_root = *blake3::hash(&collected).as_bytes();
    anyhow::ensure!(
        &actual_root == root,
        "fetched shard {shard_idx} root mismatch: expected {}, got {}",
        hex::encode(root),
        hex::encode(actual_root)
    );
    Ok(collected)
}
