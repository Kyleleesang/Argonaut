//! Prism DAG state machine — port of prism-rust `blockchain/` module.
//!
//! All consensus logic lives here. The Substrate runtime is only called via
//! `prism-ledger` after a leader is confirmed (ε-final).
//!
//! # Vote rollback
//! Each voter chain is a Nakamoto-style longest-chain: a new voter block may
//! extend the current tip, or start/extend a competing fork. When a competing
//! fork becomes longer than the current tip, the chain re-orgs: votes cast by
//! blocks on the abandoned segment are unwound, and votes cast by blocks on the
//! newly-adopted segment are (re-)applied, walking back to their lowest common
//! ancestor (LCA). `recount_votes` independently recomputes vote totals by
//! replaying each chain's current canonical history from scratch; it must
//! always agree with the incrementally-maintained totals in `votes`.
use argonaut_primitives_prism::{
    block_hash, BlockContent, ChainId, DifficultyRetarget, Params, PrismBlock, H256,
};
use argonaut_prism_db::{columns, BlockStatus, PrismDb};
use parity_scale_codec::{Decode, Encode};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;

/// B-13: caps `PrismChain::orphans`. Without this, a PoW-valid block whose
/// parent/refs never arrive sits in memory forever -- the attacker still
/// pays real PoW per entry (not a cheap DoS), but it's unbounded and
/// permanent, a slow memory leak on any long-lived node. `orphan_order`
/// (insertion order) is what lets eviction be O(1) instead of scanning the
/// whole map for "oldest".
const MAX_ORPHANS: usize = 10_000;

/// Per-proposer votes, one per voter chain that has cast one, keyed by chain
/// id with the *depth* (on that chain) of the block that cast it. Depth is
/// needed for stage-1 "conservative k-deep" confirmation (architecture guide
/// §3.8): a vote only counts toward confirmation once its casting block is at
/// least `k` blocks behind its chain's current tip.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VoteInfo {
    pub per_chain: HashMap<u16, u64>,
}

impl VoteInfo {
    pub fn total(&self) -> u64 {
        self.per_chain.len() as u64
    }
}

/// Outcome of inserting a block — drives network requests and bridge wakeups.
#[derive(Debug, Default)]
pub struct InsertOutcome {
    /// Hashes of blocks we need to fetch from peers (missing parents or refs).
    pub missing: Vec<H256>,
    /// True if new leader levels may now be confirmable by prism-ledger.
    pub may_confirm: bool,
}

/// Lightweight per-voter-block metadata kept for fast tree traversal
/// (fork-choice, LCA search, vote replay) without needing a DB round-trip.
#[derive(Clone, Debug, Encode, Decode)]
struct VoterMeta {
    parent: H256,
    depth: u64,
    /// Proposer hashes this block voted on (copied from its `BlockContent`).
    votes: Vec<H256>,
}

/// Lightweight per-proposer-block metadata.
#[derive(Clone, Debug, Encode, Decode)]
struct ProposerMeta {
    level: u64,
}

/// The Prism chain state machine.
///
/// Owns fast in-memory indices (voter/proposer metadata, vote totals, orphan
/// pool) needed for DAG traversal. A `PrismDb` handle is optional: attach one
/// with [`PrismChain::with_db`] for durable full-block storage; without one,
/// the state machine runs entirely in memory (used by its own unit tests, and
/// suitable for any offline scripted-sequence testing).
pub struct PrismChain {
    pub params: Params,
    /// B-5: per voter chain `i`'s genesis hash (`genesis_hash(chain_spec_genesis, i)`),
    /// computed once in `new()`. Not a real mined block -- seeded directly
    /// into `voter_meta` so parent lookups for the first real block on each
    /// chain resolve without special-casing, same role `genesis_sentinel()`
    /// used to serve, but domain-separated per chain and per network.
    pub voter_genesis: Vec<H256>,
    /// B-5: the proposer tree's genesis hash (`genesis_hash(chain_spec_genesis, m)`).
    pub proposer_genesis: H256,
    pub proposer_best_level: u64,
    /// Per voter chain: (tip_hash, tip_depth).
    pub voter_tips: Vec<(H256, u64)>,
    /// Tx blocks not yet referenced by any proposer block.
    pub unreferenced_tx_blocks: HashSet<H256>,
    /// Proposer blocks not yet referenced by a deeper proposer.
    pub unreferenced_proposers: HashSet<H256>,
    /// Every tx block hash ever inserted (unlike `unreferenced_tx_blocks`, never
    /// shrinks) -- used to check whether a proposer's `tx_refs` are known.
    known_tx_blocks: HashSet<H256>,
    /// Per voter chain: level → first-seen proposer hash at that level that
    /// this chain has not yet voted on.
    pub unvoted_levels: Vec<BTreeMap<u64, H256>>,
    /// level → the canonical (first-seen) proposer at that level. Unlike
    /// `unvoted_levels`, entries here are never removed once a chain votes --
    /// needed to look up "the proposer at level ℓ" after all chains have
    /// already voted on it, for confirmation queries.
    pub level_proposer: BTreeMap<u64, H256>,
    /// Proposer → accumulated vote count.
    pub votes: HashMap<H256, VoteInfo>,
    /// Blocks waiting on missing parent or refs (hash → block).
    pub orphans: HashMap<H256, PrismBlock>,
    /// B-13: `orphans`' insertion order, for O(1) oldest-first eviction once
    /// `orphans.len() >= MAX_ORPHANS`. Kept in sync with `orphans` at every
    /// insert (`insert_orphan`) and every removal (`drain_orphans`).
    orphan_order: VecDeque<H256>,
    voter_meta: HashMap<H256, VoterMeta>,
    proposer_meta: HashMap<H256, ProposerMeta>,
    db: Option<Arc<PrismDb>>,
    /// B-2: rolling-window difficulty retarget, fed one timestamp per
    /// accepted block (any content type -- see `observe_for_retarget`).
    retarget: DifficultyRetarget,
    /// B-7: set for the duration of `with_db`'s startup replay. Suppresses
    /// `observe_for_retarget` (replay order isn't guaranteed to match live
    /// arrival order, so re-feeding the rolling window from replay could
    /// clobber the just-restored `CURRENT_DIFFICULTY_TARGET` with a value
    /// computed from a different window than actually occurred -- the
    /// restored target is authoritative; the window simply refills from
    /// scratch as new blocks arrive post-restart) and re-persisting blocks
    /// that are, by construction, already the ones being read back from `db`.
    replaying: bool,
    /// B5 pruning prerequisite: counts accepted (non-replay) blocks since
    /// the last `ChainCheckpoint` write, to drive `CHECKPOINT_INTERVAL`.
    blocks_since_checkpoint: u64,
    /// B5 pruning: the highest confirmed level whose proposer/tx-block
    /// bodies have already been swept by `prune`. Not persisted in
    /// `ChainCheckpoint` -- restarting at 0 just makes the next `prune`
    /// call re-walk (and no-op delete, since the bodies are already gone)
    /// the whole confirmed history once more, a bounded one-time cost, not
    /// a correctness gap.
    last_body_pruned_level: u64,
}

