//! Prism DAG state machine — port of prism-rust `blockchain/` module.
//!
//! All consensus logic lives here. The Substrate runtime is only called via
//! `prism-ledger` after a leader is confirmed (ε-final).
//!
//! # WARNING: vote rollback
//! Voter-chain reorg requires rolling back votes from the abandoned segment
//! and re-applying votes from the new main-chain segment. This is the hardest
//! code path in the whole port. The stub below handles the happy path only.
//! Full rollback logic must be ported from prism-rust `blockchain/mod.rs`
//! before the B1 acceptance tests can pass.
use argonaut_primitives_prism::{block_hash, BlockContent, ChainId, Params, PrismBlock, H256};
use argonaut_prism_db::PrismDb;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

/// Per-proposer vote accounting.
#[derive(Clone, Debug, Default)]
pub struct VoteInfo {
    /// Total votes accumulated across all voter chains.
    pub total: u64,
    /// Per voter chain: Option<(depth_from_tip, voted_level)>.
    pub per_chain: Vec<Option<(u64, u64)>>,
}

/// Outcome of inserting a block — drives network requests and bridge wakeups.
pub struct InsertOutcome {
    /// Hashes of blocks we need to fetch from peers (missing parents or refs).
    pub missing: Vec<H256>,
    /// True if new leader levels may now be confirmable by prism-ledger.
    pub may_confirm: bool,
}

/// The Prism chain state machine (in-memory state, persisted via PrismDb).
pub struct PrismChain {
    pub params: Params,
    pub proposer_best_level: u64,
    /// Per voter chain: (tip_hash, tip_depth).
    pub voter_tips: Vec<(H256, u64)>,
    /// Tx blocks not yet referenced by any proposer block.
    pub unreferenced_tx_blocks: HashSet<H256>,
    /// Proposer blocks not yet referenced by a deeper proposer.
    pub unreferenced_proposers: HashSet<H256>,
    /// Per voter chain: level → first-seen proposer hash at that level.
    pub unvoted_levels: Vec<BTreeMap<u64, H256>>,
    /// Proposer → accumulated vote data.
    pub votes: HashMap<H256, VoteInfo>,
    /// Blocks waiting on missing parent or refs (hash → block).
    pub orphans: HashMap<H256, PrismBlock>,
    pub db: Arc<PrismDb>,
}

impl PrismChain {
    pub fn new(params: Params, db: Arc<PrismDb>) -> Self {
        let m = params.m_voter_chains as usize;
        PrismChain {
            params,
            proposer_best_level: 0,
            voter_tips: vec![(H256::zero(), 0); m],
            unreferenced_tx_blocks: HashSet::new(),
            unreferenced_proposers: HashSet::new(),
            unvoted_levels: vec![BTreeMap::new(); m],
            votes: HashMap::new(),
            orphans: HashMap::new(),
            db,
        }
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
        let mut missing = Vec::new();
        let mut may_confirm = false;

        match block.content.clone() {
            BlockContent::Transaction { .. } => {
                self.db.insert_block(&block).ok();
                self.unreferenced_tx_blocks.insert(hash);
            }

            BlockContent::Proposer { ref tx_refs, ref prop_refs } => {
                for r in tx_refs.iter().chain(prop_refs.iter()) {
                    if self.db.get_block(r).ok().flatten().is_none() {
                        missing.push(*r);
                    }
                }
                if missing.is_empty() {
                    self.db.insert_block(&block).ok();
                    for r in tx_refs { self.unreferenced_tx_blocks.remove(r); }
                    for r in prop_refs { self.unreferenced_proposers.remove(r); }
                    self.unreferenced_proposers.insert(hash);
                    may_confirm = true;
                } else {
                    self.orphans.insert(hash, block);
                }
            }

            BlockContent::Voter { chain, ref votes, .. } => {
                let chain = chain as usize;
                if chain >= self.voter_tips.len() {
                    return InsertOutcome { missing, may_confirm };
                }
                self.db.insert_block(&block).ok();

                // TODO (B1): full vote rollback on voter-chain reorg.
                // Port prism-rust blockchain/mod.rs::update_voter_chain — unwind
                // votes from the abandoned fork, re-apply from the new tip.
                let new_depth = self.voter_tips[chain].1 + 1;
                self.voter_tips[chain] = (hash, new_depth);

                for vh in votes {
                    self.votes.entry(*vh).or_insert_with(VoteInfo::default).total += 1;
                }
                may_confirm = true;
            }
        }

        // Always re-check orphans after every insert — failing to do so causes
        // unresolvable dependency deadlocks on DAG reconnection.
        self.drain_orphans();
        InsertOutcome { missing, may_confirm }
    }

    /// Drain orphans whose dependencies are now satisfied.
    fn drain_orphans(&mut self) {
        let resolved: Vec<H256> = self
            .orphans
            .iter()
            .filter_map(|(hash, block)| {
                if let BlockContent::Proposer { tx_refs, prop_refs } = &block.content {
                    let all_present = tx_refs.iter().chain(prop_refs.iter()).all(|r| {
                        self.db.get_block(r).ok().flatten().is_some()
                    });
                    if all_present { Some(*hash) } else { None }
                } else {
                    None
                }
            })
            .collect();

        for h in resolved {
            if let Some(block) = self.orphans.remove(&h) {
                self.insert_block(block);
            }
        }
    }

    /// Recount all votes from scratch — used to verify incremental accounting.
    /// Property test: `recount_votes()` == `self.votes` after any sequence of inserts.
    pub fn recount_votes(&self) -> HashMap<H256, u64> {
        let mut counts: HashMap<H256, u64> = HashMap::new();
        for (tip_hash, tip_depth) in &self.voter_tips {
            let mut current = *tip_hash;
            let mut depth = *tip_depth;
            while depth > 0 {
                if let Ok(Some(block)) = self.db.get_block(&current) {
                    if let BlockContent::Voter { votes, voter_parent, .. } = &block.content {
                        for vh in votes {
                            *counts.entry(*vh).or_insert(0) += 1;
                        }
                        current = *voter_parent;
                    }
                }
                depth -= 1;
            }
        }
        counts
    }
}
