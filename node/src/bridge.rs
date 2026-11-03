//! B3: the Substrate bridge (architecture guide §3.8). Prism runs entirely
//! client-side; this is the only place a confirmed Prism leader level ever
//! turns into a *best/finalized* Substrate block. The bridge chain never
//! reorgs -- every block the bridge itself derives is imported
//! `finalized: true` with `fork_choice: Custom(true)` ("always best"), since
//! Prism's own confirmation rule (`PrismChain::confirmed_leader_at_level`) is
//! the actual consensus; Substrate's fork-choice here is just bookkeeping over
//! an already-decided sequence. This replaces the throwaway B0 PoW chain
//! (`node/src/pow.rs`, now deleted) entirely -- there is no more client-side
//! PoW mining loop driving the Substrate chain, only this bridge.
//!
//! R-1/R-2/R-6: best and finalized move *only* via this bridge. Blocks that
//! arrive over Substrate sync are cached as non-best, non-final side blocks
//! (see [`PrismBridgeVerifier`]) and are promoted only when the local Prism
//! ledger confirms the same level. A node therefore follows the chain only by
//! running Prism; there is no trusted "follow via Substrate sync" mode.
use crate::service::FullClient;
use argonaut_prism_chain::PrismChain;
use argonaut_prism_db::PrismDb;
use argonaut_prism_ledger::{confirmation::ConfirmationParams, PrismLedger};
use argonaut_primitives_prism::H256;
use argonaut_runtime::opaque::{Block, UncheckedExtrinsic};
use parity_scale_codec::{Decode, Encode};
use sc_block_builder::BlockBuilderBuilder;
use sc_client_api::{Finalizer, StorageProvider};
use sc_consensus::{
    import_queue::Verifier, BlockImport, BlockImportParams, ForkChoiceStrategy, ImportResult,
    StateAction, StorageChanges,
};
use sp_blockchain::{BlockStatus, HeaderBackend};
use sp_consensus::BlockOrigin;
use sp_inherents::InherentDataProvider as _;
use sp_runtime::generic::DigestItem;
use sp_runtime::traits::{Block as BlockT, Header as HeaderT, SaturatedConversion};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Must match `pallet_timestamp::Config::MinimumPeriod` in `runtime/src/lib.rs`
/// (`ConstU64<{ SLOT_DURATION / 2 }>`). Duplicated here (rather than adding a
/// `pallet-timestamp` dependency just to read one `Get` impl) -- if the
/// runtime's `MinimumPeriod` ever changes, update this too. (An earlier
/// version of this comment claimed `K_DEEP_CONFIRMATION` was duplicated the
/// same way -- it isn't: `node/src/service.rs` imports
/// `argonaut_prism_ledger::K_DEEP_CONFIRMATION`/its chain-spec-driven value
/// directly, see B-16. That was a stale comment, not a real duplication.)
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
/// Nothing in the runtime reads this back -- it exists purely so a block can
/// be traced back to the Prism leader sequence that produced it.
#[derive(Encode, Decode)]
pub struct LeaderDigest {
    pub level: u64,
    pub leader: H256,
}

/// `ConsensusEngineId` for the leader digest. Must be `PreRuntime`, not
/// `Other`: `Executive::initialize_block`'s `extract_pre_digest` (frame-executive)
/// keeps only `PreRuntime` items from the header it's handed and drops
/// everything else -- found live via a real cross-node bug, not by
/// inspection. Self-authored blocks are imported with
/// `StateAction::ApplyChanges`, which never calls back into the runtime, so
/// an `Other` item survived fine on the originating node. But any *other*
/// node receiving the same block via ordinary Substrate block-sync (e.g.
/// after a restart, or a new node joining) goes through
/// `PrismBridgeVerifier`'s default `StateAction::Execute`, which re-runs
/// `execute_block` end to end -- and `final_checks`' `assert_eq!` on digest
/// item counts trips the instant an `Other` item present in the received
/// header doesn't come back out of `initialize_block`+`finalize()`,
/// panicking (a wasm `unreachable` trap) on every single resync attempt.
/// `PreRuntime` is exactly the variant `extract_pre_digest` preserves, so it
/// survives identically whether the block is being authored or re-verified.
pub const LEADER_DIGEST_ENGINE_ID: &[u8; 4] = b"prsm";