/// B5 pruning prerequisite: a point-in-time snapshot of every in-memory DAG
/// index, so `PrismChain::with_db` can restart in O(live window) instead of
/// O(total historical blocks) -- and, once pruning actually deletes old
/// blocks from `BLOCKS`, so restart is possible *at all* past the pruned
/// frontier (full replay would otherwise try to re-derive state from bodies
/// that no longer exist).
///
/// SCALE-encoded as one blob under a single `columns::CHAIN_CHECKPOINT` key
/// (not a dedicated CF per field, unlike the removed 13-CF prism-rust
/// schema -- see `prism-db::columns`'s module doc for why). `HashMap`/
/// `HashSet` fields are represented as `Vec` pairs, matching the same
/// convention `client/prism-network`'s wire messages already use for
/// sending `PrismChain` collections over the network (`SyncResponse::Status`).
///
/// Deliberately excludes `orphans`: an orphan was never persisted to
/// `BLOCKS` in the first place (see `persist`'s doc comment on the
/// accepted-only write discipline), so it wouldn't survive a restart even
/// before this checkpoint existed -- B-6's backfill re-fetches whatever a
/// restarted node is still missing.
#[derive(Clone, Debug, Default, Encode, Decode)]
struct ChainCheckpoint {
    version: u32,
    voter_tips: Vec<(H256, u64)>,
    proposer_best_level: u64,
    voter_meta: Vec<(H256, VoterMeta)>,
    proposer_meta: Vec<(H256, ProposerMeta)>,
    votes: Vec<(H256, Vec<(u16, u64)>)>,
    unvoted_levels: Vec<Vec<(u64, H256)>>,
    level_proposer: Vec<(u64, H256)>,
    unreferenced_tx_blocks: Vec<H256>,
    unreferenced_proposers: Vec<H256>,
    known_tx_blocks: Vec<H256>,
}

const CHAIN_CHECKPOINT_VERSION: u32 = 1;

/// How many accepted (non-replay) blocks pass between automatic checkpoints.
/// The checkpoint is O(total live-window state), so writing it on every
/// single insert would be quadratic write amplification as the DAG grows;
/// a node that crashes between checkpoints just replays a bounded number
/// (`< CHECKPOINT_INTERVAL`) of extra blocks from `BLOCKS` on restart via
/// `with_db`'s `has_block`-gated replay loop, not a correctness issue.
const CHECKPOINT_INTERVAL: u64 = 256;

impl PrismChain {
    /// Pure in-memory chain state (no durable storage). Suitable for tests.
    pub fn new(params: Params) -> Self {
        let m = params.m_voter_chains as usize;
        // B-5: distinct, domain-separated genesis per voter chain (slot
        // `0..m-1`) and the proposer tree (slot `m`) -- replaces the single
        // shared `H256::zero()` sentinel every chain and every network used
        // to root at, which gave no network domain separation (a block
        // mined against network A's genesis was structurally valid
        // ancestry on network B) and no chain-slot separation at the
        // genesis boundary.
        let voter_genesis: Vec<H256> = (0..m as u16)
            .map(|i| argonaut_primitives_prism::genesis_hash(&params.chain_spec_genesis, i))
            .collect();
        let proposer_genesis =
            argonaut_primitives_prism::genesis_hash(&params.chain_spec_genesis, m as u16);

        let mut voter_meta = HashMap::new();
        for &g in &voter_genesis {
            voter_meta.insert(g, VoterMeta { parent: g, depth: 0, votes: Vec::new() });
        }

        let mut proposer_meta = HashMap::new();
        proposer_meta.insert(proposer_genesis, ProposerMeta { level: 0 });

        let voter_tips: Vec<(H256, u64)> = voter_genesis.iter().map(|&g| (g, 0)).collect();

        PrismChain {
            params,
            voter_genesis,
            proposer_genesis,
            proposer_best_level: 0,
            voter_tips,
            unreferenced_tx_blocks: HashSet::new(),
            unreferenced_proposers: HashSet::new(),
            known_tx_blocks: HashSet::new(),
            unvoted_levels: vec![BTreeMap::new(); m],
            level_proposer: BTreeMap::new(),
            votes: HashMap::new(),
            orphans: HashMap::new(),
            orphan_order: VecDeque::new(),
            voter_meta,
            proposer_meta,
            db: None,
            retarget: DifficultyRetarget::new(),
            replaying: false,
            blocks_since_checkpoint: 0,
            last_body_pruned_level: 0,
        }
    }

