//! B3: the Substrate bridge (architecture guide §3.8). Prism runs entirely
//! client-side; this is the only place a confirmed Prism leader level ever
//! turns into a Substrate block. The bridge chain never reorgs -- every block
//! is imported `finalized: true` with `fork_choice: Custom(true)` ("always
//! best"), since Prism's own confirmation rule
//! (`PrismChain::confirmed_leader_at_level`) is the actual consensus;
//! Substrate's fork-choice here is just bookkeeping over an already-decided
//! sequence. This replaces the throwaway B0 PoW chain (`node/src/pow.rs`,
//! now deleted) entirely -- there is no more client-side PoW mining loop
//! driving the Substrate chain, only this bridge.
use crate::service::FullClient;
use argonaut_prism_chain::PrismChain;
use argonaut_prism_db::PrismDb;
use argonaut_prism_ledger::PrismLedger;
use argonaut_primitives_prism::H256;
use argonaut_runtime::opaque::{Block, UncheckedExtrinsic};
use parity_scale_codec::{Decode, Encode};
use sc_block_builder::BlockBuilderBuilder;
use sc_client_api::StorageProvider;
use sc_consensus::{
    import_queue::Verifier, BlockImport, BlockImportParams, ForkChoiceStrategy, ImportResult,
    StateAction, StorageChanges,
};
use sp_blockchain::HeaderBackend;
use sp_consensus::BlockOrigin;
use sp_inherents::InherentDataProvider as _;
use sp_runtime::generic::DigestItem;
use sp_runtime::traits::Block as BlockT;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Must match `pallet_timestamp::Config::MinimumPeriod` in `runtime/src/lib.rs`
/// (`ConstU64<{ SLOT_DURATION / 2 }>`). Duplicated here (rather than adding a
/// `pallet-timestamp` dependency just to read one `Get` impl) the same way
/// `K_DEEP_CONFIRMATION` is duplicated between `prism-ledger` and this crate
/// -- if the runtime's `MinimumPeriod` ever changes, update this too.
const TIMESTAMP_MINIMUM_PERIOD_MS: u64 = argonaut_runtime::SLOT_DURATION / 2;

/// Raw storage key for `pallet_timestamp::Now` (`twox_128("Timestamp") ++
/// twox_128("Now")`) -- there's no dedicated runtime API for reading it back,
/// and adding one just for this would be a bigger surface than a raw read.
fn timestamp_now_key() -> sp_core::storage::StorageKey {
    let mut key = sp_io::hashing::twox_128(b"Timestamp").to_vec();
    key.extend(sp_io::hashing::twox_128(b"Now"));
    sp_core::storage::StorageKey(key)
}

/// The previous Substrate block's `pallet_timestamp::Now`, or 0 at genesis.
/// Reading this from chain state (rather than, say, the bridge's own
/// in-memory "last timestamp used") is what keeps it correct across restarts
/// and across nodes with different histories -- it's the one place every
/// honest node's view is guaranteed to agree, since it's already-imported
/// on-chain state.
fn previous_timestamp(client: &FullClient, parent_hash: <Block as BlockT>::Hash) -> u64 {
    client
        .storage(parent_hash, &timestamp_now_key())
        .ok()
        .flatten()
        .and_then(|data| u64::decode(&mut &data.0[..]).ok())
        .unwrap_or(0)
}

/// Embedded in every bridge-built block's digest, for auditability: which
/// confirmed Prism leader level and proposer hash this block corresponds to.
/// Nothing in the runtime reads this back -- `DigestItem::Other` is a plain,
/// unclaimed byte payload -- it exists purely so a block can be traced back
/// to the Prism leader sequence that produced it.
#[derive(Encode, Decode)]
pub struct LeaderDigest {
    pub level: u64,
    pub leader: H256,
}

/// Human-readable marker prefix so the raw digest bytes are recognizable when
/// inspecting a block (e.g. via `state_getStorage`-style raw dumps), even
/// though nothing parses it back out programmatically.
pub const LEADER_DIGEST_PREFIX: &[u8; 4] = b"prsm";

fn leader_digest_item(level: u64, leader: H256) -> DigestItem {
    let mut bytes = LEADER_DIGEST_PREFIX.to_vec();
    bytes.extend(LeaderDigest { level, leader }.encode());
    DigestItem::Other(bytes)
}

/// Verifier for blocks arriving over the network (self-authored blocks are
/// imported directly by [`run_prism_bridge`], bypassing the import queue
/// entirely). Since the bridge chain is a deterministic function of the
/// confirmed Prism DAG and never reorgs, every block that reaches this
/// verifier via normal Substrate block sync is treated the same way a
/// self-authored one is: finalized immediately, always the new best block.
/// The actual safety check is the default `StateAction::Execute` left in
/// place by `BlockImportParams::new` -- the client re-executes the block's
/// extrinsics against the runtime and rejects it on a state-root/extrinsics-
/// root mismatch, the same as any other Substrate block import.
pub struct PrismBridgeVerifier;

#[async_trait::async_trait]
impl Verifier<Block> for PrismBridgeVerifier {
    async fn verify(
        &self,
        mut block: BlockImportParams<Block>,
    ) -> Result<BlockImportParams<Block>, String> {
        block.finalized = true;
        block.fork_choice = Some(ForkChoiceStrategy::Custom(true));
        Ok(block)
    }
}

