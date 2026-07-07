//! Stage-2 fast confirmation (architecture guide §3.8; Prism paper §4.5 "Fast
//! confirmation latency", Definition 4.5 "Proposer set confirmation policy";
//! cross-verified against Appendix E.2 "Fast list confirmation policy",
//! equations (33)-(36)).
//!
//! The idea (paper Fig. 9): keep a confidence interval `[V_n(r), V̄_n(r)]`
//! around the final vote count each contending proposer `p_n` at a level will
//! end up with, and one for a potential still-hidden private (adversary-
//! withheld) block. Once the private block's upper bound falls below the
//! largest lower bound among public proposers, the set of proposers whose
//! upper bound could still catch up to that leading lower bound is
//! *confirmed* as `Π_ℓ(r)` (Definition 4.5) -- guaranteed to contain whichever
//! proposer eventually wins the level, even though which *one* of them wins
//! may not be settled yet.
//!
//! Formulas, transcribed from Prism.pdf p.11 (§4.5.1) and cross-checked
//! against the appendix restatement on p.30 (verified verbatim against the
//! PDF's own text layer for eq. 33-34, not just visual transcription):
//!
//! ```text
//! V_n(r)      := max_{k>=k_min} (V_n^k(r) - delta_d(k)*m)_+                    (33)
//! V̄_n(r)      := V_n(r) + (V_{-n}^raw(r) - V_{-n}(r)) + U_l(r)                 (34)
//! V_private(r) := 0
//! V̄_private(r) := m - sum_{p_n in P_l(r)} V_n(r)                              (35)
//! Definition 4.5: if max_n V_n(r) > V̄_private(r), confirm
//!   Π_l(r) := { p_i : V̄_i(r) > max_n V_n(r) }                                 (36)
//! ```
//! where `V_n^k(r)` is the number of votes at depth >= k for `p_n`, `V_{-n}^k(r)`
//! is the same for every *other* proposer at the level pooled together,
//! `V_{-n}^raw(r)` is that pool's undiscounted count (k=0), and `U_l(r)` is the
//! number of voter chains that haven't voted on the level at all yet.
//!
//! # Known gap, flagged explicitly rather than guessed
//! `delta_d` -- the per-depth confidence margin used in (33)-(34) -- has an
//! exact closed form in the paper, but deriving it requires Appendix E.1's
//! "typicality" lemma: a multi-page concentration bound over the Poisson
//! voter-chain mining process (the constant `rho_delta_r`, threshold `k_min`,
//! and the macroscopic chain-growth/common-prefix/chain-quality lemmas E.2-E.4
//! that feed it). That derivation was not ported this session -- getting a
//! consensus-safety margin subtly wrong by mistranscribing dense inline math
//! is worse than not shipping it. [`PLACEHOLDER_DELTA_D`] is an explicit,
//! swappable stand-in with the right *qualitative* shape (shrinks with depth
//! and with `m`) so the confirmation *policy* above -- which is verified
//! faithful -- can be implemented and tested today. Do not treat its exact
//! output as a safety guarantee; replace it once Appendix E.1 is ported.
use argonaut_prism_chain::PrismChain;
use argonaut_primitives_prism::H256;

/// NOT the paper's derived bound -- see module docs. Returns a margin
/// *fraction* of `m` (multiply by `m` to get a vote-count margin, as (33)
/// does with `delta_d(k)*m`).
pub fn placeholder_delta_d(depth: u64, m: u64) -> f64 {
    1.0 / (((depth + 1) * m.max(1)) as f64).sqrt()
}

pub type DeltaDFn = fn(u64, u64) -> f64;

pub struct ConfirmationParams {
    /// Paper's `k_min`: votes shallower than this never count toward a lower
    /// bound. 1 is a conservative default in the absence of the paper's own
    /// derived value (see module docs).
    pub k_min: u64,
    pub delta_d: DeltaDFn,
}

impl Default for ConfirmationParams {
    fn default() -> Self {
        ConfirmationParams { k_min: 1, delta_d: placeholder_delta_d }
    }
}

/// For every voter chain that cast a vote for `proposer`, how many blocks
/// behind that chain's current tip the casting block is (i.e. `V_n^k(r)`'s
/// per-chain depth input). `PrismChain::votes` records the *absolute* depth
/// the vote was cast at (`add_vote`'s `depth` parameter); the confidence
/// bounds need depth-from-tip, matching `confirmed_leader_at_level`'s own
/// `tip_depth.saturating_sub(depth)`.
fn vote_depths_from_tip(chain: &PrismChain, proposer: H256) -> Vec<u64> {
    let Some(info) = chain.votes.get(&proposer) else { return Vec::new() };
    info.per_chain
        .iter()
        .map(|(&chain_idx, &cast_depth)| {
            let tip_depth = chain.voter_tips[chain_idx as usize].1;
            tip_depth.saturating_sub(cast_depth)
        })
        .collect()
}