    /// Same as [`PrismChain::new`], but persists every accepted block's full
    /// body to `db` as it's inserted (best-effort: a DB write failure does not
    /// abort insertion, since in-memory state is still consistent either way).
    /// Also restores the last-persisted difficulty target (B-2), if any, so a
    /// restart resumes from wherever retargeting last left off instead of
    /// `params`'s initial guess every time.
    ///
    /// B-7: replays previously-accepted blocks from `db` to rebuild the
    /// in-memory DAG indices (`proposer_meta`, `voter_meta`, `votes`,
    /// `voter_tips`, ...) that `PrismChain::new` alone leaves empty --
    /// without this, a restarted node forgets its entire DAG and starts
    /// mining/voting from genesis again while the bridge (`node/src/bridge.rs`)
    /// still believes those levels were never confirmed, silently
    /// re-deriving and re-importing Substrate blocks that already exist.
    ///
    /// B5 pruning prerequisite: loads the last `ChainCheckpoint` first (if
    /// any), then only replays blocks `has_block` doesn't already recognize
    /// -- both so restart is O(blocks since the last checkpoint) instead of
    /// O(total history), and so it still works once pruning starts deleting
    /// old bodies out of `BLOCKS` (full unconditional replay would try to
    /// re-derive state from bodies that no longer exist). Feeding order
    /// still doesn't matter: every dependency of a stored block is itself
    /// guaranteed to be in the stored set (or already covered by the
    /// checkpoint), and `insert_block`'s orphan-cascade (`drain_orphans`,
    /// called after every acceptance, recursively) resolves the remainder
    /// to a fixed point regardless of the order it's fed in -- the same
    /// argument as before, just starting from a non-empty base state.
    ///
    /// B-5: hard-errors if `db` was already opened once under a different
    /// `params.chain_spec_genesis` -- every stored block's ancestry is
    /// derived from that value via per-chain genesis hashes, so a mismatch
    /// means every parent hash in `db` would silently fail to resolve
    /// against the new genesis set, producing a DB full of unreachable
    /// orphans rather than an obvious error. Devnet answer: purge and
    /// remine (`docs/IMPLEMENTATION_AB_FIXPLAN.md`, "B-5" section) --
    /// there's no migration path for a genesis change.
    pub fn with_db(mut params: Params, db: Arc<PrismDb>) -> Self {
        match db.get_h256(columns::CHAIN_SPEC_GENESIS, b"current") {
            Ok(Some(stored)) => assert_eq!(
                stored, params.chain_spec_genesis,
                "PrismDb was created for chain_spec_genesis {stored:?}, but this node is running \
                 with chain_spec_genesis {:?} -- every stored block's ancestry is unrecoverable \
                 under a different genesis; purge and remine this PrismDb",
                params.chain_spec_genesis,
            ),
            Ok(None) => {
                let _ = db.put_h256(columns::CHAIN_SPEC_GENESIS, b"current", &params.chain_spec_genesis);
            }
            Err(_) => {}
        }
        if let Ok(Some(target)) = db.get_u256(columns::CURRENT_DIFFICULTY_TARGET, b"current") {
            params.set_combined_target(target);
        }
        let mut chain = Self::new(params);
        chain.db = Some(db.clone());
        chain.replaying = true;

        if let Some(checkpoint) = db
            .get_bytes(columns::CHAIN_CHECKPOINT, b"current")
            .ok()
            .flatten()
            .and_then(|bytes| ChainCheckpoint::decode(&mut &bytes[..]).ok())
        {
            chain.load_checkpoint(checkpoint);
        }
        for block in db.iter_blocks() {
            let hash = block_hash(&block.header);
            if !chain.has_block(&hash) {
                chain.insert_block(block);
            }
        }
        chain.replaying = false;
        chain
    }

    fn to_checkpoint(&self) -> ChainCheckpoint {
        ChainCheckpoint {
            version: CHAIN_CHECKPOINT_VERSION,
            voter_tips: self.voter_tips.clone(),
            proposer_best_level: self.proposer_best_level,
            voter_meta: self.voter_meta.iter().map(|(h, m)| (*h, m.clone())).collect(),
            proposer_meta: self.proposer_meta.iter().map(|(h, m)| (*h, m.clone())).collect(),
            votes: self
                .votes
                .iter()
                .map(|(h, v)| (*h, v.per_chain.iter().map(|(&c, &d)| (c, d)).collect()))
                .collect(),
            unvoted_levels: self
                .unvoted_levels
                .iter()
                .map(|m| m.iter().map(|(&l, &h)| (l, h)).collect())
                .collect(),
            level_proposer: self.level_proposer.iter().map(|(&l, &h)| (l, h)).collect(),
            unreferenced_tx_blocks: self.unreferenced_tx_blocks.iter().copied().collect(),
            unreferenced_proposers: self.unreferenced_proposers.iter().copied().collect(),
            known_tx_blocks: self.known_tx_blocks.iter().copied().collect(),
        }
    }

