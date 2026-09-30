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
//! path. [`placeholder_delta_d`] was the default through B-15 (below): right
//! qualitative shape (shrinks with depth and `m`), usable at the depths this
//! codebase's tests and devnets actually reach, but not a proven safety
//! margin -- see [`MarginPolicy::Custom`]'s doc for where it still lives (test
//! pins only, post-B-15). `paper_k_min`/`paper_delta_d` are kept, faithfully
//! computed and unit-tested against the paper's own numbers, for whoever
//! eventually needs the rigorous asymptotic guarantee (e.g. choosing
//! parameters for a long-lived mainnet where thousands of blocks of depth is
//! an acceptable wait).
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
//! important -- what it does and doesn't guarantee. As of B-15 (below) this
//! *is* wired in as the default, at the reviewed `ε_target = 1e-6`/check
//! operating point (chain-spec-overridable -- see the B-15 section).
//!
//! # B-15 (2026-07-22): stage-2 on by default, with the real margin
//! Stage-1 (`confirmed_leader_at_level`) requires literally every one of the
//! `m` voter chains to vote -- one censored or offline chain stalls
//! confirmation forever, no matter the depth. Stage-2 is the paper's own
//! designed way around that (it needs a *statistical* majority, not
//! unanimity -- see `stage2_latency_tests.rs`'s B-4 measurements for exactly
//! how much that buys in practice). [`ConfirmationParams::default`] now sets
//! `stage2_enabled: true` with `margin: `[`MarginPolicy::SingleCheck`]` { beta:
//! `[`ASSUMED_ADVERSARY_FRACTION`]`, eps_target: 1e-6 }` -- β and the ε
//! exponent are chain-spec properties in production (`prismBetaPercent`/
//! `prismEpsExponent`, `node/src/chain_spec.rs`), not hardcoded past this
//! crate's default.
//!
//! **Read this before expecting a latency win:** at `m=100` (this repo's
//! `Params::testnet()`) the real margin is *vacuous* below `k≈400` (see
//! [`single_check_margin`]'s doc comment) -- stage-2 simply won't confirm
//! anything shallower than that. So with the real margin, stage-2 is **not**
//! faster than stage-1 in the common case; it's a provably-safe *eventual
//! backstop*: when stage-1 stalls forever on a permanently-silent chain,
//! stage-2 still confirms once the surviving chains' votes get a few hundred
//! blocks deep (hours, not never). When stage-1 works normally, it wins the
//! race in `PrismLedger::advance` and stage-2 costs nothing. Tests that need
//! to exercise stage-2 confirming at shallow depth (the old placeholder's
//! behavior) pin `margin: `[`MarginPolicy::Custom`]`(placeholder_delta_d)`
//! explicitly -- see that variant's doc comment for the list.
//!
//! **ε sizing.** `eps_target` is a *per-check* permanent-split probability
//! (the bridge finalizes with no reorg path -- see [`single_check_margin`]'s
//! "does NOT prove" section for the full derivation of why lifetime risk
//! needs a much smaller value than a single check's). `1e-6`/check
//! (`prismEpsExponent = 6`) is fine for a devnet; a deployment holding real
//! value should raise `prismEpsExponent` to `9`+ -- the margin floor grows
//! only logarithmically with tighter ε (`−ln(ε)/m`: 13.8%→20.7% at `m=100`
//! going `1e-6`→`1e-9`), so this is cheap insurance, not a latency cliff.
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
/// `ε_target`. As of B-15, this operating point (`β=0.25`, `ε_target=1e-6`
/// per check) *is* [`ConfirmationParams::default`]'s margin -- see the module
/// doc's B-15 section for the vacuous-below-`k≈400` caveat and the ε sizing
/// guidance for deployments holding real value.
pub fn single_check_margin(depth: u64, m: u64, beta: f64, eps_target: f64) -> f64 {
    let g = gamma(beta);
    let mu_bar = 4.0 * (-g * depth as f64 / 2.0).exp();
    let l = -eps_target.ln();
    let a = (l + (l * l + 8.0 * m as f64 * l * mu_bar).sqrt()) / (2.0 * m as f64);
    mu_bar + a
}