fn leader_digest_item(level: u64, leader: H256) -> DigestItem {
    DigestItem::PreRuntime(*LEADER_DIGEST_ENGINE_ID, LeaderDigest { level, leader }.encode())
}

/// Decodes the single `LeaderDigest` a bridge block must carry. Requires
/// *exactly one* `PreRuntime` item under [`LEADER_DIGEST_ENGINE_ID`] that
/// SCALE-decodes cleanly (and consumes every byte); anything else -- zero
/// items, duplicates, trailing garbage -- is `None`. Shared by the network
/// verifier (R-1) and the bridge's own parent/existing-block checks (R-2) so
/// both apply the same definition of "well-formed".
fn leader_digest_of(header: &<Block as BlockT>::Header) -> Option<LeaderDigest> {
    let mut found: Option<LeaderDigest> = None;
    for item in header.digest().logs() {
        if let DigestItem::PreRuntime(id, data) = item {
            if id == LEADER_DIGEST_ENGINE_ID {
                if found.is_some() {
                    return None;
                }
                let mut input = &data[..];
                let d = LeaderDigest::decode(&mut input).ok()?;
                if !input.is_empty() {
                    return None;
                }
                found = Some(d);
            }
        }
    }
    found
}

/// Verifier for blocks arriving over the network (self-authored blocks are
/// imported directly by [`run_prism_bridge`], bypassing the import queue
/// entirely).
///
/// R-1: a network block must NEVER become best or finalized on its own. The
/// old verifier marked every network block `finalized` + "always best";
/// re-execution (the default `StateAction::Execute`) proves only that the
/// block is a *valid state transition*, not that it is the *Prism-ordered*
/// one, so any peer could author arbitrary extrinsics and have victims adopt
/// them as the canonical chain (and, after a restart, adopt the fake height
/// as the ledger cursor). Now:
///
/// 1. The header must carry exactly one well-formed `prsm` `LeaderDigest`
///    whose `level == header.number` (genesis never comes through the queue,
///    and the bridge's invariant is block number == Prism level). Otherwise
///    `Err`, which makes the import queue penalize the peer -- correct for a
///    malformed block.
/// 2. Otherwise the block is imported as a non-best, non-final side block
///    (`finalized = false`, `Custom(false)`). Honest peers' blocks are just a
///    cache: construction is deterministic, so when the local ledger confirms
///    that level [`run_prism_bridge`] derives the identical hash and promotes
///    the cached block to best+final. A malicious block becomes a dead side
///    fork that Substrate discards as soon as a sibling at that height is
///    finalized.
///
/// Consequence: a node follows the chain only by running Prism. There is no
/// trusted "follow via Substrate sync" mode (any such mode would have to be
/// an explicit, documented opt-in).
pub struct PrismBridgeVerifier;

#[async_trait::async_trait]
impl Verifier<Block> for PrismBridgeVerifier {
    async fn verify(
        &self,
        mut block: BlockImportParams<Block>,
    ) -> Result<BlockImportParams<Block>, String> {
        let number: u64 = (*block.header.number()).into();
        let digest = leader_digest_of(&block.header).ok_or_else(|| {
            format!(
                "block #{number} lacks exactly one well-formed `prsm` LeaderDigest pre-runtime item"
            )
        })?;
        if digest.level != number {
            return Err(format!(
                "block #{number} carries LeaderDigest level {} (must equal its number)",
                digest.level
            ));
        }
        block.finalized = false;
        block.fork_choice = Some(ForkChoiceStrategy::Custom(false));
        Ok(block)
    }
}

/// What the bridge should do for a confirmed level (R-2). Pure so it can be
/// unit-tested without a client.
#[derive(Debug, PartialEq, Eq)]
pub enum BuildDecision {
    /// Derive and import the block for this level.
    Build,
    /// A canonical block for this level with the right leader already exists.
    AlreadyDone,
    /// Stop the bridge: something is inconsistent and building would fork.
    Halt(String),
}

