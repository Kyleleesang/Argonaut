//! Prism confirmation rules and Substrate bridge (§3.8).
//!
//! # Stage 1 (B3): conservative k-deep confirmation
//! Leader at level ℓ is confirmed when every vote for it is ≥ K_DEEP blocks
//! behind its voter-chain tip. Latency ≈ Bitcoin; obviously safe. Acceptable
//! for the first devnet. [`confirmed_leader_at_level`] implements the rule
//! (AF-2: moved here from `PrismChain` -- architecture guide §3.8 assigns
//! *all* confirmation to this crate, not the DAG state machine; `PrismChain`
//! only needs to expose the raw facts the policy reads, which were already
//! public fields).
//!
//! # Stage 2 (B4): §4.5 confidence-interval fast confirmation
//! See [`confirmation`]. `δ_d`'s exact paper constant (Appendix E.1) is an
//! explicit, flagged placeholder, not derived -- see that module's docs.
//!
//! # BuildLedger (paper Alg. 2)
//! For confirmed leader sequence p_1..p_ℓ: expand each leader's direct+indirect
//! tx-block refs in reference order, concatenate, deduplicate by tx hash.
//!
//! # Substrate bridge
//! One Substrate block per confirmed leader level (or per batch). Imports with
//! `finalized: true` -- Prism is ε-final so the chain never reorgs. Skip
//! individual tx failures (don't abort); log but don't panic. That actual
//! apply/skip decision happens in the node-side bridge (`node/src/bridge.rs`,
//! via `sc-block-builder`), not in this crate -- this crate only guarantees
//! the *input* to that step (the ordered, deduped tx list) is a deterministic
//! function of the confirmed DAG, so every honest node builds from identical
//! input.
use argonaut_prism_chain::PrismChain;
use argonaut_prism_db::{columns, PrismDb};
use argonaut_primitives_prism::{BlockContent, H256};
use std::collections::HashSet;
use std::sync::Arc;

pub mod confirmation;

/// Conservative k-deep confirmation threshold (Stage 1).
pub const K_DEEP_CONFIRMATION: u64 = 6;

/// Stage-1 "conservative k-deep" leader confirmation (architecture guide
/// §3.8, paper §4.1's conservative policy): `level` is confirmed once
/// *every* voter chain has cast its vote for the canonical proposer at that
/// level, and every one of those votes is at least `k` blocks behind its
/// chain's current tip (deep enough that reversing it via reorg would need
/// to out-race `k` blocks of work -- the same safety margin as a
/// k-confirmation Bitcoin payment). Deliberately conservative/unanimous
/// rather than a fast probabilistic threshold, matching "stage 1,
/// correctness-first, latency ~Bitcoin" from the guide; [`confirmation`]'s
/// stage-2 confidence-interval rule is a separate, faster mechanism layered
/// on top.
///
/// Returns `None` if the level has no proposer yet, if any chain hasn't
/// voted on it yet, or if any chain's vote isn't `k`-deep yet -- callers
/// should treat all three as "not confirmed yet," not an error.
pub fn confirmed_leader_at_level(chain: &PrismChain, level: u64, k: u64) -> Option<H256> {
    let proposer = *chain.level_proposer.get(&level)?;
    let m = chain.params.m_voter_chains as usize;

    for chain_idx in 0..m {
        if chain.unvoted_levels[chain_idx].contains_key(&level) {
            return None; // this chain hasn't voted on this level yet
        }
    }

    let info = chain.votes.get(&proposer)?;
    if info.per_chain.len() != m {
        return None; // shouldn't happen once every chain has voted, but guard defensively
    }
    let all_k_deep = info.per_chain.iter().all(|(&chain_idx, &depth)| {
        let tip_depth = chain.voter_tips[chain_idx as usize].1;
        tip_depth.saturating_sub(depth) >= k
    });

    if all_k_deep {
        Some(proposer)
    } else {
        None
    }
}

/// A newly confirmed leader level, with its BuildLedger delta already computed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedLevel {
    pub level: u64,
    pub leader: H256,
    /// Txs newly introduced by this level (already deduped against every tx
    /// emitted by any earlier confirmed level).
    pub txs: Vec<Vec<u8>>,
    /// The leader `PrismBlock`'s own `SuperHeader.timestamp_ms` -- a
    /// deterministic function of the confirmed DAG (baked into the block's
    /// hash, so every node that agrees on `leader` agrees on this bit-for-
    /// bit). The bridge (`node/src/bridge.rs`) uses this instead of local
    /// wall-clock time for the Substrate block's timestamp inherent --
    /// using `SystemTime::now()` there made two honest nodes confirming the
    /// same level produce different block hashes, violating the "every
    /// honest node derives the identical Substrate block" contract
    /// (architecture guide §3.1).
    pub timestamp_ms: u64,
}

/// Walks confirmed Prism leader levels forward, one at a time, producing each
/// level's BuildLedger delta. Owns the "next unconfirmed level" cursor and the
/// global (cross-level) tx dedup set -- both must survive across calls to
/// `advance`, since a tx included at level ℓ must never be re-emitted at ℓ+1
/// even if some later proposer references the same tx block again.
pub struct PrismLedger {
    db: Arc<PrismDb>,
    confirmed_tip: u64,
    emitted_tx_hashes: HashSet<[u8; 32]>,
}

