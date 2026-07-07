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
//! # `delta_d`: paper-faithful formula ported, but not the default
//! `delta_d` -- the per-depth confidence margin used in (33)-(34) -- has an
//! exact closed form in the paper, Appendix E.1 eq. (31), transcribed and
//! verified below as [`paper_delta_d`]/[`paper_k_min`] (checked against the
//! PDF's own selectable embedded text, not OCR/visual guesswork, on
//! 2026-07-07). It is deliberately **not** wired in as
//! [`ConfirmationParams::default`]'s `delta_d`/`k_min`, though: `k_min` is a
//! pure function of the assumed adversary fraction `β` alone, and for any β
//! in a sane range it comes out to several *thousand* -- e.g. ~7,900 at
//! β=0.25, ~13,000 at β=0.3 -- versus the 24-block depth the paper's own
//! p.7 says its much more naive *stage-1* rule needs at the same β=0.3,
//! ε=10⁻³. That's not a transcription error (verified twice, including
//! character-for-character against the PDF's raw text layer); it's the
//! usual gap between an asymptotic concentration-bound proof (the "200"/"32"
//! constants inside are slack from a Chernoff-bound union over an
//! astronomically large round count, not a tuned practical depth) and an
//! operating parameter. Using it literally as stage-2's `k_min` would make
//! stage-2 require ~500x the depth stage-1 needs before it even starts
//! looking at a vote -- strictly *slower* than stage-1, contradicting the
//! paper's own premise that aggregating across `m` chains is the faster
//! path. [`placeholder_delta_d`] remains the actual default: right
//! qualitative shape (shrinks with depth and `m`), usable at the depths this
//! codebase's tests and devnets actually reach. `paper_k_min`/`paper_delta_d`
//! are kept, faithfully computed and unit-tested against the paper's own
//! numbers, for whoever eventually needs the rigorous asymptotic guarantee
//! (e.g. choosing parameters for a long-lived mainnet where thousands of
//! blocks of depth is an acceptable wait).
use argonaut_prism_chain::PrismChain;
use argonaut_primitives_prism::H256;

/// Adversary hash-power assumption (paper's `β`, §4.1): the maximum fraction
/// of total voter mining power the safety analysis tolerates. Not derivable
/// from the paper -- a deployment threat-model choice. `0.25` is a
/// deliberately conservative pick: comfortably under the paper's own most-
/// used worked examples (mostly `β = 0.3`-`0.4`) and its absolute safety
/// limit (`β < 0.5`, with some results proven only up to `1/3`). Change
/// freely -- nothing else here hardcodes it beyond this constant and
/// [`paper_k_min`]/[`paper_delta_d`]'s own default call sites in tests.
pub const ASSUMED_ADVERSARY_FRACTION: f64 = 0.25;

/// Paper's `γ` (Appendix E.1, eq. 31): `(1/36)(1-2β)²`.
fn gamma(beta: f64) -> f64 {
    (1.0 / 36.0) * (1.0 - 2.0 * beta).powi(2)
}

/// Paper's `c1` (eq. 31): `(1-2β)/16`.
fn paper_c1(beta: f64) -> f64 {
    (1.0 - 2.0 * beta) / 16.0
}

/// Paper's `k_min` (eq. 31): `(4/γ)·log(200/(γ·c1))` -- a pure function of
/// `β` alone (no dependence on `m` or the mining rate). See the module docs
/// for why this is *not* used as [`ConfirmationParams::default`]'s `k_min`.
pub fn paper_k_min(beta: f64) -> u64 {
    let g = gamma(beta);
    let c1 = paper_c1(beta);
    ((4.0 / g) * (200.0 / (g * c1)).ln()).ceil().max(1.0) as u64
}

/// Paper's `δ_k` (eq. 31): the per-depth confidence margin, as a fraction of
/// `m` (matching this module's `delta_d` calling convention -- (33)/(34) both
/// use it as `δ_k(k)·m`). Cross-checked against the paper's own note that
/// `δ_k = ρ_r'` under `r' = k/(2f̄_v)`: substituting confirms the two
/// formulas agree algebraically (`c1/(1+4·f̄_v·r')` becomes exactly
/// `c1/(1+2k)` under that substitution).
pub fn paper_delta_d(depth: u64, m: u64, beta: f64) -> f64 {
    let c1 = paper_c1(beta);
    let term_depth = c1 / (1.0 + 2.0 * depth as f64);
    let term_m = ((1.0 - 2.0 * beta) * c1) / (1.0 + 32.0 * (m.max(1) as f64).ln());
    term_depth.max(term_m)
}

/// NOT the paper's derived bound -- see module docs. Returns a margin
/// *fraction* of `m` (multiply by `m` to get a vote-count margin, as (33)
/// does with `delta_d(k)*m`).
pub fn placeholder_delta_d(depth: u64, m: u64) -> f64 {
    1.0 / (((depth + 1) * m.max(1)) as f64).sqrt()
}

pub type DeltaDFn = fn(u64, u64) -> f64;

pub struct ConfirmationParams {
    /// Paper's `k_min`: votes shallower than this never count toward a lower
    /// bound. `1` is a practical default -- see the module docs for why this
    /// is deliberately not [`paper_k_min`]'s literal (much larger) value.
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
