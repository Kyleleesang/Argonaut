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
use argonaut_primitives_prism::{block_hash, BlockContent, ChainId, Params, PrismBlock, H256};
use argonaut_prism_db::PrismDb;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

/// Per-proposer accumulated vote count (summed across all voter chains).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VoteInfo {
    pub total: u64,
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
#[derive(Clone, Debug)]
struct VoterMeta {
    parent: H256,
    depth: u64,
    /// Proposer hashes this block voted on (copied from its `BlockContent`).
    votes: Vec<H256>,
}

/// Lightweight per-proposer-block metadata.
#[derive(Clone, Debug)]
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
    /// Proposer → accumulated vote count.
    pub votes: HashMap<H256, VoteInfo>,
    /// Blocks waiting on missing parent or refs (hash → block).
    pub orphans: HashMap<H256, PrismBlock>,
    voter_meta: HashMap<H256, VoterMeta>,
    proposer_meta: HashMap<H256, ProposerMeta>,
    db: Option<Arc<PrismDb>>,
}

/// Shared genesis sentinel used by every voter chain and the proposer tree.
/// Not a real mined block; seeded directly into the metadata maps so parent
/// lookups for the first real block on each chain resolve without special-casing.
fn genesis_sentinel() -> H256 {
    H256::zero()
}

impl PrismChain {
    /// Pure in-memory chain state (no durable storage). Suitable for tests.
    pub fn new(params: Params) -> Self {
        let m = params.m_voter_chains as usize;
        let genesis = genesis_sentinel();

        let mut voter_meta = HashMap::new();
        voter_meta.insert(genesis, VoterMeta { parent: genesis, depth: 0, votes: Vec::new() });

        let mut proposer_meta = HashMap::new();
        proposer_meta.insert(genesis, ProposerMeta { level: 0 });

        PrismChain {
            params,
            proposer_best_level: 0,
            voter_tips: vec![(genesis, 0); m],
            unreferenced_tx_blocks: HashSet::new(),
            unreferenced_proposers: HashSet::new(),
            known_tx_blocks: HashSet::new(),
            unvoted_levels: vec![BTreeMap::new(); m],
            votes: HashMap::new(),
            orphans: HashMap::new(),
            voter_meta,
            proposer_meta,
            db: None,
        }
    }

    /// Same as [`PrismChain::new`], but persists every accepted block's full
    /// body to `db` as it's inserted (best-effort: a DB write failure does not
    /// abort insertion, since in-memory state is still consistent either way).
    pub fn with_db(params: Params, db: Arc<PrismDb>) -> Self {
        let mut chain = Self::new(params);
        chain.db = Some(db);
        chain
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
        let mut outcome = InsertOutcome::default();

        match &block.content {
            BlockContent::Transaction { .. } => {
                self.persist(&block);
                self.known_tx_blocks.insert(hash);
                self.unreferenced_tx_blocks.insert(hash);
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
                } else {
                    self.orphans.insert(hash, block);
                }
            }

            BlockContent::Voter { chain, voter_parent, votes } => {
                let chain_idx = *chain as usize;
                if chain_idx >= self.voter_tips.len() {
                    // Structurally invalid (should have been rejected by
                    // `validate_block` already); ignore defensively.
                    return outcome;
                }
                if !self.voter_meta.contains_key(voter_parent) {
                    outcome.missing.push(*voter_parent);
                    self.orphans.insert(hash, block);
                } else {
                    self.insert_voter(hash, *chain, *voter_parent, votes.clone());
                    self.persist(&block);
                    outcome.may_confirm = true;
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
            let votes = self.voter_meta[&h].votes.clone();
            for vh in votes {
                self.add_vote(chain_idx, vh);
            }
        }
    }

    fn add_vote(&mut self, chain_idx: usize, proposer_hash: H256) {
        self.votes.entry(proposer_hash).or_default().total += 1;
        if let Some(level) = self.proposer_meta.get(&proposer_hash).map(|m| m.level) {
            self.unvoted_levels[chain_idx].remove(&level);
        }
    }

    fn remove_vote(&mut self, chain_idx: usize, proposer_hash: H256) {
        if let Some(info) = self.votes.get_mut(&proposer_hash) {
            info.total = info.total.saturating_sub(1);
            if info.total == 0 {
                self.votes.remove(&proposer_hash);
            }
        }
        if let Some(level) = self.proposer_meta.get(&proposer_hash).map(|m| m.level) {
            self.unvoted_levels[chain_idx].insert(level, proposer_hash);
        }
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
        self.votes.iter().map(|(k, v)| (*k, v.total)).collect()
    }

    fn persist(&self, block: &PrismBlock) {
        if let Some(db) = &self.db {
            let _ = db.insert_block(block);
        }
    }
}

#[cfg(test)]
mod tests;