    fn load_checkpoint(&mut self, cp: ChainCheckpoint) {
        self.voter_tips = cp.voter_tips;
        self.proposer_best_level = cp.proposer_best_level;
        self.voter_meta = cp.voter_meta.into_iter().collect();
        self.proposer_meta = cp.proposer_meta.into_iter().collect();
        self.votes = cp
            .votes
            .into_iter()
            .map(|(h, per_chain)| (h, VoteInfo { per_chain: per_chain.into_iter().collect() }))
            .collect();
        self.unvoted_levels = cp.unvoted_levels.into_iter().map(|v| v.into_iter().collect()).collect();
        self.level_proposer = cp.level_proposer.into_iter().collect();
        self.unreferenced_tx_blocks = cp.unreferenced_tx_blocks.into_iter().collect();
        self.unreferenced_proposers = cp.unreferenced_proposers.into_iter().collect();
        self.known_tx_blocks = cp.known_tx_blocks.into_iter().collect();
    }

    /// Write a fresh `ChainCheckpoint` to `db` now, unconditionally. Called
    /// periodically (`maybe_checkpoint`) and after every `prune`; also safe
    /// (cheap relative to a crash-recovery replay) to call from the node on
    /// clean shutdown, though no caller does that yet -- a purely
    /// perf-motivated follow-up, not a correctness gap, since an unclean
    /// shutdown just replays up to `CHECKPOINT_INTERVAL` extra blocks.
    pub fn checkpoint_now(&mut self) {
        self.blocks_since_checkpoint = 0;
        if let Some(db) = &self.db {
            let cp = self.to_checkpoint();
            let _ = db.put_bytes(columns::CHAIN_CHECKPOINT, b"current", &cp.encode());
        }
    }

    /// Called from every accepted-block arm of `insert_block` (mirroring
    /// `observe_for_retarget`'s placement) -- checkpoints every
    /// `CHECKPOINT_INTERVAL` blocks. Suppressed during replay for the same
    /// reason `observe_for_retarget` is: replay order doesn't match live
    /// arrival order, and replay already starts from (at worst) a slightly
    /// stale checkpoint that's about to be caught up to and re-written once
    /// live traffic resumes.
    fn maybe_checkpoint(&mut self) {
        if self.replaying {
            return;
        }
        self.blocks_since_checkpoint += 1;
        if self.blocks_since_checkpoint >= CHECKPOINT_INTERVAL {
            self.checkpoint_now();
        }
    }

    /// Feed a newly-*accepted* block's timestamp to the difficulty retarget
    /// (B-2). Must be called exactly once per accepted block -- not from an
    /// orphaning path, and not for a block that's about to be orphaned,
    /// since `drain_orphans` re-runs `insert_block` for a block once its
    /// deps arrive, which would double-count it here if this were called
    /// unconditionally at `insert_block`'s entry instead of from each
    /// content-type's "accepted" arm.
    fn observe_for_retarget(&mut self, timestamp_ms: u64) {
        if self.replaying {
            return;
        }
        // B-11: off by default -- see `Params::retarget_enabled`'s doc
        // comment for why per-node retargeting is not consensus-safe.
        if !self.params.retarget_enabled {
            return;
        }
        if let Some(new_target) =
            self.retarget.observe(timestamp_ms, self.params.target_interval_ms, self.params.difficulty)
        {
            self.params.set_combined_target(new_target);
            if let Some(db) = &self.db {
                let _ = db.put_u256(columns::CURRENT_DIFFICULTY_TARGET, b"current", &new_target);
            }
        }
    }

    /// B-13: insert into `orphans`, evicting the single oldest entry first if
    /// already at `MAX_ORPHANS`. A block PoW-valid enough to reach here but
    /// evicted is simply lost (its sender may re-gossip it, or it resolves if
    /// its parent/refs arrive before it's evicted) -- acceptable: the cap
    /// exists precisely so no single unresolvable chain of orphans can grow
    /// without bound.
    fn insert_orphan(&mut self, hash: H256, block: PrismBlock) {
        if self.orphans.len() >= MAX_ORPHANS {
            if let Some(oldest) = self.orphan_order.pop_front() {
                self.orphans.remove(&oldest);
            }
        }
        self.orphans.insert(hash, block);
        self.orphan_order.push_back(hash);
    }

