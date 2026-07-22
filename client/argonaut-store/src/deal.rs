//! D4 `deal create` (spec §D.10): submit `create_deal`, wait for a provider
//! to `accept_shard` each slot on-chain, then push shard bytes directly to
//! whichever provider actually holds each accepted assignment, over
//! `/argonaut/shard-push/1`.
//!
//! **Live-devnet correction (2026-07-21):** an earlier version of this
//! module pushed bytes immediately after submitting `create_deal`, using
//! the client's own `discover_active_providers` guess as the push target.
//! That's wrong on two counts, both found by actually running it against a
//! live node rather than trusting the code read: (1) `submit_extrinsic` is
//! fire-and-forget (chain-client's own doc), so the deal usually isn't even
//! mined yet when the push fires; (2) `accept_shard` (`pallets/storage-deals`)
//! is first-come-first-served among *any* active provider -- the client has
//! no on-chain say in who ends up holding which `shard_idx`, so pushing to a
//! client-side guess can target the wrong provider even once mining catches
//! up. `protocol.rs`'s `ShardAuthority` check requires an `Accepted`
//! assignment to already exist, so every push always came back
//! `NotAssigned`. Fixed by splitting into [`create_deal`] (submit + poll
//! until mined) and [`push_shards_for_deal`] (poll each shard's
//! `ShardAssignments` entry until `Accepted`, then push to whoever it
//! actually names).
//!
//! Provider selection for the informational `discover_active_providers`
//! upfront check is deliberately simple: take the first `n` `Active`
//! providers found via `state_getKeysPaged`, no ranking by stake/price/
//! faults (the frontend spec's own deal wizard, §7.2, is where real ranking
//! + user override belongs).

use crate::net::{connect_to_provider, read_msg, write_msg};
use argonaut_chain_client::extrinsic::{build_and_sign, SignMode};
use argonaut_chain_client::keys::{double_map_key, map_key, storage_prefix, Hasher};
use argonaut_chain_client::ChainClient;
use argonaut_primitives_storage::{DealId, DealParams, EpochIndex, Manifest, ShardIndex, ShardSpec};
use argonaut_runtime::{self as runtime, AccountId, Balance, RuntimeCall};
use pallet_provider_registry::{ProviderInfo, ProviderStatus};
use pallet_storage_deals::{Assignment, Deal, ShardState};
use parity_scale_codec::{Decode, Encode};
use rand_core::{CryptoRng, RngCore};
use sp_core::bounded::BoundedVec;
use std::time::{Duration, Instant};

/// How long [`create_deal`] and [`push_shards_for_deal`] each poll on-chain
/// state before giving up -- generous because this devnet's Prism PoW
/// mining is irregular (observed 1-8 minutes between confirmed blocks) and
/// `accept_shard` additionally waits on a human/operator running the
/// one-shot `argonaut-provider accept-shard` (D3's daemon deliberately
/// doesn't auto-bid, see its module doc).
const POLL_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const POLL_INTERVAL: Duration = Duration::from_secs(3);

pub struct ChosenProvider {
    pub account: AccountId,
    pub info: ProviderInfo<Balance>,
}

/// Every `Active` provider on-chain, decoded. Simple linear scan via
/// `state_getKeysPaged` -- fine at devnet scale; a real marketplace would
/// want an index.
pub async fn discover_active_providers(
    client: &ChainClient,
    max: u32,
) -> anyhow::Result<Vec<ChosenProvider>> {
    let prefix = storage_prefix("ProviderRegistry", "Providers");
    let keys = client.storage_keys_paged(&prefix, max, None).await?;
    let mut out = Vec::new();
    for key in &keys {
        let Some(info) = client.read_storage::<ProviderInfo<Balance>>(key).await? else { continue };
        if info.status != ProviderStatus::Active {
            continue;
        }
        // Blake2_128Concat key = prefix ++ blake2_128(encoded_key) ++ encoded_key;
        // AccountId32 encodes to exactly 32 bytes, so the account is the
        // last 32 bytes of the full key.
        anyhow::ensure!(key.len() >= 32, "storage key shorter than an AccountId32");
        let account_bytes: [u8; 32] = key[key.len() - 32..].try_into().unwrap();
        out.push(ChosenProvider { account: AccountId::from(account_bytes), info });
    }
    Ok(out)
}

