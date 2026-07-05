//! Prism confirmation rules and Substrate bridge (§3.8).
//!
//! # Stage 1 (B3): conservative k-deep confirmation
//! Leader at level ℓ is confirmed when every vote for it is ≥ K_DEEP blocks
//! behind its voter-chain tip. Latency ≈ Bitcoin; obviously safe. Acceptable
//! for the first devnet.
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
//! `finalized: true` — Prism is ε-final so the chain never reorgs.
//! Skip individual tx failures (don't abort); log but don't panic.
//! Determinism: skip rule is a pure function of (ordered tx list, prior state).
use argonaut_prism_db::{columns, PrismDb};
use argonaut_primitives_prism::{BlockContent, H256};
use std::collections::HashSet;
use std::sync::Arc;

/// Conservative k-deep confirmation threshold (Stage 1).
pub const K_DEEP_CONFIRMATION: u64 = 6;

pub struct PrismLedger {
    db: Arc<PrismDb>,
    confirmed_leader_tip: u64,
    /// Tx hashes already emitted to Substrate — global dedup across all leaders.
    emitted_tx_hashes: HashSet<[u8; 32]>,
}

impl PrismLedger {
    pub fn new(db: Arc<PrismDb>) -> Self {
        PrismLedger {
            db,
            confirmed_leader_tip: 0,
            emitted_tx_hashes: HashSet::new(),
        }
    }

    /// Stage-1 confirmation: proposer at level ℓ is confirmed once every
    /// vote for it is ≥ K_DEEP blocks behind its voter-chain tip.
    pub fn is_confirmed_k_deep(&self, vote_depths: &[u64]) -> bool {
        !vote_depths.is_empty()
            && vote_depths.iter().all(|&d| d >= K_DEEP_CONFIRMATION)
    }

    /// BuildLedger Δ (paper Alg. 2):
    /// Return all txs newly included by confirming up to `leader_level`.
    /// Expands leader's direct+indirect tx-block ref graph in reference order,
    /// deduplicates by tx hash, skips any already emitted.
    pub fn build_ledger_delta(&mut self, leader_level: u64) -> Vec<Vec<u8>> {
        let mut result = Vec::new();

        // Walk confirmed leader levels from confirmed_leader_tip+1 to leader_level.
        for level in (self.confirmed_leader_tip + 1)..=leader_level {
            let leader_hash = match self.db.get_h256(
                columns::PROPOSER_LEADER_SEQUENCE,
                &level.to_le_bytes(),
            ) {
                Ok(Some(h)) => h,
                _ => continue,
            };

            // Expand proposer's tx-block refs (BFS over prop_refs to find all tx refs)
            let mut tx_block_hashes = Vec::new();
            self.expand_proposer_tx_refs(&leader_hash, &mut tx_block_hashes, &mut HashSet::new());

            // Collect txs in ref order, dedup
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

            // Persist ledger order for auditability
            self.db.put_h256(
                columns::PROPOSER_LEDGER_ORDER,
                &level.to_le_bytes(),
                &leader_hash,
            ).ok();
        }

        self.confirmed_leader_tip = leader_level;
        result
    }

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