    /// Main entry point — idempotent block insertion.
    ///
    /// Returns missing refs (to fetch from peers) and whether confirmation
    /// may have advanced (to wake prism-ledger).
    ///
    /// Always re-drains the orphan pool after every insertion to avoid
    /// dependency-resolution deadlocks.
    pub fn insert_block(&mut self, block: PrismBlock) -> InsertOutcome {
        let hash = block_hash(&block.header);
        let timestamp_ms = block.header.timestamp_ms;
        let mut outcome = InsertOutcome::default();

        match &block.content {
            BlockContent::Transaction { .. } => {
                self.persist(&block);
                self.known_tx_blocks.insert(hash);
                self.unreferenced_tx_blocks.insert(hash);
                self.observe_for_retarget(timestamp_ms);
                self.maybe_checkpoint();
            }

            BlockContent::Proposer { tx_refs, prop_refs } => {
                for r in tx_refs {
                    if !self.known_tx_blocks.contains(r) {
                        outcome.missing.push(*r);
                    }
                }
                for r in prop_refs {
                    if !self.proposer_meta.contains_key(r) {
                        outcome.missing.push(*r);
                    }
                }
                if !self.proposer_meta.contains_key(&block.parent) {
                    outcome.missing.push(block.parent);
                }

                if outcome.missing.is_empty() {
                    self.insert_proposer(hash, block.parent);
                    self.persist(&block);
                    for r in tx_refs {
                        self.unreferenced_tx_blocks.remove(r);
                    }
                    for r in prop_refs {
                        self.unreferenced_proposers.remove(r);
                    }
                    self.unreferenced_proposers.insert(hash);
                    outcome.may_confirm = true;
                    self.observe_for_retarget(timestamp_ms);
                    self.maybe_checkpoint();
                } else {
                    self.insert_orphan(hash, block);
                }
            }

            BlockContent::Voter { chain, voter_parent, votes } => {
                let chain_idx = *chain as usize;
                if chain_idx >= self.voter_tips.len() {
                    // Structurally invalid (should have been rejected by
                    // `validate_block` already); ignore defensively.
                    return outcome;
                }
                // B-5: per-chain geneses make this enforceable at the
                // genesis boundary for the first time -- before, every
                // chain shared one sentinel, so there was no way to tell a
                // block genuinely starting chain `chain_idx` from one
                // wrongly (or maliciously) parented onto a *different*
                // chain's genesis. `voter_parent` matching *some* chain's
                // genesis but not `chain_idx`'s own is unambiguous: reject,
                // don't orphan -- it will never resolve into a legitimate
                // ancestor of this chain.
                if self.voter_genesis.contains(voter_parent) && *voter_parent != self.voter_genesis[chain_idx] {
                    return outcome;
                }
                if !self.voter_meta.contains_key(voter_parent) {
                    if self.is_pruned(voter_parent) {
                        // The parent is gone for good (B5 pruning), not
                        // merely not-yet-received -- orphaning this block
                        // would leave it in `orphans` forever waiting on a
                        // fetch that can never resolve. Reject outright.
                        return outcome;
                    }
                    outcome.missing.push(*voter_parent);
                    self.insert_orphan(hash, block);
                } else {
                    self.insert_voter(hash, *chain, *voter_parent, votes.clone());
                    self.persist(&block);
                    outcome.may_confirm = true;
                    self.observe_for_retarget(timestamp_ms);
                    self.maybe_checkpoint();
                }
            }
        }

        // Always re-check orphans after every insert — failing to do so causes
        // unresolvable dependency-resolution deadlocks.
        self.drain_orphans();
        outcome
    }

    /// Record a newly-accepted proposer block: assign its level, and if it's
    /// the first block ever seen at a new deepest level, mark that level
    /// unvoted on every voter chain (the miner needs this to know what's
    /// eligible to vote on).
    fn insert_proposer(&mut self, hash: H256, parent: H256) {
        let parent_level = self.proposer_meta.get(&parent).map(|m| m.level).unwrap_or(0);
        let level = parent_level + 1;
        self.proposer_meta.insert(hash, ProposerMeta { level });

        if level > self.proposer_best_level {
            self.proposer_best_level = level;
            self.level_proposer.insert(level, hash);
            for chain_map in self.unvoted_levels.iter_mut() {
                chain_map.insert(level, hash);
            }
        }
    }

    /// Record a newly-accepted voter block, then apply fork-choice: if it
    /// extends its chain past the current tip's depth, adopt it as the new
    /// tip and reconcile votes against whatever it's replacing.
    fn insert_voter(&mut self, hash: H256, chain: ChainId, parent: H256, votes: Vec<H256>) {
        let chain_idx = chain as usize;
        let parent_depth = self.voter_meta.get(&parent).map(|m| m.depth).unwrap_or(0);
        let new_depth = parent_depth + 1;
        self.voter_meta.insert(hash, VoterMeta { parent, depth: new_depth, votes });

        let (tip_hash, tip_depth) = self.voter_tips[chain_idx];
        if new_depth > tip_depth {
            self.reorg_voter_chain(chain_idx, tip_hash, hash);
            self.voter_tips[chain_idx] = (hash, new_depth);
        }
        // A shorter-or-equal fork is recorded (for potential future adoption)
        // but doesn't change the tip or vote accounting.
    }

    /// Reconcile vote accounting when chain `chain_idx`'s tip moves from
    /// `old_tip` to `new_tip`: walk both back to their LCA, unwind votes cast
    /// on the abandoned segment (old_tip..LCA), then apply votes cast on the
    /// newly-adopted segment (LCA..new_tip).
    fn reorg_voter_chain(&mut self, chain_idx: usize, old_tip: H256, new_tip: H256) {
        let mut a = old_tip;
        let mut b = new_tip;
        let mut a_depth = self.voter_meta.get(&a).map(|m| m.depth).unwrap_or(0);
        let mut b_depth = self.voter_meta.get(&b).map(|m| m.depth).unwrap_or(0);

        let mut unwind = Vec::new();
        let mut apply = Vec::new();

        while a_depth > b_depth {
            unwind.push(a);
            a = self.voter_meta[&a].parent;
            a_depth -= 1;
        }
        while b_depth > a_depth {
            apply.push(b);
            b = self.voter_meta[&b].parent;
            b_depth -= 1;
        }
        while a != b {
            unwind.push(a);
            a = self.voter_meta[&a].parent;
            apply.push(b);
            b = self.voter_meta[&b].parent;
        }

        for h in unwind {
            let votes = self.voter_meta[&h].votes.clone();
            for vh in votes {
                self.remove_vote(chain_idx, vh);
            }
        }
        // `apply` was collected walking new_tip back to the LCA (tip-first
        // order); reverse so votes are re-applied oldest-first.
        for h in apply.into_iter().rev() {
            let depth = self.voter_meta[&h].depth;
            let votes = self.voter_meta[&h].votes.clone();
            for vh in votes {
                self.add_vote(chain_idx, vh, depth);
            }
        }
    }

