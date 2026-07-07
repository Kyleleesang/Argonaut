//! Prism confirmation rules and Substrate bridge (§3.8).
//!
//! # Stage 1 (B3): conservative k-deep confirmation
//! Leader at level ℓ is confirmed when every vote for it is ≥ K_DEEP blocks
//! behind its voter-chain tip. Latency ≈ Bitcoin; obviously safe. Acceptable
//! for the first devnet. The actual rule lives in
//! [`argonaut_prism_chain::PrismChain::confirmed_leader_at_level`] (per-vote
//! depth tracking added alongside it); this crate just walks it forward.
//!
//! # Stage 2 (B4): §4.5 confidence-interval fast confirmation
//! TODO: implement `V_lower_n(r)` / `V_upper_n(r)` and the private-block upper
//! bound. Port constants (δ_d table) from paper §4.5.1.
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

/// A newly confirmed leader level, with its BuildLedger delta already computed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedLevel {
    pub level: u64,
    pub leader: H256,
    /// Txs newly introduced by this level (already deduped against every tx
    /// emitted by any earlier confirmed level).
    pub txs: Vec<Vec<u8>>,
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
    pub fn advance(&mut self, chain: &PrismChain, k: u64) -> Vec<ConfirmedLevel> {
        let mut out = Vec::new();
        loop {
            let next = self.confirmed_tip + 1;
            let Some(leader) = chain.confirmed_leader_at_level(next, k) else {
                break;
            };
            let txs = self.build_ledger_delta_for_leader(&leader);

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
            out.push(ConfirmedLevel { level: next, leader, txs });
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
