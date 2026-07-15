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
//!
//! # B-3 (2026-07-14): a practical middle ground, [`single_check_margin`]
//! The "astronomically large round count" the previous paragraph refers to is
//! specifically the paper's union bound over its `r_max²` possible `(r, r')`
//! pairs (`Prism.pdf` p.29's proof), needed because the paper proves `T` holds
//! *for the whole run, uniformly*. `confirm_unique_leader` doesn't need that
//! strength -- it re-checks every round anyway. Re-solving the paper's own
//! cited Chernoff bound (eq. 32) directly for a single check's target failure
//! probability, without the union step, gives a real, Monte-Carlo-validated
//! margin at depths in the low thousands rather than ~7,900-13,000. See
//! [`single_check_margin`]'s doc for the derivation, the numbers, and --
//! important -- what it does and doesn't guarantee. Still not wired in as a
//! default; needs a reviewed `ε_target` choice first.
//!
//! # B-15: this is what unblocks fixing stage-1's unanimity weakness
//! Stage-1 (`confirmed_leader_at_level`) requires literally every one of the
//! `m` voter chains to vote -- one censored or offline chain stalls
//! confirmation forever, no matter the depth. Stage-2 is the paper's own
//! designed way around that (it needs a *statistical* majority, not
//! unanimity -- see `stage2_latency_tests.rs`'s B-4 measurements for exactly
//! how much that buys in practice), but only once its confidence margin is
//! real. [`example_single_check_confirmation_params_beta025_eps1e6`] is a
//! ready-to-use (if reviewed) `ConfirmationParams` built on
//! [`single_check_margin`] -- B-15 is "resolved" in the sense that the tool
//! now exists; actually turning it on for a given deployment is still a
//! reviewed decision, not a default this crate makes for you.
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

/// B-3: a *practical* confidence margin, derived (not guessed) from the same
/// underlying Chernoff bound the paper cites (its eq. (32), `Prism.pdf` p.28) --
/// but solved directly for a single confirmation check, instead of unioned
/// over the paper's `r_max²` possible `(r, r')` pairs the way `paper_delta_d`
/// effectively is (that union is what makes [`paper_k_min`] come out to
/// ~7,900+ for reasonable `β`; see the module docs).
///
/// # What this proves, precisely
/// Per eq. (32): for `X̄ = (1/m)Σ 1(E_j^c[r-r',r])`, a mean `μ ≤ 4·e^{-γf̄_v r'}`,
/// and any `a ≥ 0`: `P{X̄ ≥ μ+a} ≤ e^{-ma²/(a+2μ)}`. Substituting `r' = k/(2f̄_v)`
/// (the same substitution [`paper_delta_d`]'s doc comment verifies -- `f̄_v`
/// cancels out entirely) gives `μ_bar(k) = 4·e^{-γk/2}`. Solving
/// `e^{-ma²/(a+2·μ_bar)} ≤ ε_target` for the smallest valid `a` (a quadratic
/// in `a`: `m·a² − L·a − 2·L·μ_bar ≥ 0` where `L = -ln(ε_target)`, positive
/// root `a = (L + √(L² + 8mLμ_bar)) / (2m)`) gives the tightest margin
/// `ρ = μ_bar + a` such that **this one check**, at this one `(m, k, β)`,
/// fails with probability at most `ε_target`. Cross-checked by Monte Carlo
/// simulation against the literal Bernoulli-sum tail (see `tests.rs`).
///
/// # What this does NOT prove (read before using as a network default)
/// This is a **per-check** bound, not the paper's whole-history bound.
/// `confirm_unique_leader` is called fresh every round (`PrismLedger::advance`,
/// once per bridge wakeup, so up to once per confirmed level): each call is an
/// *independent* chance for a check to be wrong at rate `ε_target`, not one
/// bound that, once true, stays true forever the way the paper's `T` event
/// does. For an actual deployment-lifetime guarantee, union this over the
/// realistic number of confirmation attempts the deployment will ever make
/// (which is `O(number of confirmed levels)`, NOT the paper's `O(r_max²)` --
/// unioning over every possible *pair* of rounds is what made the paper's
/// bound need such enormous depth for a claim far stronger than what a
/// running system actually needs). E.g. for 10 years at 1 confirmed level/sec
/// (~3×10^8 levels, a generous upper bound), an overall lifetime failure
/// budget of 10^-6 needs `ε_target ≈ 10^-6 / 3×10^8 ≈ 3×10^-15` per check --
/// pick `ε_target` this way, don't use `1e-6` as a per-check value and assume
/// it's also the lifetime figure.
///
/// # Illustrative numbers (β=0.25, ε_target=1e-6 PER CHECK, not lifetime)
/// - `m=100` (this repo's `Params::testnet()`): margin bottoms out around
///   13.8% of `m` (`k≈3000`+) -- `4×e^{-γk/2}` itself doesn't drop below 1
///   (the point at which the bound stops being vacuous) until `k≈400`.
/// - `m=1000` (paper §4.2's intended mainnet scale): `k=1500` → ~5.4% margin,
///   `k=2000` → ~2.3% margin -- markedly better, because Chernoff
///   concentration is inherently a function of `m`, not just depth.
///
/// This is real progress over `paper_delta_d`/`paper_k_min` (getting a stated,
/// checked probability *at all*, at a depth in the low thousands instead of
/// ~13,000, is a genuine improvement) but it is **not** "confirm in tens of
/// blocks" -- that would need either a much larger `m` or accepting a looser
/// `ε_target`. Treat this as a real, numerically-validated building block for
/// a production operating point, not a drop-in replacement for
/// [`placeholder_delta_d`] -- deliberately not wired in as
/// [`ConfirmationParams::default`]; needs an explicit, reviewed choice of
/// `ε_target` and the deployment's real expected round count first (see
/// `docs/IMPLEMENTATION_AB_FIXPLAN.md`'s B-3 entry).
pub fn single_check_margin(depth: u64, m: u64, beta: f64, eps_target: f64) -> f64 {
    let g = gamma(beta);
    let mu_bar = 4.0 * (-g * depth as f64 / 2.0).exp();
    let l = -eps_target.ln();
    let a = (l + (l * l + 8.0 * m as f64 * l * mu_bar).sqrt()) / (2.0 * m as f64);
    mu_bar + a
}