/// Reacts to `prism_chain` for newly confirmed leader levels and turns each
/// into exactly one Substrate block (architecture guide §3.8's bridge
/// pseudocode). `block_import` is a second handle onto the same client used
/// for the import queue (`Arc<FullClient>` is cheap to clone and implements
/// `BlockImport` directly) -- self-authored blocks are imported here
/// directly, never going through the queue/[`PrismBridgeVerifier`] above.
///
/// AF-1: woken by `confirmation_notify` (signaled by
/// `client/prism-network`'s `PrismNetworkWorker` whenever an accepted
/// block's `may_confirm` comes back true) instead of polling on a fixed
/// timer -- this used to be a plain `sleep(250ms)` loop despite
/// `InsertOutcome::may_confirm` existing specifically to drive this wakeup,
/// which meant every confirmed level paid up to 250ms of pure waiting
/// latency for no reason, on top of needlessly re-locking `prism_chain`
/// every quarter-second whether or not anything had changed. A long
/// fallback tick remains as a safety net (e.g. the very first check, or if
/// a signal is ever somehow missed).
pub async fn run_prism_bridge(
    client: Arc<FullClient>,
    block_import: Arc<FullClient>,
    prism_chain: Arc<Mutex<PrismChain>>,
    prism_db: Arc<PrismDb>,
    k_deep: u64,
    confirmation_notify: Arc<tokio::sync::Notify>,
) {
    let mut ledger = PrismLedger::new(prism_db);

    loop {
        tokio::select! {
            _ = confirmation_notify.notified() => {},
            _ = tokio::time::sleep(Duration::from_secs(5)) => {},
        }

        let confirmed = {
            let chain = prism_chain.lock().expect("PrismChain mutex poisoned");
            ledger.advance(&chain, k_deep)
        };

        for level in confirmed {
            let level_no = level.level;
            if let Err(err) = build_and_import_block(
                &client,
                &block_import,
                level.level,
                level.leader,
                level.txs,
                level.timestamp_ms,
            )
            .await
            {
                log::error!(
                    target: "prism-bridge",
                    "failed to build/import block for confirmed leader level {level_no}: {err}"
                );
            }
        }
    }
}

async fn build_and_import_block(
    client: &Arc<FullClient>,
    block_import: &Arc<FullClient>,
    level: u64,
    leader: H256,
    txs: Vec<Vec<u8>>,
    leader_timestamp_ms: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let info = client.info();
    let parent_hash = info.best_hash;
    let parent_number = info.best_number;

    let mut builder = BlockBuilderBuilder::new(&**client)
        .on_parent_block(parent_hash)
        .with_parent_block_number(parent_number)
        .build()?;

    // Deterministic timestamp (B-1 fix): every honest node that confirms the
    // same leader must derive the identical Substrate block, but
    // `from_system_time()` used the *local* wall clock, so two nodes
    // confirming the same level moments apart produced different block
    // hashes -- found live, never actually caught by the (never-run)
    // byte-identical-chains test the B3 acceptance criterion calls for.
    // `leader_timestamp_ms` is part of the leader block's own header (baked
    // into its hash), so every node agrees on it bit-for-bit. It's clamped
    // forward to respect `pallet_timestamp`'s hard invariant
    // (`now >= previous + MinimumPeriod`, enforced via `assert!` -- a
    // violation panics block production, not just fails gracefully) rather
    // than trusting it could only ever increase on its own: Prism leader
    // timestamps come from independently-mined blocks and aren't guaranteed
    // monotonic across levels.
    let prev_ts = previous_timestamp(client, parent_hash);
    let ts = leader_timestamp_ms.max(prev_ts.saturating_add(TIMESTAMP_MINIMUM_PERIOD_MS));

    let mut inherent_data = sp_inherents::InherentData::new();
    sp_timestamp::InherentDataProvider::new(ts.into())
        .provide_inherent_data(&mut inherent_data)
        .await?;

    for inherent in builder.create_inherents(inherent_data)? {
        builder.push(inherent)?;
    }

    for tx in txs {
        // Two independent reasons a push can fail, both "skip, not fatal" per
        // architecture guide §3.1: the bytes might not even decode as an
        // extrinsic (shouldn't happen -- the miner only ever mines bytes
        // pulled from a live tx pool -- but Prism itself never validates tx
        // content, so a malicious/buggy peer's tx block could contain
        // garbage), or the extrinsic decodes fine but fails application (bad
        // nonce, a double-spend that survived structural dedup, etc).
        let Ok(xt) = UncheckedExtrinsic::from_bytes(&tx) else { continue };
        let _ = builder.push(xt);
    }

    let built = builder.build()?;
    let (header, body) = built.block.deconstruct();

    let mut params = BlockImportParams::new(BlockOrigin::Own, header);
    params.body = Some(body);
    params.finalized = true;
    params.fork_choice = Some(ForkChoiceStrategy::Custom(true));
    params.state_action = StateAction::ApplyChanges(StorageChanges::Changes(built.storage_changes));
    // `Executive::initialize_block` only preserves `PreRuntime` digest items
    // from the header passed into block-building (see `extract_pre_digest`
    // in frame-executive) -- an `Other` item set before building is silently
    // dropped. Post-build digests merged in via `post_digests` (the same
    // mechanism `sc-consensus-manual-seal`'s `ConsensusDataProvider` uses)
    // survive into the actually-imported header.
    params.post_digests.push(leader_digest_item(level, leader));

    match block_import.import_block(params).await? {
        ImportResult::Imported(_) => Ok(()),
        other => Err(format!("block import for leader level {level} was not accepted: {other:?}").into()),
    }
}