    fn add_vote(&mut self, chain_idx: usize, proposer_hash: H256, depth: u64) {
        self.votes
            .entry(proposer_hash)
            .or_default()
            .per_chain
            .insert(chain_idx as u16, depth);
        if let Some(level) = self.proposer_meta.get(&proposer_hash).map(|m| m.level) {
            self.unvoted_levels[chain_idx].remove(&level);
        }
    }

    fn remove_vote(&mut self, chain_idx: usize, proposer_hash: H256) {
        if let Some(info) = self.votes.get_mut(&proposer_hash) {
            info.per_chain.remove(&(chain_idx as u16));
            if info.per_chain.is_empty() {
                self.votes.remove(&proposer_hash);
            }
        }
        if let Some(level) = self.proposer_meta.get(&proposer_hash).map(|m| m.level) {
            self.unvoted_levels[chain_idx].insert(level, proposer_hash);
        }
    }

    /// B-18: every ancestor hash that some orphan is still waiting on and that
    /// we do not have at all -- the source of truth for "ancestors we still
    /// need to fetch".
    ///
    /// Uses the same readiness conditions as `drain_orphans` (proposer:
    /// `parent`, `tx_refs`, `prop_refs`; voter: `voter_parent`; transaction:
    /// none), then keeps only refs that fail `has_block` -- so a ref that is
    /// itself an orphan is excluded (its own missing deps are reported via
    /// that orphan), as is anything known or pruned-tombstoned.
    ///
    /// Why this exists: filtering only a peer's advertised *frontier* by
    /// `has_block` misses these. `has_block` is true for orphans, so once
    /// gossip has delivered the tips (as orphans) the frontier-derived
    /// backfill seed is empty forever and the orphans' missing ancestors are
    /// never requested -- the voter chains stay disconnected and the node's
    /// ledger never confirms. Bounded by `MAX_ORPHANS` orphans (times their
    /// per-block ref counts), deduplicated.
    pub fn orphan_missing_refs(&self) -> Vec<H256> {
        let mut seen: HashSet<H256> = HashSet::new();
        let mut out: Vec<H256> = Vec::new();
        let mut want = |r: &H256, known: bool, this: &Self| {
            if !known && !this.has_block(r) && seen.insert(*r) {
                out.push(*r);
            }
        };
        for block in self.orphans.values() {
            match &block.content {
                BlockContent::Proposer { tx_refs, prop_refs } => {
                    want(&block.parent, self.proposer_meta.contains_key(&block.parent), self);
                    for r in tx_refs {
                        want(r, self.known_tx_blocks.contains(r), self);
                    }
                    for r in prop_refs {
                        want(r, self.proposer_meta.contains_key(r), self);
                    }
                }
                BlockContent::Voter { voter_parent, .. } => {
                    want(voter_parent, self.voter_meta.contains_key(voter_parent), self);
                }
                BlockContent::Transaction { .. } => {}
            }
        }
        out
    }

    /// Drain orphans whose dependencies are now satisfied.
    fn drain_orphans(&mut self) {
        let resolved: Vec<H256> = self
            .orphans
            .iter()
            .filter_map(|(hash, block)| {
                let ready = match &block.content {
                    BlockContent::Proposer { tx_refs, prop_refs } => {
                        self.proposer_meta.contains_key(&block.parent)
                            && tx_refs.iter().all(|r| self.known_tx_blocks.contains(r))
                            && prop_refs.iter().all(|r| self.proposer_meta.contains_key(r))
                    }
                    BlockContent::Voter { voter_parent, .. } => {
                        self.voter_meta.contains_key(voter_parent)
                    }
                    BlockContent::Transaction { .. } => true,
                };
                if ready {
                    Some(*hash)
                } else {
                    None
                }
            })
            .collect();

        if resolved.is_empty() {
            return;
        }
        // B-13: keep `orphan_order` in sync -- these hashes are about to
        // leave `orphans` below, so `insert_orphan`'s eviction bookkeeping
        // must not still count them.
        let resolved_set: HashSet<H256> = resolved.iter().copied().collect();
        self.orphan_order.retain(|h| !resolved_set.contains(h));
        for h in resolved {
            if let Some(block) = self.orphans.remove(&h) {
                self.insert_block(block);
            }
        }
    }

    /// Recount all votes from scratch by replaying each voter chain's current
    /// canonical history — an independent check against the incrementally
    /// maintained totals in `self.votes`.
    pub fn recount_votes(&self) -> HashMap<H256, u64> {
        let mut counts: HashMap<H256, u64> = HashMap::new();
        for &(tip_hash, _) in &self.voter_tips {
            let mut current = tip_hash;
            while let Some(meta) = self.voter_meta.get(&current) {
                if meta.depth == 0 {
                    break; // reached the genesis sentinel
                }
                for vh in &meta.votes {
                    *counts.entry(*vh).or_insert(0) += 1;
                }
                current = meta.parent;
            }
        }
        counts
    }

    /// Current incrementally-maintained vote totals, in the same shape as
    /// [`PrismChain::recount_votes`] for direct comparison in tests.
    pub fn vote_totals(&self) -> HashMap<H256, u64> {
        self.votes.iter().map(|(k, v)| (*k, v.total())).collect()
    }

    /// The level of a known proposer block, or `None` if `hash` hasn't been
    /// inserted (or is the proposer genesis). Used by `client/prism-miner` to
    /// pick the deepest unreferenced proposer as a new proposer/tx block's parent.
    pub fn proposer_level(&self, hash: &H256) -> Option<u64> {
        if *hash == self.proposer_genesis {
            return None;
        }
        self.proposer_meta.get(hash).map(|m| m.level)
    }