/// Submit `create_deal` and poll until it's actually mined. Needed because
/// `submit_extrinsic` only proves pool-acceptance (chain-client's own doc on
/// [`ChainClient::submit_extrinsic`]) -- callers that immediately act on the
/// returned `deal_id` (e.g. telling providers to `accept_shard`) need the
/// deal to actually exist on-chain first, or every such call fails with
/// `DealNotFound`.
#[allow(clippy::too_many_arguments)]
pub async fn create_deal(
    client: &ChainClient,
    manifest: &Manifest,
    manifest_bytes: &[u8],
    duration_epochs: EpochIndex,
    price_per_shard_epoch: Balance,
    challenge_rate_ppm: u32,
    mode: SignMode,
    signing_key: &[u8],
    signer_account: &AccountId,
    rng: &mut (impl RngCore + CryptoRng),
) -> anyhow::Result<DealId> {
    let n = manifest.shards.len();

    // Informational only: acceptance is first-come-first-served among *any*
    // active provider (`pallet_storage_deals::accept_shard`), so this can't
    // gate deal creation -- it just warns early if the devnet looks empty.
    let providers = discover_active_providers(client, (n as u32) * 4).await?;
    if providers.len() < n {
        eprintln!(
            "warning: only {} active provider(s) registered, deal needs {n} accepted -- \
             creating anyway, waiting for more to come online/accept",
            providers.len()
        );
    }

    let manifest_root = crate::blob_root(manifest_bytes);
    let shards: Vec<ShardSpec> = manifest
        .shards
        .iter()
        .map(|root| ShardSpec { root: *root, size: manifest.shard_size })
        .collect();
    let params = DealParams::<Balance> {
        manifest: manifest_root,
        shards: BoundedVec::try_from(shards)
            .map_err(|_| anyhow::anyhow!("too many shards for MAX_SHARDS"))?,
        k: manifest.k,
        n: manifest.n,
        duration_epochs,
        price_per_shard_epoch,
        challenge_rate_ppm,
    };

    // Best-effort deal_id: the pre-submission value of NextDealId. Race-prone
    // if something else creates a deal in between (fine for a devnet proof;
    // a production client would instead watch for `DealCreated` and read the
    // id back out of the event).
    let next_id_key = storage_prefix("StorageDeals", "NextDealId");
    let deal_id: DealId = client.read_storage(&next_id_key).await?.unwrap_or(0);

    let call: RuntimeCall = runtime::StorageDealsCall::create_deal { params }.into();
    let ctx = client.signing_context(signer_account).await?;
    let xt = build_and_sign(call, mode, &ctx, signing_key, rng)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    client.submit_extrinsic(&xt).await?;

    wait_for_deal_mined(client, deal_id, signer_account).await?;
    Ok(deal_id)
}

async fn wait_for_deal_mined(
    client: &ChainClient,
    deal_id: DealId,
    signer_account: &AccountId,
) -> anyhow::Result<()> {
    let key = map_key("StorageDeals", "Deals", Hasher::Blake2_128Concat, &deal_id);
    let start = Instant::now();
    loop {
        if let Some(deal) = client.read_storage::<Deal<AccountId, Balance>>(&key).await? {
            anyhow::ensure!(
                &deal.client == signer_account,
                "deal {deal_id} exists but belongs to a different client -- \
                 a concurrent create_deal race, re-run"
            );
            return Ok(());
        }
        anyhow::ensure!(
            start.elapsed() < POLL_TIMEOUT,
            "timed out waiting for deal {deal_id} to be mined"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Push every shard once its provider has actually run `accept_shard`
/// on-chain -- see this module's doc for why the push can't happen any
/// earlier, or target a client-side guess rather than the real assignee.
pub async fn push_shards_for_deal(
    client: &ChainClient,
    deal_id: DealId,
    manifest: &Manifest,
    shard_bytes: &[Vec<u8>],
) -> anyhow::Result<()> {
    let n = manifest.shards.len();
    anyhow::ensure!(shard_bytes.len() == n, "have {} shard files, manifest says n={n}", shard_bytes.len());

    let endpoint = crate::net::client_endpoint().await?;
    for (shard_idx, bytes) in shard_bytes.iter().enumerate() {
        let provider = wait_for_shard_accepted(client, deal_id, shard_idx as ShardIndex).await?;
        push_shard_to_provider(&endpoint, &provider, deal_id, shard_idx as ShardIndex, bytes).await?;
    }
    Ok(())
}

async fn wait_for_shard_accepted(
    client: &ChainClient,
    deal_id: DealId,
    shard_idx: ShardIndex,
) -> anyhow::Result<ChosenProvider> {
    let assignment_key = double_map_key(
        "StorageDeals",
        "ShardAssignments",
        Hasher::Blake2_128Concat,
        &deal_id,
        Hasher::Twox64Concat,
        &shard_idx,
    );
    let start = Instant::now();
    let account = loop {
        if let Some(a) = client.read_storage::<Assignment<AccountId>>(&assignment_key).await? {
            if matches!(a.state, ShardState::Accepted | ShardState::Activated) {
                break a.provider;
            }
        }
        anyhow::ensure!(
            start.elapsed() < POLL_TIMEOUT,
            "timed out waiting for shard {shard_idx} of deal {deal_id} to be accepted -- \
             has a provider run `argonaut-provider accept-shard`?"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    };

    let info_key = map_key("ProviderRegistry", "Providers", Hasher::Blake2_128Concat, &account);
    let info = client
        .read_storage::<ProviderInfo<Balance>>(&info_key)
        .await?
        .ok_or_else(|| anyhow::anyhow!("provider accepted shard {shard_idx} but isn't in ProviderRegistry"))?;
    Ok(ChosenProvider { account, info })
}

async fn push_shard_to_provider(
    endpoint: &iroh::Endpoint,
    provider: &ChosenProvider,
    deal_id: DealId,
    shard_idx: ShardIndex,
    bytes: &[u8],
) -> anyhow::Result<()> {
    use argonaut_primitives_storage::{ShardPushRequest, ShardPushResponse};

    let conn = connect_to_provider(
        endpoint,
        &provider.info.iroh_node_id,
        &provider.info.relay_hint,
        argonaut_primitives_storage::SHARD_PUSH_ALPN,
    )
    .await?;
    let (mut send, mut recv) = conn.open_bi().await?;
    let req = ShardPushRequest { deal_id, shard_idx, bytes: bytes.to_vec() };
    write_msg(&mut send, &req.encode()).await?;
    send.finish()?;
    let resp_bytes = read_msg(&mut recv, 4096).await?;
    let resp = ShardPushResponse::decode(&mut &resp_bytes[..])?;
    anyhow::ensure!(
        matches!(resp, ShardPushResponse::Ok),
        "provider rejected shard {shard_idx} for deal {deal_id}: {resp:?}"
    );
    conn.close(0u32.into(), b"done");
    Ok(())
}