/// B-15: a ready-to-use [`ConfirmationParams`] at the same `(β=0.25,
/// ε_target=1e-6)` operating point as [`ConfirmationParams::default`] itself
/// (that default *is* this preset, modulo `k_min`) -- kept as a separate,
/// explicitly-named function for callers who want `k_min: 0` rather than the
/// default's `k_min: 1`. `k_min: 0` is safe here because, unlike the paper's
/// `k_min`, [`single_check_margin`] has no hard cutoff below which it's
/// invalid, only a *vacuous* one (`ρ≥1`, see that function's doc comment for
/// the `k≈400` non-vacuous threshold at `m=100`) -- `confirm_unique_leader`
/// simply won't confirm anything while the margin is that large, so no
/// separate `k_min` gate is needed; the shape of the curve does the gating.
///
/// # Before using a *different* operating point than this for production
/// 1. Pick your own `ε_target` from your deployment's real expected
///    confirmation-check count and acceptable lifetime failure budget (see
///    [`single_check_margin`]'s doc comment for the arithmetic) -- `1e-6`
///    here is a devnet-appropriate default, not a mainnet recommendation
///    (see the module doc's ε sizing note).
/// 2. Confirm your deployment's real `m` (`Params::m_voter_chains`, chain-
///    spec-driven -- B-16) and `β` (`prismBetaPercent`, chain-spec-driven --
///    B-15).
/// 3. Have someone independently check the math (or at minimum, run
///    `single_check_margin_tests`'s Monte Carlo cross-check against your own
///    chosen `(m, k, ε_target)`) before treating this as a security boundary.
pub fn example_single_check_confirmation_params_beta025_eps1e6() -> ConfirmationParams {
    ConfirmationParams {
        k_min: 0,
        margin: MarginPolicy::SingleCheck { beta: ASSUMED_ADVERSARY_FRACTION, eps_target: 1e-6 },
        stage2_enabled: true,
    }
}

/// NOT the paper's derived bound -- see module docs. Returns a margin
/// *fraction* of `m` (multiply by `m` to get a vote-count margin, as (33)
/// does with `delta_d(k)*m`). Pre-B-15 this was the operational default
/// ([`MarginPolicy::Custom`]); post-B-15 it survives only as an explicit test
/// pin for tests that specifically exercise stage-2 confirming at shallow
/// depth, where the real ([`MarginPolicy::SingleCheck`]) margin would be
/// vacuous -- see [`MarginPolicy::Custom`]'s doc comment for the list.
pub fn placeholder_delta_d(depth: u64, m: u64) -> f64 {
    1.0 / (((depth + 1) * m.max(1)) as f64).sqrt()
}

pub type DeltaDFn = fn(u64, u64) -> f64;

/// B-15: the confidence margin `ConfirmationParams` uses, as a policy rather
/// than a bare `fn` pointer -- a bare `DeltaDFn` can't capture the runtime
/// `(β, ε_target)` values a chain spec provides (`node/src/chain_spec.rs`'s
/// `prismBetaPercent`/`prismEpsExponent`), only compile-time constants.
#[derive(Clone, Copy)]
pub enum MarginPolicy {
    /// An explicit `fn(depth, m) -> margin_fraction`, bypassing
    /// [`single_check_margin`] entirely. Production code has no reason to use
    /// this post-B-15; it exists so tests that specifically exercise
    /// stage-2's *rule structure* at shallow depths (where the real margin is
    /// vacuous) can pin the old [`placeholder_delta_d`] and say so explicitly
    /// -- the 4 `confirm_unique_leader` tests in `confirmation/tests.rs`,
    /// both `stage2_latency_tests.rs` tests,
    /// `persistent_balancing_attacker_stalls_stage1_alone_but_not_the_system`
    /// (`adversarial_tests.rs`), and
    /// `any_real_skew_in_an_honest_split_still_resolves_via_stage_2`
    /// (`honest_split_tests.rs`).
    Custom(DeltaDFn),
    /// [`single_check_margin`]'s derived, Monte-Carlo-validated margin (B-3),
    /// parametrized by the deployment's assumed adversary fraction and
    /// per-check target failure probability. [`ConfirmationParams::default`]
    /// uses this at `(β=0.25, ε_target=1e-6)` -- see the module doc's B-15
    /// section for what this does and doesn't guarantee.
    SingleCheck { beta: f64, eps_target: f64 },
}