    /// Every known proposer block at `level`, canonical or not (sibling
    /// proposers arriving after a level's first-seen one are still recorded
    /// in `proposer_meta` -- see `insert_proposer` -- just never added to
    /// any chain's `unvoted_levels`). Needed by stage-2 confirmation
    /// (architecture guide §3.8, paper Definition 4.5), which must reason
    /// about every proposer *contending* at a level, not just the one
    /// honest miners default to voting for.
    pub fn proposers_at_level(&self, level: u64) -> Vec<H256> {
        self.proposer_meta
            .iter()
            .filter(|(_, meta)| meta.level == level)
            .map(|(hash, _)| *hash)
            .collect()
    }

    /// The current leader at `level`: the proposer at that level with the
    /// most votes, tie-broken by hash for determinism. `None` if no proposer
    /// is known at that level yet.
    ///
    /// This is a raw vote-count snapshot, not the stage-1 "conservative
    /// k-deep" *confirmed* leader (architecture guide §3.8) -- votes here
    /// aren't filtered by how deep the casting voter block is on its own
    /// chain, so it can still change as more votes arrive. That distinction
    /// (and the per-vote depth tracking it needs) is B3's job.
    pub fn leader_at_level(&self, level: u64) -> Option<H256> {
        let totals = self.vote_totals();
        self.proposer_meta
            .iter()
            .filter(|(_, meta)| meta.level == level)
            .map(|(hash, _)| (*hash, totals.get(hash).copied().unwrap_or(0)))
            .max_by_key(|(hash, votes)| (*votes, *hash))
            .map(|(hash, _)| hash)
    }

    /// True if `hash` is already known to this chain: an accepted proposer,
    /// voter, or transaction block (in-memory, or pruned out of the
    /// in-memory maps but tombstoned `BlockStatus::Pruned` in `db`), or a
    /// still-orphaned one. Used by `client/prism-network`'s backfill path
    /// (B-6) to filter a peer's `Status` response down to genuinely-unknown
    /// hashes before requesting them -- re-running `insert_block` on an
    /// *already-accepted* proposer is not idempotent (it would wrongly
    /// re-add an already-referenced proposer to `unreferenced_proposers`),
    /// so this check must happen before, not instead of, relying on
    /// `insert_block`'s own missing-ref bookkeeping (which only ever adds
    /// genuinely-unknown refs).
    ///
    /// B5 pruning: the DB fallback is load-bearing, not defensive. Once
    /// `prune` removes a voter block's `voter_meta` entry, the in-memory
    /// checks alone would report `false` for a hash that's still perfectly
    /// real (just evicted from the hot maps) -- without the tombstone
    /// check, B-6 backfill would treat every pruned hash as unknown and
    /// re-request it from peers forever, and any re-delivery would then hit
    /// the same non-idempotency this method exists to prevent.
    pub fn has_block(&self, hash: &H256) -> bool {
        if self.proposer_meta.contains_key(hash)
            || self.voter_meta.contains_key(hash)
            || self.known_tx_blocks.contains(hash)
            || self.orphans.contains_key(hash)
        {
            return true;
        }
        self.is_pruned(hash)
    }

    /// True if `hash` is specifically tombstoned `BlockStatus::Pruned` in
    /// `db` -- distinct from `has_block` (which also covers still-live
    /// in-memory state) because callers like `insert_block`'s voter arm
    /// need to distinguish "genuinely missing, go fetch it" from "gone for
    /// good, reject rather than orphan forever."
    fn is_pruned(&self, hash: &H256) -> bool {
        self.db
            .as_ref()
            .map(|db| matches!(db.get_block_status(hash), Ok(Some(BlockStatus::Pruned))))
            .unwrap_or(false)
    }

    /// B5 pruning (design resolved 2026-07-07, `docs/IMPLEMENTATION_AB_FIXPLAN.md`'s
    /// "Then: B5 (pruning)" section). Trims two independent things bounded
    /// by two independent inputs -- do not conflate them:
    ///
    /// - **Voter blocks**, bounded by `k_prune` alone. Both confirmation
    ///   stages (`prism-ledger`) read only `chain.votes` (the incrementally
    ///   maintained totals) and `voter_tips`, never voter-block history, so
    ///   pruning `voter_meta` is a complete no-op on vote accounting by
    ///   construction -- this method never touches `self.votes`. What
    ///   `k_prune` actually bounds is *reorg processability*:
    ///   `reorg_voter_chain`'s LCA walk indexes `voter_meta` directly and
    ///   would panic if a live reorg ever needed to cross the pruned
    ///   frontier, so `k_prune` must be chosen deep enough that this is
    ///   never expected (the same assumption stage-1 confirmation's own
    ///   k-deep rule already relies on -- pick `k_prune` a large multiple,
    ///   e.g. ≥10x, of that k). `insert_block`'s voter arm rejects rather
    ///   than orphans a block whose parent is tombstoned pruned, so a fork
    ///   attempting to cross the frontier is dropped, not stuck retrying
    ///   forever.
    /// - **Proposer/tx-block bodies**, bounded by `ledger_confirmed_level`
    ///   (the caller's job to supply -- `node/src/bridge.rs` owns both this
    ///   chain and `PrismLedger`, and reads `PrismLedger::confirmed_tip()`;
    ///   `prism-chain` deliberately never depends on `prism-ledger`, same
    ///   crate-boundary discipline AF-2 established). `proposer_meta` and
    ///   `known_tx_blocks` are kept forever -- every confirmation query
    ///   (`proposers_at_level`, `leader_at_level`, `level_proposer`) only
    ///   ever needs level/existence, never body content -- only the `BLOCKS`
    ///   body rows for the *canonical confirmed leader sequence*'s subtree
    ///   are dropped, mirroring `PrismLedger::build_ledger_delta_for_leader`'s
    ///   own DFS (which has, by the time a level is confirmed, already
    ///   walked and durably deduped everything those bodies could produce).
    ///
    /// Both leave a small, deliberately-unswept, *bounded* remainder behind
    /// (in the same practical-compromise spirit as B-3's `k_min`, not a
    /// correctness gap): voter blocks on an already-abandoned fork, deep
    /// enough to predate the frontier, aren't reachable by walking a live
    /// tip backward and are left in `voter_meta`; sibling (non-canonical)
    /// proposers at a confirmed level keep their bodies, since only the
    /// leader's own subtree is walked. Exact sweeping of either would need
    /// a per-chain/per-level reverse index this port doesn't maintain.
    ///
    /// Checkpoints immediately afterward (`checkpoint_now`) so a crash right
    /// after pruning doesn't leave the next restart's replay trying to walk
    /// bodies this call just deleted.
    pub fn prune(&mut self, k_prune: u64, ledger_confirmed_level: u64) {
        self.prune_voter_blocks(k_prune);
        self.prune_confirmed_bodies(ledger_confirmed_level);
        self.checkpoint_now();
    }

