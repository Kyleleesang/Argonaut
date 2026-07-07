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

/// Polls `prism_chain` for newly confirmed leader levels and turns each into
/// exactly one Substrate block (architecture guide §3.8's bridge pseudocode).
/// `block_import` is a second handle onto the same client used for the import
/// queue (`Arc<FullClient>` is cheap to clone and implements `BlockImport`
/// directly) -- self-authored blocks are imported here directly, never going
/// through the queue/[`PrismBridgeVerifier`] above.
pub async fn run_prism_bridge(
    client: Arc<FullClient>,
    block_import: Arc<FullClient>,
    prism_chain: Arc<Mutex<PrismChain>>,
    prism_db: Arc<PrismDb>,
    k_deep: u64,
) {
    let mut ledger = PrismLedger::new(prism_db);

    loop {
        tokio::time::sleep(Duration::from_millis(250)).await;

        let confirmed = {
            let chain = prism_chain.lock().expect("PrismChain mutex poisoned");
            ledger.advance(&chain, k_deep)
        };

        for level in confirmed {
            let level_no = level.level;
            if let Err(err) =
                build_and_import_block(&client, &block_import, level.level, level.leader, level.txs).await
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
) -> Result<(), Box<dyn std::error::Error>> {
    let info = client.info();
    let parent_hash = info.best_hash;
    let parent_number = info.best_number;

    let mut builder = BlockBuilderBuilder::new(&**client)
        .on_parent_block(parent_hash)
        .with_parent_block_number(parent_number)
        .build()?;

    let mut inherent_data = sp_inherents::InherentData::new();
    sp_timestamp::InherentDataProvider::from_system_time()
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