/// `max_{k>=k_min} (count(depths >= k) - delta_d(k)*m)_+` -- shared by both
/// a single proposer's own lower bound (33) and the pooled complement-set
/// lower bound needed inside (34).
fn lower_bound_from_depths(depths: &[u64], params: &ConfirmationParams, m: u64) -> u64 {
    let max_depth = depths.iter().copied().max().unwrap_or(0);
    (params.k_min..=max_depth.max(params.k_min))
        .map(|k| {
            let raw = depths.iter().filter(|&&d| d >= k).count() as f64;
            let margin = (params.delta_d)(k, m) * m as f64;
            (raw - margin).max(0.0)
        })
        .fold(0.0_f64, f64::max)
        .floor() as u64
}

fn v_lower(chain: &PrismChain, proposer: H256, params: &ConfirmationParams, m: u64) -> u64 {
    lower_bound_from_depths(&vote_depths_from_tip(chain, proposer), params, m)
}

fn v_upper(
    chain: &PrismChain,
    proposer: H256,
    others: &[H256],
    level: u64,
    params: &ConfirmationParams,
    m: u64,
) -> u64 {
    let own_lower = v_lower(chain, proposer, params, m);
    let other_depths: Vec<u64> =
        others.iter().flat_map(|&op| vote_depths_from_tip(chain, op)).collect();
    let others_raw = other_depths.len() as u64;
    let others_lower = lower_bound_from_depths(&other_depths, params, m);
    let m_chains = chain.params.m_voter_chains as usize;
    let u_level = (0..m_chains).filter(|&c| chain.unvoted_levels[c].contains_key(&level)).count() as u64;
    own_lower + others_raw.saturating_sub(others_lower) + u_level
}

/// Definition 4.5's "proposer set confirmation policy" (eq. 36): the list of
/// proposers at `level` that could still end up as the leader, guaranteed
/// (w.p. implied by `delta_d`'s -- currently placeholder -- confidence level)
/// to contain whichever proposer eventually wins. Empty if the level has no
/// proposers yet, or if the private-block upper bound (eq. 35) hasn't yet
/// been beaten by the best public lower bound (the gate in eq. 14/Def. 4.5).
pub fn confirm_proposer_list(chain: &PrismChain, level: u64, params: &ConfirmationParams) -> Vec<H256> {
    let m = chain.params.m_voter_chains as u64;
    let proposers = chain.proposers_at_level(level);
    if proposers.is_empty() {
        return Vec::new();
    }

    let lowers: Vec<(H256, u64)> =
        proposers.iter().map(|&p| (p, v_lower(chain, p, params, m))).collect();
    let best_lower = lowers.iter().map(|&(_, l)| l).max().unwrap_or(0);

    let v_bar_private = m.saturating_sub(lowers.iter().map(|&(_, l)| l).sum());
    if best_lower <= v_bar_private {
        return Vec::new();
    }

    proposers
        .iter()
        .filter(|&&p| {
            let others: Vec<H256> = proposers.iter().copied().filter(|&x| x != p).collect();
            v_upper(chain, p, &others, level, params, m) > best_lower
        })
        .copied()
        .collect()
}

/// Strengthening beyond the paper's own list-confirmation (per architecture
/// guide §3.8): a proposer is confirmed as the *unique* leader, not just a
/// list member, once its own lower bound alone already exceeds every rival's
/// upper bound -- at that point no future event (more votes, a revealed
/// private block) can change the outcome, since even each rival's best case
/// falls short of what this proposer has already locked in.
pub fn confirm_unique_leader(chain: &PrismChain, level: u64, params: &ConfirmationParams) -> Option<H256> {
    let m = chain.params.m_voter_chains as u64;
    let proposers = chain.proposers_at_level(level);
    if proposers.len() < 1 {
        return None;
    }

    let bounds: Vec<(H256, u64, u64)> = proposers
        .iter()
        .map(|&p| {
            let others: Vec<H256> = proposers.iter().copied().filter(|&x| x != p).collect();
            let lo = v_lower(chain, p, params, m);
            let hi = v_upper(chain, p, &others, level, params, m);
            (p, lo, hi)
        })
        .collect();

    bounds.iter().find_map(|&(p, lo, _)| {
        let rivals_max_upper =
            bounds.iter().filter(|&&(q, _, _)| q != p).map(|&(_, _, hi)| hi).max().unwrap_or(0);
        (lo > rivals_max_upper).then_some(p)
    })
}

#[cfg(test)]
mod tests;