    fn prune_voter_blocks(&mut self, k_prune: u64) {
        let Some(db) = self.db.clone() else { return };
        let tips = self.voter_tips.clone();
        for (tip_hash, tip_depth) in tips {
            let frontier = tip_depth.saturating_sub(k_prune);
            if frontier == 0 {
                continue; // this chain isn't deep enough yet for anything to be prunable
            }
            // Walk from the tip back to the frontier without touching
            // anything still inside the retained window.
            let mut current = tip_hash;
            loop {
                let Some(meta) = self.voter_meta.get(&current) else { break };
                if meta.depth <= frontier {
                    break;
                }
                current = meta.parent;
            }
            // `current` is now the deepest still-retained block. Everything
            // from here back to (but not including) the genesis sentinel is
            // prunable.
            loop {
                let Some(meta) = self.voter_meta.get(&current).cloned() else { break };
                if meta.depth == 0 {
                    break; // never prune the genesis sentinel
                }
                self.voter_meta.remove(&current);
                let _ = db.delete_block_body(&current);
                let _ = db.set_block_status(&current, BlockStatus::Pruned);
                current = meta.parent;
            }
        }
    }

    fn prune_confirmed_bodies(&mut self, ledger_confirmed_level: u64) {
        let Some(db) = self.db.clone() else { return };
        if ledger_confirmed_level <= self.last_body_pruned_level {
            return;
        }
        let mut visited = HashSet::new();
        let mut stalled = false;
        for level in (self.last_body_pruned_level + 1)..=ledger_confirmed_level {
            // R-5: the leader to sweep is the one the *ledger confirmed*
            // (recorded in `PROPOSER_LEADER_SEQUENCE`), never
            // `level_proposer[level]`, which is merely the first proposer
            // THIS node saw at that level -- local state, not consensus. If
            // stage-2 confirmed P while this node first saw P', sweeping
            // `level_proposer` would delete the wrong subtree's bodies.
            // A level with no recorded leader is left unpruned (and stops
            // the sweep cursor there so it's retried) rather than guessed.
            let leader = match db.get_h256(columns::PROPOSER_LEADER_SEQUENCE, &level.to_le_bytes()) {
                Ok(Some(leader)) => leader,
                _ => {
                    log::warn!(
                        target: "prism",
                        "R-5: no recorded confirmed leader for level {level} <= ledger_confirmed_level \
                         {ledger_confirmed_level}; not pruning it"
                    );
                    stalled = true;
                    continue;
                }
            };
            prune_proposer_subtree_bodies(&db, &leader, &mut visited);
        }
        if !stalled {
            self.last_body_pruned_level = ledger_confirmed_level;
        }
    }

    fn persist(&self, block: &PrismBlock) {
        if self.replaying {
            return; // already on disk -- that's where replay just read it from
        }
        if let Some(db) = &self.db {
            let _ = db.insert_block(block);
        }
    }
}

/// Delete `proposer_hash`'s body and every body reachable through its
/// `tx_refs`/`prop_refs` (mirrors `PrismLedger::build_ledger_delta_for_leader`'s
/// DFS -- see `PrismChain::prune`'s doc comment). `visited` both prevents
/// re-walking a proposer reachable via multiple paths (the proposer DAG can
/// have diamonds) and lets repeated `prune` calls skip already-pruned
/// subtrees cheaply: `db.get_block` returns `None` for an already-pruned
/// hash, which this treats as "nothing left to do here," not an error.
fn prune_proposer_subtree_bodies(db: &PrismDb, proposer_hash: &H256, visited: &mut HashSet<H256>) {
    if !visited.insert(*proposer_hash) {
        return;
    }
    let Ok(Some(block)) = db.get_block(proposer_hash) else {
        return;
    };
    if let BlockContent::Proposer { tx_refs, prop_refs } = &block.content {
        for r in tx_refs {
            let _ = db.delete_block_body(r);
            let _ = db.set_block_status(r, BlockStatus::Pruned);
        }
        for r in prop_refs {
            prune_proposer_subtree_bodies(db, r, visited);
        }
    }
    let _ = db.delete_block_body(proposer_hash);
    let _ = db.set_block_status(proposer_hash, BlockStatus::Pruned);
}

#[cfg(test)]
mod tests;