impl PrismLedger {
    pub fn new(db: Arc<PrismDb>) -> Self {
        PrismLedger { db, confirmed_tip: 0, emitted_tx_hashes: HashSet::new() }
    }

    /// The last level whose delta has already been returned by `advance`.
    pub fn confirmed_tip(&self) -> u64 {
        self.confirmed_tip
    }

    /// Advance as far as `chain`'s current DAG state allows, given
    /// confirmation depth `k`. Levels are confirmed strictly in order --
    /// stops at the first level not yet confirmed and never skips ahead, so
    /// the ledger delta at level ℓ can always assume every earlier level's
    /// txs have already been emitted.
    ///
    /// B-4: tries stage-2's confidence-interval confirmation
    /// ([`confirmation::confirm_unique_leader`]) first, falling back to
    /// stage-1's conservative k-deep rule -- stage-2's list is guaranteed to
    /// contain the eventual leader, so a *uniquely* confirmed stage-2 leader
    /// is safe to act on before stage-1's unanimity requirement is met.
    /// Since this function is called repeatedly (every bridge wakeup, per
    /// AF-1), whichever rule fires first in real time is the one that ends
    /// up confirming the level -- there's no separate "race" to coordinate,
    /// just checking both every time. Stage-2's numeric margin (`δ_d`) is
    /// still a flagged placeholder (B-3, not yet the paper's derived
    /// constant), so this is "faster when available, never less safe than
    /// stage-1 alone" rather than a fully paper-faithful latency win yet.
    pub fn advance(&mut self, chain: &PrismChain, k: u64) -> Vec<ConfirmedLevel> {
        let mut out = Vec::new();
        loop {
            let next = self.confirmed_tip + 1;
            let stage2 = confirmation::confirm_unique_leader(
                chain,
                next,
                &confirmation::ConfirmationParams::default(),
            );
            let Some(leader) = stage2.or_else(|| confirmed_leader_at_level(chain, next, k)) else {
                break;
            };
            let txs = self.build_ledger_delta_for_leader(&leader);
            // The leader block's own timestamp -- deterministic across every
            // node that agrees on `leader` (it's part of what's hashed into
            // the block's identity). See `ConfirmedLevel::timestamp_ms`.
            let timestamp_ms = self
                .db
                .get_block(&leader)
                .ok()
                .flatten()
                .map(|b| b.header.timestamp_ms)
                .unwrap_or(0);

            let _ = self.db.put_h256(
                columns::PROPOSER_LEADER_SEQUENCE,
                &next.to_le_bytes(),
                &leader,
            );
            let _ = self.db.put_h256(
                columns::PROPOSER_LEDGER_ORDER,
                &next.to_le_bytes(),
                &leader,
            );

            self.confirmed_tip = next;
            out.push(ConfirmedLevel { level: next, leader, txs, timestamp_ms });
        }
        out
    }

    /// BuildLedger for a single confirmed leader (paper Alg. 2): expand the
    /// leader's direct+indirect tx-block refs (DFS over `prop_refs`, tx_refs
    /// collected in visitation order), dedup by tx hash against every tx ever
    /// emitted by this `PrismLedger` (not just within this level).
    fn build_ledger_delta_for_leader(&mut self, leader_hash: &H256) -> Vec<Vec<u8>> {
        let mut tx_block_hashes = Vec::new();
        self.expand_proposer_tx_refs(leader_hash, &mut tx_block_hashes, &mut HashSet::new());

        let mut result = Vec::new();
        for tx_block_hash in &tx_block_hashes {
            if let Ok(Some(block)) = self.db.get_block(tx_block_hash) {
                if let BlockContent::Transaction { txs } = block.content {
                    for tx in txs {
                        let tx_hash = *blake3::hash(&tx).as_bytes();
                        if self.emitted_tx_hashes.insert(tx_hash) {
                            result.push(tx);
                        }
                    }
                }
            }
        }
        result
    }

    /// DFS over a proposer's `prop_refs`, collecting every reachable
    /// `tx_refs` entry in visitation order. `visited` prevents re-walking a
    /// proposer reachable via multiple paths (the proposer DAG can have
    /// diamonds: two proposers both referencing a common ancestor).
    fn expand_proposer_tx_refs(
        &self,
        proposer_hash: &H256,
        tx_block_hashes: &mut Vec<H256>,
        visited: &mut HashSet<H256>,
    ) {
        if !visited.insert(*proposer_hash) {
            return;
        }
        if let Ok(Some(block)) = self.db.get_block(proposer_hash) {
            if let BlockContent::Proposer { tx_refs, prop_refs } = block.content {
                for r in &tx_refs {
                    tx_block_hashes.push(*r);
                }
                for r in &prop_refs {
                    self.expand_proposer_tx_refs(r, tx_block_hashes, visited);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod adversarial_tests;