/// B-15: [`ConfirmationParams::delta_d`] is a plain `fn(u64, u64) -> f64`
/// (no closures -- see the type's own definition), so a specific
/// [`single_check_margin`] operating point needs a *named* function with
/// exactly that two-argument shape to actually be pluggable. This is the
/// EXAMPLE illustrated in [`single_check_margin`]'s own doc comment
/// (`β=0.25`, `ε_target=1e-6` per check) -- not a recommendation, not wired
/// in anywhere. `k_min` in the returned [`ConfirmationParams`] is `0`
/// deliberately: unlike the paper's `k_min`, this formula has no hard cutoff
/// below which it's invalid, only a *vacuous* one (`ρ≥1`, see the doc
/// comment's `k≈400` non-vacuous threshold at m=100) -- `confirm_unique_leader`
/// simply won't confirm anything while the margin is that large, so no
/// separate gate is needed; the shape of the curve does the gating.
///
/// # Before using this for anything but experimentation
/// 1. Pick your own `ε_target` from your deployment's real expected
///    confirmation-check count and acceptable lifetime failure budget (see
///    [`single_check_margin`]'s doc comment for the arithmetic) -- `1e-6`
///    here is illustrative, not a recommendation.
/// 2. Confirm your deployment's real `m` (`Params::m_voter_chains`, now
///    chain-spec-driven -- B-16) and re-derive the constant below for it;
///    this example is hardcoded to `β=0.25`, matching
///    [`crate::confirmation::ASSUMED_ADVERSARY_FRACTION`].
/// 3. Have someone independently check the math (or at minimum, run
///    `single_check_margin_tests`'s Monte Carlo cross-check against your own
///    chosen `(m, k, ε_target)`) before treating this as a security boundary.
pub fn example_single_check_confirmation_params_beta025_eps1e6() -> ConfirmationParams {
    fn delta_d_beta025_eps1e6(depth: u64, m: u64) -> f64 {
        single_check_margin(depth, m, ASSUMED_ADVERSARY_FRACTION, 1e-6)
    }
    // stage2_enabled: true -- this is a self-contained, explicit choice at
    // whatever call site plugs it in (e.g. via `advance_with_params`
    // directly), separate from the bridge's own `--prism-stage2` CLI switch
    // (which flips `ConfirmationParams::default()`'s flag, not this one). A
    // caller reaching for this function by name has already opted in.
    ConfirmationParams { k_min: 0, delta_d: delta_d_beta025_eps1e6, stage2_enabled: true }
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
    /// B-12: gates [`confirm_unique_leader`] entirely. **Off by default.**
    /// `advance` (`client/prism-ledger/src/lib.rs`) tries stage-2 *first*,
    /// falling back to stage-1's conservative k-deep rule only if stage-2
    /// returns `None` -- but stage-2's live `delta_d` here is
    /// [`placeholder_delta_d`], which the module doc above says explicitly
    /// is **not a real safety margin** (the paper's exact `delta_d`/`k_min`,
    /// [`paper_delta_d`]/[`paper_k_min`], are impractically deep to use as
    /// the default -- see the module doc's "not the default" section). So
    /// with stage-2 live, the bridge's irreversible finality (every
    /// confirmed level is imported `finalized: true`, no reorg path) rested
    /// on an unproven confidence margin. Until a real operating point for
    /// `delta_d`/`k_min` is derived (B-3), stage-1-only is the safe default.
    /// Also doubles as the B-4 measurement toggle: flip this to compare
    /// confirmed-level latency with stage-2 on vs. off.
    pub stage2_enabled: bool,
}

impl Default for ConfirmationParams {
    fn default() -> Self {
        ConfirmationParams { k_min: 1, delta_d: placeholder_delta_d, stage2_enabled: false }
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
    // B-12: off by default -- see `ConfirmationParams::stage2_enabled`'s doc
    // comment for why an unproven confidence margin must not gate finality.
    if !params.stage2_enabled {
        return None;
    }
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