impl MarginPolicy {
    fn margin(&self, depth: u64, m: u64) -> f64 {
        match *self {
            MarginPolicy::Custom(f) => f(depth, m),
            MarginPolicy::SingleCheck { beta, eps_target } =>
                single_check_margin(depth, m, beta, eps_target),
        }
    }
}

pub struct ConfirmationParams {
    /// Paper's `k_min`: votes shallower than this never count toward a lower
    /// bound. `1` is a practical default -- see the module docs for why this
    /// is deliberately not [`paper_k_min`]'s literal (much larger) value.
    pub k_min: u64,
    pub margin: MarginPolicy,
    /// Gates [`confirm_unique_leader`] entirely. **On by default (B-15).**
    /// `advance` (`client/prism-ledger/src/lib.rs`) tries stage-2 *first*,
    /// falling back to stage-1's conservative k-deep rule only if stage-2
    /// returns `None`. With the real [`MarginPolicy::SingleCheck`] margin (the
    /// default `margin`), this is safe to leave on: the module doc's B-15
    /// section explains why it's a backstop for stage-1's unanimity weakness
    /// rather than a latency win. Set `false` to fall back to stage-1 alone
    /// (e.g. `--prism-no-stage2`, `node/src/cli.rs`), or to isolate
    /// stage-1-only latency measurements the way B-4 originally did.
    pub stage2_enabled: bool,
}

impl Default for ConfirmationParams {
    fn default() -> Self {
        ConfirmationParams {
            k_min: 1,
            margin: MarginPolicy::SingleCheck { beta: ASSUMED_ADVERSARY_FRACTION, eps_target: 1e-6 },
            stage2_enabled: true,
        }
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
            let margin = params.margin.margin(k, m) * m as f64;
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
/// list member, once its own lower bound alone already exceeds the best case
/// of every rival it could still lose to -- at that point no future event can
/// change the outcome. "Every rival" is two distinct things, and both are
/// checked below:
/// 1. each *visible* competing proposer at this level, via its `v_upper`; and
/// 2. the *invisible* one -- a proposer mined and withheld, bounded by eq.
///    (35)'s `m − Σ lower bounds`, identically to [`confirm_proposer_list`].
///
/// Only (1) was checked until 2026-07-28. Because a withheld block is by
/// definition absent from `proposers_at_level`, omitting (2) meant a level
/// with a single *visible* proposer compared against no rivals at all
/// (`max().unwrap_or(0)`) and so confirmed on any lower bound whatsoever --
/// see `confirmation/tests.rs`'s
/// `a_unique_leader_is_never_confirmed_while_a_private_proposer_could_still_beat_it`
/// for the concrete case where this rule and [`confirm_proposer_list`]
/// disagreed about whether a level was confirmable at all.
pub fn confirm_unique_leader(chain: &PrismChain, level: u64, params: &ConfirmationParams) -> Option<H256> {
    // B-15: on by default -- see `ConfirmationParams::stage2_enabled`'s doc
    // comment, and the module doc's B-15 section for why the real derived
    // margin makes that safe.
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

    // Eq. (35)'s private-block bound, exactly as `confirm_proposer_list`
    // applies it -- and load-bearing here for the same reason. A proposer
    // this node has never seen (mined and deliberately withheld) is absent
    // from `proposers_at_level`, so it contributes nothing to `bounds`; the
    // votes it could still end up holding are precisely the ones no *visible*
    // proposer has provably locked in. Without this term the rule compares a
    // candidate only against rivals it can see, and in the single-visible-
    // proposer case `rivals_max_upper` degenerates to `unwrap_or(0)` -- so any
    // lower bound at all would "uniquely" win a level a withheld block could
    // still take. That is unsound anywhere, but especially here: `advance`
    // hands confirmed levels to `node/src/bridge.rs`, which imports each one
    // `finalized: true` with no reorg path, so a leader beaten after the fact
    // is a permanent split rather than a recoverable one.
    let v_bar_private = m.saturating_sub(bounds.iter().map(|&(_, lo, _)| lo).sum());

    bounds.iter().find_map(|&(p, lo, _)| {
        let rivals_max_upper =
            bounds.iter().filter(|&&(q, _, _)| q != p).map(|&(_, _, hi)| hi).max().unwrap_or(0);
        (lo > rivals_max_upper.max(v_bar_private)).then_some(p)
    })
}

#[cfg(test)]
mod tests;