/// R-2: decide whether to build level `level`.
///
/// * `parent_digest_level`: the `LeaderDigest` level of the canonical block at
///   height `level - 1`; `None` if that block is missing or its digest is
///   malformed. For `level - 1 == 0` the caller passes `Some(0)` when genesis
///   exists (genesis carries no digest by construction).
/// * `existing_at_level`: `None` if no canonical block exists at height
///   `level`; `Some(None)` if one exists but has no valid digest;
///   `Some(Some(leader))` otherwise.
/// * `expected_leader`: the leader Prism confirmed for `level`.
///
/// Target height is `level`, parent is the canonical block at `level - 1`
/// (never `best_hash`). After R-1 a canonical block at `level` can only have
/// come from our own bridge, so a mismatch there is a real bug and we must
/// halt rather than build over it.
pub fn decide_build(
    level: u64,
    parent_digest_level: Option<u64>,
    existing_at_level: Option<Option<H256>>,
    expected_leader: H256,
) -> BuildDecision {
    if level == 0 {
        return BuildDecision::Halt("level 0 is genesis and is never built".into());
    }
    match parent_digest_level {
        None => {
            return BuildDecision::Halt(format!(
                "canonical parent at #{} is missing or has no valid LeaderDigest",
                level - 1
            ))
        },
        Some(l) if l != level - 1 => {
            return BuildDecision::Halt(format!(
                "canonical parent at #{} carries LeaderDigest level {l}, expected {}",
                level - 1,
                level - 1
            ))
        },
        Some(_) => {},
    }
    match existing_at_level {
        None => BuildDecision::Build,
        Some(Some(leader)) if leader == expected_leader => BuildDecision::AlreadyDone,
        Some(Some(leader)) => BuildDecision::Halt(format!(
            "canonical block #{level} already exists with leader {leader:?}, but Prism confirmed {expected_leader:?}"
        )),
        Some(None) => BuildDecision::Halt(format!(
            "canonical block #{level} already exists without a valid LeaderDigest"
        )),
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
///
/// B5 pruning: `prune_enabled`/`k_prune` are `--prism-prune`/`--prism-k-prune`
/// (`node/src/cli.rs`). When enabled, `PrismChain::prune` is called here --
/// this loop already owns both `prism_chain` and `ledger.confirmed_tip()` in
/// the same scope `advance` runs in, exactly what `prune`'s
/// `ledger_confirmed_level` parameter needs, keeping `prism-chain` itself
/// free of any `prism-ledger` dependency (same crate-boundary discipline
/// AF-2 established). Only runs on the 5s fallback tick, not on every
/// notify-driven wakeup (which can fire once per confirmed level) -- pruning
/// is cheap but not free, and there's no benefit to running it more often
/// than every few seconds.
pub async fn run_prism_bridge(
    client: Arc<FullClient>,
    block_import: Arc<FullClient>,
    prism_chain: Arc<Mutex<PrismChain>>,
    prism_db: Arc<PrismDb>,
    k_deep: u64,
    confirmation_notify: Arc<tokio::sync::Notify>,
    prune_enabled: bool,
    k_prune: u64,
    confirmation_params: ConfirmationParams,
) {
    // B-7: recover the ledger's cursor from the Substrate chain's own
    // best-block number, not a fresh `PrismLedger::new` -- see
    // `PrismLedger::recover`'s doc comment for why a restarted node must not
    // start replaying confirmed levels from 1 again.
    let best_number: u64 = client.info().best_number.into();
    let mut ledger = PrismLedger::recover(prism_db, best_number);

    loop {
        let woke_by_fallback_tick = tokio::select! {
            _ = confirmation_notify.notified() => false,
            _ = tokio::time::sleep(Duration::from_secs(5)) => true,
        };

        let confirmed = {
            let chain = prism_chain.lock().expect("PrismChain mutex poisoned");
            ledger.advance_with_params(&chain, k_deep, &confirmation_params)
        };

        for level in confirmed {
            let level_no = level.level;
            // R-6: never advance past a level whose import failed. `ledger`
            // has already moved its cursor past this level inside
            // `advance_with_params`, so carrying on after a failure would
            // leave the Substrate height drifting from the Prism level
            // forever. Stop the task instead (it is spawned as an essential
            // task, so the node shuts down loudly); on restart
            // `PrismLedger::recover(best_number)` re-derives from Substrate's
            // best, which is correct because best only ever moves via this
            // bridge (R-1) and the ledger no longer persists a global
            // emitted-tx set.
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
                    "R-6: halting the bridge at confirmed leader level {level_no}: {err}"
                );
                return;
            }
        }

        if prune_enabled && woke_by_fallback_tick {
            let mut chain = prism_chain.lock().expect("PrismChain mutex poisoned");
            chain.prune(k_prune, ledger.confirmed_tip());
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
    // R-2: parent is the canonical block at `level - 1`, never `best_hash`.
    // (Best only moves via this bridge after R-1, but "block number == Prism
    // level" is the invariant everything else relies on, so enforce it
    // explicitly instead of assuming it.) `HeaderBackend::hash(n)` resolves
    // the *canonical* block at height n, so side blocks cached from the
    // network (R-1) are invisible here.
    let parent_number: <<Block as BlockT>::Header as HeaderT>::Number =
        level.saturating_sub(1).saturated_into();
    let parent_hash = client.hash(parent_number)?;
    let parent_digest_level = match parent_hash {
        None => None,
        Some(h) if level == 1 => client.header(h)?.map(|_| 0),
        Some(h) => client.header(h)?.and_then(|hd| leader_digest_of(&hd)).map(|d| d.level),
    };
    let level_number: <<Block as BlockT>::Header as HeaderT>::Number = level.saturated_into();
    let existing_at_level = match client.hash(level_number)? {
        None => None,
        Some(h) => Some(client.header(h)?.and_then(|hd| leader_digest_of(&hd)).map(|d| d.leader)),
    };
    match decide_build(level, parent_digest_level, existing_at_level, leader) {
        BuildDecision::Build => {},
        BuildDecision::AlreadyDone => return Ok(()),
        BuildDecision::Halt(reason) => return Err(reason.into()),
    }
    let parent_hash = parent_hash.expect("decide_build returned Build, so the parent exists");

    let mut builder = BlockBuilderBuilder::new(&**client)
        .on_parent_block(parent_hash)
        .with_parent_block_number(parent_number)
        .with_inherent_digests(sp_runtime::generic::Digest {
            logs: vec![leader_digest_item(level, leader)],
        })
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
        //
        // R-6: skipping is CORRECT and must stay. Replay protection is the
        // runtime's job (nonce + era), not the ledger's: a repeated, stale or
        // out-of-order extrinsic fails `push` identically on every node, so
        // every node derives the same block. (The ledger no longer keeps a
        // global "already emitted" set; marking a tx emitted before it
        // executed was what used to strand accounts and fork a restarted node.)
        let Ok(xt) = UncheckedExtrinsic::from_bytes(&tx) else { continue };
        let _ = builder.push(xt);
    }

    let built = builder.build()?;
    let (header, body) = built.block.deconstruct();

    // R-1: an honest peer's block for this level may already be cached as a
    // non-best side block. Construction is deterministic, so it has the same
    // hash as ours; a plain import would return `AlreadyInChain` and change
    // nothing. `import_existing = true` does NOT fix that here: the client
    // would re-apply our `StorageChanges` and `sc_state_db` rejects
    // re-inserting a not-yet-canonical block (`BlockAlreadyExists`,
    // state-db/src/noncanonical.rs:307). So promote the cached block with
    // `finalize_block` instead, which finalizes it and (since it is not an
    // ancestor of best) moves the head onto it (client.rs:939-959).
    let hash = header.hash();
    if client.status(hash)? == BlockStatus::InChain {
        client.finalize_block(hash, None, true)?;
        return Ok(());
    }

    let mut params = BlockImportParams::new(BlockOrigin::Own, header);
    params.body = Some(body);
    params.finalized = true;
    params.fork_choice = Some(ForkChoiceStrategy::Custom(true));
    params.state_action = StateAction::ApplyChanges(StorageChanges::Changes(built.storage_changes));
    // The leader digest is already in `header` (set via `with_inherent_digests`
    // above, before `build()`) -- see `LEADER_DIGEST_ENGINE_ID`'s doc comment
    // for why it must be baked in at build time, not appended after.

    match block_import.import_block(params).await? {
        ImportResult::Imported(_) => Ok(()),
        other => Err(format!("block import for leader level {level} was not accepted: {other:?}").into()),
    }
}

#[cfg(test)]
mod tests {
    //! AF-4 (fixplan "Open work" §1): the determinism choke-point.
    //!
    //! Two live nodes never share Prism state directly -- each maintains its
    //! own `PrismChain`/`PrismDb`/`PrismLedger` and only agrees with peers by
    //! processing the identical block DAG. The bridge's entire safety
    //! argument (self-authored blocks import via a trusted
    //! `StateAction::ApplyChanges` fast path; every other node re-verifies via
    //! `PrismBridgeVerifier` and the runtime's own state-root check) rests on
    //! this: the leader digest baked into a block's header must be a
    //! deterministic function of the confirmed DAG, so two honest nodes that
    //! received the same blocks always compute the identical digest bytes.
    //! The 2026-07-07 two-node live run demonstrated this empirically; this
    //! is the standing test the fixplan flagged as still missing.
    //!
    //! No mocking: two fully independent `PrismChain`+`PrismDb`+`PrismLedger`
    //! triples (separate tempdirs, no shared state) are fed the identical
    //! sequence of block inserts, then asserted to (a) confirm byte-identical
    //! `ConfirmedLevel`s and (b) produce byte-identical `leader_digest_item`
    //! encodings -- the exact bytes that end up `PreRuntime`-tagged in every
    //! honest node's `BlockImportParams` header.
    use super::*;
    use argonaut_primitives_prism::{block_hash, BlockContent, ChainId, MerkleProof, Params, PrismBlock, SuperHeader};
    use argonaut_prism_ledger::PrismLedger;
    use sp_core::U256;
    use std::sync::Arc;

    fn params() -> Params {
        let mut p = Params::testnet();
        p.m_voter_chains = 4;
        p
    }

    fn header(nonce: u64) -> SuperHeader {
        SuperHeader {
            parents_root: H256::zero(),
            content_root: H256::zero(),
            timestamp_ms: 0,
            nonce,
            difficulty: U256::max_value(),
        }
    }

    fn proposer_block(nonce: u64, parent: H256, tx_refs: Vec<H256>, prop_refs: Vec<H256>) -> PrismBlock {
        PrismBlock {
            header: header(nonce),
            parent,
            content: BlockContent::Proposer { tx_refs, prop_refs },
            parent_proof: MerkleProof::default(),
            content_proof: MerkleProof::default(),
        }
    }

    fn voter_block(nonce: u64, chain: ChainId, voter_parent: H256, votes: Vec<H256>) -> PrismBlock {
        PrismBlock {
            header: header(nonce),
            parent: voter_parent,
            content: BlockContent::Voter { chain, voter_parent, votes },
            parent_proof: MerkleProof::default(),
            content_proof: MerkleProof::default(),
        }
    }

    /// A fresh, fully independent chain (own tempdir-backed `PrismDb`).
    fn build_chain() -> (tempfile::TempDir, PrismChain, Arc<PrismDb>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Arc::new(PrismDb::open(dir.path()).expect("open PrismDb"));
        let chain = PrismChain::with_db(params(), db.clone());
        (dir, chain, db)
    }

    fn vote_from_every_chain(chain: &mut PrismChain, nonce_start: u64, proposer_hash: H256) {
        for c in 0..4u16 {
            let tip = chain.voter_tips[c as usize].0;
            chain.insert_block(voter_block(nonce_start + c as u64, c, tip, vec![proposer_hash]));
        }
    }

    #[test]
    fn two_independent_chains_confirm_byte_identical_leaders_and_digests() {
        let (_dir_a, mut chain_a, db_a) = build_chain();
        let (_dir_b, mut chain_b, db_b) = build_chain();

        // Same `Params` (fixed `chain_spec_genesis`, no randomness) must yield
        // the same proposer genesis on two independently-constructed chains --
        // the precondition for everything below.
        assert_eq!(chain_a.proposer_genesis, chain_b.proposer_genesis);

        // Three proposer levels, each voted-in by every chain -- the IDENTICAL
        // sequence of inserts, in the same order, fed to both chains
        // independently (no shared state between chain_a and chain_b at all).
        let mut parent = chain_a.proposer_genesis;
        let mut nonce = 1u64;
        for _ in 0..3u64 {
            let p = proposer_block(nonce, parent, vec![], vec![]);
            let p_hash = block_hash(&p.header);
            chain_a.insert_block(p.clone());
            chain_b.insert_block(p);
            nonce += 1;

            vote_from_every_chain(&mut chain_a, nonce, p_hash);
            vote_from_every_chain(&mut chain_b, nonce, p_hash);
            nonce += 4;

            parent = p_hash;
        }

        let mut ledger_a = PrismLedger::new(db_a);
        let mut ledger_b = PrismLedger::new(db_b);
        let confirmed_a = ledger_a.advance(&chain_a, 0);
        let confirmed_b = ledger_b.advance(&chain_b, 0);

        assert!(!confirmed_a.is_empty(), "test setup must actually confirm something");
        assert_eq!(confirmed_a.len(), confirmed_b.len());

        for (a, b) in confirmed_a.iter().zip(confirmed_b.iter()) {
            assert_eq!(a.level, b.level);
            assert_eq!(
                a.leader, b.leader,
                "two independent chains disagreed on the confirmed leader at level {}",
                a.level
            );
            assert_eq!(a.timestamp_ms, b.timestamp_ms);
            assert_eq!(a.txs, b.txs);

            // The actual bytes that land in every honest node's block header
            // (the production function, not a re-implementation of it).
            let digest_a = leader_digest_item(a.level, a.leader).encode();
            let digest_b = leader_digest_item(b.level, b.leader).encode();
            assert_eq!(
                digest_a, digest_b,
                "leader digest bytes diverged at level {} -- two honest nodes would fork silently",
                a.level
            );
        }
    }

    #[test]
    fn decide_build_cases() {
        let l = H256::repeat_byte(1);
        let other = H256::repeat_byte(2);
        // Normal: parent digest is level-1, nothing at `level`.
        assert_eq!(decide_build(5, Some(4), None, l), BuildDecision::Build);
        // Level 1 on genesis (caller passes Some(0)).
        assert_eq!(decide_build(1, Some(0), None, l), BuildDecision::Build);
        // Missing/malformed parent.
        assert!(matches!(decide_build(5, None, None, l), BuildDecision::Halt(_)));
        // Parent digest level wrong.
        assert!(matches!(decide_build(5, Some(3), None, l), BuildDecision::Halt(_)));
        assert!(matches!(decide_build(5, Some(5), None, l), BuildDecision::Halt(_)));
        // Existing canonical block with the same leader: nothing to do.
        assert_eq!(decide_build(5, Some(4), Some(Some(l)), l), BuildDecision::AlreadyDone);
        // Existing with a different leader / malformed digest: halt, never build over it.
        assert!(matches!(decide_build(5, Some(4), Some(Some(other)), l), BuildDecision::Halt(_)));
        assert!(matches!(decide_build(5, Some(4), Some(None), l), BuildDecision::Halt(_)));
        // Level 0 is never built.
        assert!(matches!(decide_build(0, Some(0), None, l), BuildDecision::Halt(_)));
        // Parent problems take precedence over an existing block.
        assert!(matches!(decide_build(5, None, Some(Some(l)), l), BuildDecision::Halt(_)));
    }

    #[test]
    fn leader_digest_of_requires_exactly_one_well_formed_item() {
        use sp_runtime::generic::Digest;
        type H = <Block as BlockT>::Header;
        let mk = |logs: Vec<DigestItem>| -> H {
            H::new(
                7u32.into(),
                Default::default(),
                Default::default(),
                Default::default(),
                Digest { logs },
            )
        };
        let l = H256::repeat_byte(9);
        let good = leader_digest_item(7, l);
        let d = leader_digest_of(&mk(vec![good.clone()])).expect("well-formed");
        assert_eq!((d.level, d.leader), (7, l));
        // none
        assert!(leader_digest_of(&mk(vec![])).is_none());
        // duplicated
        assert!(leader_digest_of(&mk(vec![good.clone(), good.clone()])).is_none());
        // wrong engine id is ignored (-> none)
        assert!(leader_digest_of(&mk(vec![DigestItem::PreRuntime(*b"xxxx", vec![0; 40])])).is_none());
        // `Other` item with our payload does not count
        assert!(leader_digest_of(&mk(vec![DigestItem::Other(LeaderDigest { level: 7, leader: l }.encode())])).is_none());
        // trailing garbage
        let mut bytes = LeaderDigest { level: 7, leader: l }.encode();
        bytes.push(0);
        assert!(leader_digest_of(&mk(vec![DigestItem::PreRuntime(*LEADER_DIGEST_ENGINE_ID, bytes)])).is_none());
        // truncated
        assert!(leader_digest_of(&mk(vec![DigestItem::PreRuntime(*LEADER_DIGEST_ENGINE_ID, vec![1, 2, 3])])).is_none());
    }
}
