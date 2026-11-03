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
    let m = chain.params.m_voter_chains as usize;

    for chain_idx in 0..m {
        if chain.unvoted_levels[chain_idx].contains_key(&level) {
            return None; // this chain hasn't voted on this level yet
        }
    }

    // R-4a: the candidate comes from VOTE STATE only -- the proposer at this
    // level holding a vote from all m chains, every one >= k deep. It used
    // to be `chain.level_proposer[level]`, i.e. whichever proposer this node
    // happened to see first: local, arrival-order-dependent state, so two
    // honest nodes could run the unanimity check on different proposers.
    //
    // More than one proposer can currently satisfy this only because a
    // voter block may double-vote a level (R-4b); we then pick the lowest
    // hash, which is at least deterministic across nodes. R-4b's
    // vote-validity rules (exactly one known proposer per level per voter
    // block) will make the qualifying proposer unique, and this tie-break
    // then becomes dead code.
    chain
        .proposers_at_level(level)
        .into_iter()
        .filter(|proposer| {
            let Some(info) = chain.votes.get(proposer) else { return false };
            if info.per_chain.len() != m {
                return false; // not voted by every chain (also guards defensively)
            }
            info.per_chain.iter().all(|(&chain_idx, &depth)| {
                let tip_depth = chain.voter_tips[chain_idx as usize].1;
                tip_depth.saturating_sub(depth) >= k
            })
        })
        .min()
}

/// A newly confirmed leader level, with its BuildLedger delta already computed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedLevel {
    pub level: u64,
    pub leader: H256,
    /// Txs of this level's delta (R-6: deduped within this level only).
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
/// R-6: there is deliberately NO global emitted-tx set any more; dedup is
/// per level (see `build_ledger_delta_for_leader`).
pub struct PrismLedger {
    db: Arc<PrismDb>,
    confirmed_tip: u64,
}

impl PrismLedger {
    pub fn new(db: Arc<PrismDb>) -> Self {
        PrismLedger { db, confirmed_tip: 0 }
    }

    /// B-7: rebuild a ledger's cursor + tx-dedup state after a restart.
    ///
    /// `new` alone is only correct for a genuinely fresh chain: it always
    /// starts `confirmed_tip` at 0, so on a restarted node with an existing
    /// DAG, `advance` would re-confirm level 1 all over again and the bridge
    /// (`node/src/bridge.rs`) would build a *second* Substrate block for it on
    /// top of the real chain's current tip -- silently re-applying every
    /// already-settled transaction as brand-new extrinsics.
    ///
    /// `confirmed_tip` must come from the Substrate client's own
    /// `info().best_number` (the bridge's caller reads this), not from this
    /// crate's own `PROPOSER_LEDGER_ORDER`/`PROPOSER_LEADER_SEQUENCE` columns:
    /// `advance` writes those *before* the corresponding Substrate block is
    /// actually built and imported, so if a node crashes mid-import those
    /// columns can be one level ahead of what was truly committed. The
    /// Substrate best-block number can never be ahead of reality, so it's the
    /// only race-free anchor.
    ///
    /// R-6: `recover` no longer reloads any tx-dedup state. The old global
    /// `EMITTED_TX_HASH` set was persisted BEFORE the Substrate block was
    /// executed, so (a) a tx that failed execution (nonce n+1 ordered before
    /// n) was blacklisted forever, and (b) a crash between `advance` and
    /// import made the restarted node rebuild level N with its txs deduped
    /// away -- a fork. Now a level's delta is a pure function of the DAG, so
    /// re-advancing from `confirmed_tip` reproduces it exactly. Replay
    /// protection is the runtime's job (nonce/era).
    pub fn recover(db: Arc<PrismDb>, confirmed_tip: u64) -> Self {
        PrismLedger { db, confirmed_tip }
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
    /// B-4/B-15: tries stage-2's confidence-interval confirmation
    /// ([`confirmation::confirm_unique_leader`]) first, falling back to
    /// stage-1's conservative k-deep rule -- stage-2's list is guaranteed to
    /// contain the eventual leader, so a *uniquely* confirmed stage-2 leader
    /// is safe to act on before stage-1's unanimity requirement is met.
    /// Since this function is called repeatedly (every bridge wakeup, per
    /// AF-1), whichever rule fires first in real time is the one that ends
    /// up confirming the level -- there's no separate "race" to coordinate,
    /// just checking both every time. Stage-2's default margin is now B-3's
    /// real derived one ([`confirmation::single_check_margin`]), which is
    /// vacuous below ~400-deep votes at `m=100` -- see `confirmation.rs`'s
    /// module doc's B-15 section for why this makes stage-2 an eventual
    /// safety backstop for stage-1's unanimity weakness, not a latency win.
    pub fn advance(&mut self, chain: &PrismChain, k: u64) -> Vec<ConfirmedLevel> {
        self.advance_with_params(chain, k, &confirmation::ConfirmationParams::default())
    }

    /// Same as [`advance`](Self::advance), but with an explicit
    /// [`confirmation::ConfirmationParams`] instead of the default (which has
    /// `stage2_enabled: true` as of B-15 -- see that field's doc comment).
    /// The node builds its `ConfirmationParams` from chain-spec
    /// `prismBetaPercent`/`prismEpsExponent` and `--prism-no-stage2`, then
    /// threads the whole struct down to this (`node/src/bridge.rs`).
    pub fn advance_with_params(
        &mut self,
        chain: &PrismChain,
        k: u64,
        confirmation_params: &confirmation::ConfirmationParams,
    ) -> Vec<ConfirmedLevel> {
        let mut out = Vec::new();
        loop {
            let next = self.confirmed_tip + 1;
            let stage2 = confirmation::confirm_unique_leader(chain, next, confirmation_params);
            let via_stage2 = stage2.is_some();
            let Some(leader) = stage2.or_else(|| confirmed_leader_at_level(chain, next, k)) else {
                break;
            };
            // B-15: which rule fired is the live signal that stage-2 is
            // evaluated-but-not-firing at shallow depth (stage-1 wins).
            log::debug!(
                target: "prism",
                "level {} confirmed via {}",
                next,
                if via_stage2 { "stage-2" } else { "stage-1" },
            );
            // R-5/R-6: build the COMPLETE delta first; nothing for this
            // level is persisted unless that succeeds. A missing body (or
            // leader timestamp) would make this node emit a different delta
            // than its peers, so we halt instead of advancing.
            let (txs, timestamp_ms) = match self.build_ledger_delta_for_leader(chain, next, &leader) {
                Ok(v) => v,
                Err(why) => {
                    log::error!(
                        target: "prism",
                        "R-5: cannot fully build ledger delta for level {} (leader {:?}): {}; \
                         not advancing confirmed_tip",
                        next, leader, why,
                    );
                    break;
                }
            };

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
    /// collected in visitation order), dedup by tx hash WITHIN this level
    /// only (first occurrence wins).
    ///
    /// R-6: no cross-level dedup -- the same tx at a later level is emitted
    /// again and the runtime rejects it by nonce, identically on every node.
    /// To keep that from re-emitting the entire history every level, the DFS
    /// does not descend into an earlier level's confirmed leader (its whole
    /// subtree was already emitted at that level; with pruning its bodies
    /// may be gone too).
    ///
    /// R-5: returns `Err` if any needed body or the leader's timestamp is
    /// unavailable -- never a partial delta.
    fn build_ledger_delta_for_leader(
        &self,
        chain: &PrismChain,
        level: u64,
        leader_hash: &H256,
    ) -> Result<(Vec<Vec<u8>>, u64), String> {
        let leader_block = self
            .db
            .get_block(leader_hash)
            .map_err(|e| format!("db error loading leader body: {e:?}"))?
            .ok_or_else(|| "leader body missing".to_string())?;
        let timestamp_ms = leader_block.header.timestamp_ms;

        let mut tx_block_hashes = Vec::new();
        self.expand_proposer_tx_refs(
            chain,
            level,
            leader_hash,
            true,
            &mut tx_block_hashes,
            &mut HashSet::new(),
        )?;

        let mut seen: HashSet<[u8; 32]> = HashSet::new();
        let mut result = Vec::new();
        for tx_block_hash in &tx_block_hashes {
            let block = self
                .db
                .get_block(tx_block_hash)
                .map_err(|e| format!("db error loading tx block {tx_block_hash:?}: {e:?}"))?
                .ok_or_else(|| format!("tx block {tx_block_hash:?} body missing"))?;
            if let BlockContent::Transaction { txs } = block.content {
                for tx in txs {
                    let tx_hash = *blake3::hash(&tx).as_bytes();
                    if seen.insert(tx_hash) {
                        result.push(tx);
                    }
                }
            }
        }
        Ok((result, timestamp_ms))
    }

    /// DFS over a proposer's `prop_refs`, collecting every reachable
    /// `tx_refs` entry in visitation order. `visited` prevents re-walking a
    /// proposer reachable via multiple paths (the proposer DAG can have
    /// diamonds: two proposers both referencing a common ancestor).
    /// R-5: a proposer body that fails to load is an `Err`, not a skip.
    fn expand_proposer_tx_refs(
        &self,
        chain: &PrismChain,
        level: u64,
        proposer_hash: &H256,
        is_root: bool,
        tx_block_hashes: &mut Vec<H256>,
        visited: &mut HashSet<H256>,
    ) -> Result<(), String> {
        if !visited.insert(*proposer_hash) {
            return Ok(());
        }
        // R-6: stop at an earlier level's confirmed leader (see caller doc).
        if !is_root {
            if let Some(l) = chain.proposer_level(proposer_hash) {
                if l < level
                    && self
                        .db
                        .get_h256(columns::PROPOSER_LEADER_SEQUENCE, &l.to_le_bytes())
                        .ok()
                        .flatten()
                        == Some(*proposer_hash)
                {
                    return Ok(());
                }
            }
        }
        let block = self
            .db
            .get_block(proposer_hash)
            .map_err(|e| format!("db error loading proposer {proposer_hash:?}: {e:?}"))?
            .ok_or_else(|| format!("proposer {proposer_hash:?} body missing"))?;
        if let BlockContent::Proposer { tx_refs, prop_refs } = block.content {
            for r in &tx_refs {
                tx_block_hashes.push(*r);
            }
            for r in &prop_refs {
                self.expand_proposer_tx_refs(chain, level, r, false, tx_block_hashes, visited)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod adversarial_tests;

#[cfg(test)]
mod honest_split_tests;

#[cfg(test)]
mod stage2_latency_tests;
