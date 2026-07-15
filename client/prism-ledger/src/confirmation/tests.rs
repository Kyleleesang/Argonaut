//! Stage-2 confirmation tests. `lower_bound_from_depths` is tested directly
//! against hand-computed values (it's the one place `placeholder_delta_d`'s
//! exact numbers matter); `confirm_proposer_list`/`confirm_unique_leader` are
//! tested end-to-end against a real `PrismChain` built by inserting actual
//! proposer/voter blocks, not by injecting synthetic vote-depth vectors --
//! the goal is to prove the whole pipeline (real per-chain vote depths ->
//! confidence bounds -> Definition 4.5's gate) works, matching this session's
//! practice throughout B0-B4 of exercising real state machines over mocks.
use super::*;
use argonaut_primitives_prism::{
    block_hash, BlockContent, ChainId, MerkleProof, Params, PrismBlock, SuperHeader,
};
use sp_core::U256;

fn params(m: u16) -> Params {
    let mut p = Params::testnet();
    p.m_voter_chains = m;
    p
}

fn header(nonce: u64) -> SuperHeader {
    SuperHeader { parents_root: H256::zero(), content_root: H256::zero(), timestamp_ms: 0, nonce, difficulty: U256::max_value() }
}

fn proposer_block(nonce: u64, parent: H256) -> PrismBlock {
    PrismBlock {
        header: header(nonce),
        parent,
        content: BlockContent::Proposer { tx_refs: vec![], prop_refs: vec![] },
        parent_proof: MerkleProof::default(),
        content_proof: MerkleProof::default(),
    }
}

fn voter_block(nonce: u64, chain: ChainId, voter_parent: H256, votes: Vec<H256>) -> PrismBlock {
    PrismBlock {
        header: header(nonce),
        parent: voter_parent,
        content: BlockContent::Voter { chain, voter_parent, votes },
        parent_proof: MerkleProof::default(),
        content_proof: MerkleProof::default(),
    }
}

/// Extend `chain_idx`'s voter chain by `count` empty (voteless) blocks,
/// starting from its current tip, returning the new tip hash.
fn extend(chain: &mut PrismChain, chain_idx: ChainId, count: u64, nonce: &mut u64) -> H256 {
    let mut tip = chain.voter_tips[chain_idx as usize].0;
    for _ in 0..count {
        *nonce += 1;
        let b = voter_block(*nonce, chain_idx, tip, vec![]);
        tip = block_hash(&b.header);
        chain.insert_block(b);
    }
    tip
}

/// B-3: `paper_k_min`/`paper_delta_d` (Appendix E.1, eq. 31) against hand
/// computations from the paper's own transcribed formula -- not testing
/// against `ConfirmationParams::default()` (which deliberately still uses
/// the practical placeholder; see this module's doc comment for why).
mod paper_formula_tests {
    use super::*;

    #[test]
    fn paper_k_min_is_thousands_not_tens() {
        // The whole point of the module doc's warning: this is not a small,
        // practically-usable depth threshold at any sane beta.
        assert!(paper_k_min(0.25) > 5_000, "k_min(0.25) = {}", paper_k_min(0.25));
        assert!(paper_k_min(0.3) > 10_000, "k_min(0.3) = {}", paper_k_min(0.3));
    }

    #[test]
    fn paper_k_min_grows_as_beta_approaches_one_half() {
        // (1-2beta) -> 0 as beta -> 0.5, shrinking gamma/c1 and blowing up
        // k_min -- a higher adversary-power assumption costs a deeper floor.
        assert!(paper_k_min(0.4) > paper_k_min(0.3));
        assert!(paper_k_min(0.3) > paper_k_min(0.25));
    }

    #[test]
    fn paper_delta_d_shrinks_with_depth() {
        let beta = ASSUMED_ADVERSARY_FRACTION;
        assert!(paper_delta_d(10, 100, beta) > paper_delta_d(1000, 100, beta));
    }

    #[test]
    fn paper_delta_d_shrinks_with_m_once_the_m_term_dominates() {
        // delta_k is a max of two terms, one depth-only (c1/(1+2k)) and one
        // m-only ((1-2beta)c1/(1+32 ln m)) -- `m` only visibly affects the
        // result once depth is large enough that the depth term has decayed
        // below the m term. At shallow depth the depth term alone can
        // dominate regardless of `m`, which isn't a bug (see this module's
        // docs on why k_min gates out shallow depths in the first place).
        let beta = ASSUMED_ADVERSARY_FRACTION;
        assert!(paper_delta_d(100, 10, beta) > paper_delta_d(100, 1000, beta));
    }

    #[test]
    fn paper_delta_d_matches_hand_computed_value_at_zero_depth() {
        // At depth=0: term_depth = c1/(1+0) = c1. c1(0.25) = 0.5/16 = 0.03125.
        // term_m = (1-2*0.25)*c1 / (1+32*ln(100)) = 0.5*0.03125 / (1+32*4.6052)
        //        = 0.015625 / 148.365 ~= 0.0001053 -- far smaller than term_depth.
        let got = paper_delta_d(0, 100, 0.25);
        assert!((got - 0.03125).abs() < 1e-9, "got {got}");
    }

    #[test]
    fn default_confirmation_params_are_unaffected_by_the_paper_formula_existing() {
        // The practical default must still be the placeholder, not the
        // paper's literal (impractically deep) constants -- see module docs.
        let params = ConfirmationParams::default();
        assert_eq!(params.k_min, 1);
        assert_eq!((params.delta_d)(5, 20), placeholder_delta_d(5, 20));
    }
}

/// B-3: Monte Carlo cross-check of [`single_check_margin`] against a literal
/// simulation of the Bernoulli sum the Chernoff bound (eq. 32) is bounding --
/// an independent check on the algebra (quadratic-formula solve for `a`), not
/// just "it compiles and returns a number". Uses a fixed seed for
/// reproducibility; a real bug in the derivation should make the empirical
/// violation rate blow past the claimed `eps_target`, not hover near it, so
/// this has real power to catch a mistake despite the modest trial count
/// tests can afford to run.
mod single_check_margin_tests {
    use super::*;
    use rand_chacha::ChaCha20Rng;
    use rand_core::{RngCore, SeedableRng};

    /// `4*e^{-gamma(beta)*k/2}` -- the same worst-case upper bound on the
    /// per-chain "atypical" probability [`single_check_margin`] uses
    /// internally, exposed here so the Monte Carlo simulates from EXACTLY the
    /// mean the bound is derived against (the true worst case: `p == mu_bar`,
    /// not some arbitrary smaller value that would make the check too easy).
    fn mu_bar(k: u64, beta: f64) -> f64 {
        let g = (1.0 / 36.0) * (1.0 - 2.0 * beta).powi(2);
        4.0 * (-g * k as f64 / 2.0).exp()
    }

    /// Uniform f64 in [0, 1) from 53 random bits -- the standard technique
    /// (same one `rand`'s own `gen::<f64>()` uses internally), so this
    /// doesn't need to pull in the full `rand` crate just for one call site.
    fn uniform_f64(rng: &mut ChaCha20Rng) -> f64 {
        (rng.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Simulate `m` i.i.d. Bernoulli(`p`) draws `trials` times, return the
    /// fraction of trials where the sample mean reaches `rho`.
    fn empirical_violation_rate(m: u64, p: f64, rho: f64, trials: u32, seed: u64) -> f64 {
        let mut rng = ChaCha20Rng::seed_from_u64(seed);
        let mut violations = 0u32;
        for _ in 0..trials {
            let count = (0..m).filter(|_| uniform_f64(&mut rng) < p).count() as u64;
            if (count as f64) / (m as f64) >= rho {
                violations += 1;
            }
        }
        violations as f64 / trials as f64
    }

    #[test]
    fn single_check_margin_empirically_holds_at_several_m_k_combinations() {
        let beta = ASSUMED_ADVERSARY_FRACTION;
        let eps_target = 1e-4; // loose enough to empirically validate with a modest trial count
        let trials = 200_000u32;

        for &(m, k) in &[(100u64, 1_500u64), (100, 2_000), (1_000, 1_000), (1_000, 1_500)] {
            let rho = single_check_margin(k, m, beta, eps_target);
            let p = mu_bar(k, beta);
            let empirical = empirical_violation_rate(m, p, rho, trials, 42);
            // Give a generous slack multiple (not a tight equality) -- Monte
            // Carlo noise at 200k trials on a 1e-4 target has real variance;
            // this is a "the derivation isn't badly wrong" check, not a
            // precision instrument. A genuine algebra bug (e.g. dropping the
            // factor of 2, or solving the wrong root of the quadratic) blows
            // this up by orders of magnitude, which this comfortably catches.
            assert!(
                empirical <= eps_target * 5.0,
                "m={m} k={k}: empirical violation rate {empirical} far exceeds \
                 target {eps_target} (rho={rho}, mu_bar={p}) -- single_check_margin's \
                 derivation likely has a real bug"
            );
        }
    }

    #[test]
    fn single_check_margin_is_between_zero_and_one_for_practical_depths() {
        // A margin >= 1 (subtracting >= 100% of m) is a vacuous bound, not
        // wrong per se, but any (m, k, eps) combination meant to be used in
        // practice must clear this floor.
        let beta = ASSUMED_ADVERSARY_FRACTION;
        assert!(single_check_margin(2_000, 100, beta, 1e-6) < 1.0);
        assert!(single_check_margin(1_500, 1_000, beta, 1e-6) < 1.0);
    }

    #[test]
    fn single_check_margin_shrinks_as_m_grows_at_fixed_depth_and_target() {
        // Chernoff concentration is a function of m, not just depth -- more
        // voter chains means a tighter margin at the SAME k, matching the
        // module doc's m=100 vs m=1000 comparison.
        let beta = ASSUMED_ADVERSARY_FRACTION;
        assert!(
            single_check_margin(1_500, 1_000, beta, 1e-6)
                < single_check_margin(1_500, 100, beta, 1e-6)
        );
    }

    /// B-15: the example preset is correctly wired -- `stage2_enabled` is on
    /// (self-contained, usable as-is if plugged into `advance_with_params`)
    /// and `delta_d` actually delegates to `single_check_margin` at the
    /// documented `(β=0.25, ε_target=1e-6)` point, not some other formula.
    #[test]
    fn example_single_check_confirmation_params_is_wired_correctly() {
        let params = example_single_check_confirmation_params_beta025_eps1e6();
        assert!(params.stage2_enabled, "must be immediately usable as-is");
        assert_eq!(params.k_min, 0);
        for &(depth, m) in &[(1_500u64, 100u64), (2_000, 1_000)] {
            assert_eq!(
                (params.delta_d)(depth, m),
                single_check_margin(depth, m, ASSUMED_ADVERSARY_FRACTION, 1e-6),
                "delta_d must delegate to single_check_margin at the documented operating point"
            );
        }
    }
}

mod lower_bound_from_depths_tests {
    use super::*;

    #[test]
    fn empty_depths_gives_zero() {
        let params = ConfirmationParams::default();
        assert_eq!(lower_bound_from_depths(&[], &params, 6), 0);
    }

    #[test]
    fn shallow_single_vote_is_swamped_by_its_own_margin() {
        let params = ConfirmationParams::default();
        // depth-from-tip 0: at k=1 (k_min), raw=0 already since 0 < 1.
        assert_eq!(lower_bound_from_depths(&[0], &params, 6), 0);
    }

    #[test]
    fn five_deep_votes_matches_hand_computed_value() {
        let params = ConfirmationParams::default();
        // At k=9 (the deepest k where all 5 still count), margin =
        // placeholder_delta_d(9, 6) * 6 = 6/sqrt(10*6) ~= 0.7746.
        // floor(5 - 0.7746) = 4.
        assert_eq!(lower_bound_from_depths(&[9, 9, 9, 9, 9], &params, 6), 4);
    }
}

/// The Figure 9 (Prism.pdf p.11)-style scenario: a leading proposer with
/// many deep (locked-in) votes versus a contending sibling with a single
/// shallow one. Built via real block insertion, m=6 total voter chains: 5
/// vote p1 and then extend past it (depth-from-tip 9), 1 votes p2 and stops
/// (depth-from-tip 0) -- every chain has voted, so `U_l(r) = 0` throughout.
fn build_contested_level() -> (PrismChain, H256, H256) {
    let mut chain = PrismChain::new(params(6));
    let genesis = chain.proposer_genesis;
    let mut nonce = 1000u64;

    let p1 = proposer_block(1, genesis);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);
    let p2 = proposer_block(2, genesis);
    let p2_hash = block_hash(&p2.header);
    chain.insert_block(p2);

    for c in 0..5u16 {
        nonce += 1;
        let vote_block = voter_block(nonce, c, chain.voter_genesis[c as usize], vec![p1_hash]);
        chain.insert_block(vote_block);
        extend(&mut chain, c, 9, &mut nonce);
    }
    nonce += 1;
    chain.insert_block(voter_block(nonce, 5, chain.voter_genesis[5], vec![p2_hash]));

    (chain, p1_hash, p2_hash)
}

#[test]
fn confirm_proposer_list_excludes_the_far_behind_sibling() {
    let (chain, p1_hash, _p2_hash) = build_contested_level();
    let params = ConfirmationParams::default();
    assert_eq!(confirm_proposer_list(&chain, 1, &params), vec![p1_hash]);
}

#[test]
fn confirm_unique_leader_declares_p1_once_its_lower_bound_beats_p2s_upper() {
    let (chain, p1_hash, _p2_hash) = build_contested_level();
    // B-12: stage2_enabled is off by default; this test exercises
    // confirm_unique_leader's math directly, so opt it on explicitly.
    let params = ConfirmationParams { stage2_enabled: true, ..ConfirmationParams::default() };
    assert_eq!(confirm_unique_leader(&chain, 1, &params), Some(p1_hash));
}

#[test]
fn no_proposer_at_level_yields_empty_list_and_no_leader() {
    let chain = PrismChain::new(params(4));
    // B-12: opt stage-2 on so this `None` comes from the "no proposer at
    // level" branch, not from the stage2_enabled gate short-circuiting first.
    let params = ConfirmationParams { stage2_enabled: true, ..ConfirmationParams::default() };
    assert_eq!(confirm_proposer_list(&chain, 1, &params), Vec::<H256>::new());
    assert_eq!(confirm_unique_leader(&chain, 1, &params), None);
}

#[test]
fn single_proposer_with_full_deep_votes_is_confirmed_alone() {
    // Leaves chain 4 of 5 unvoted on purpose: with no rivals and U_l(r) = 0
    // (every chain voted), V_bar_1(r) collapses to exactly V_1(r), tying
    // rather than strictly beating `max_n V_n(r)` per eq. (36) -- a real
    // boundary property of the formula at this degenerate "unanimous,
    // nothing left in flight" corner, not something worth working around
    // (Stage 1's k-deep rule already handles unanimous levels cleanly; Stage
    // 2 is for contested ones). Leaving one chain still unvoted keeps
    // U_l(r) = 1 > 0, clearing the boundary the way a genuinely-in-progress
    // level would.
    let mut chain = PrismChain::new(params(5));
    let genesis = chain.proposer_genesis;
    let mut nonce = 1000u64;
    let p1 = proposer_block(1, genesis);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);

    for c in 0..4u16 {
        nonce += 1;
        chain.insert_block(voter_block(nonce, c, chain.voter_genesis[c as usize], vec![p1_hash]));
        extend(&mut chain, c, 9, &mut nonce);
    }

    // B-12: opt stage-2 on -- this test exercises confirm_unique_leader's math.
    let params = ConfirmationParams { stage2_enabled: true, ..ConfirmationParams::default() };
    assert_eq!(confirm_proposer_list(&chain, 1, &params), vec![p1_hash]);
    assert_eq!(confirm_unique_leader(&chain, 1, &params), Some(p1_hash));
}

/// Two evenly-matched contenders: list confirmation is possible (the private
/// block is ruled out), but neither's lower bound beats the other's upper --
/// so the *list* is confirmed while no *unique* leader is, illustrating the
/// distinction the architecture guide draws between the paper's own list
/// confirmation and its unique-leader strengthening.
#[test]
fn evenly_matched_contenders_confirm_as_a_list_but_not_a_unique_leader() {
    // With this placeholder delta_d, a lower bound always costs exactly 1 vote
    // off the raw count (margin lands in (0,1) whenever depth>=1, and an
    // integer count minus a fractional margin always floors down by one).
    // For two evenly-split contenders to both clear the private-block gate
    // (2L+L > m) with L = r-1, r needs to exceed m/3 + 1 -- hence 4 votes
    // each out of m=8 (r=4 > 8/3+1), rather than 3-of-6 (3 is not > 6/3+1).
    let mut chain = PrismChain::new(params(8));
    let genesis = chain.proposer_genesis;
    let mut nonce = 1000u64;
    let p1 = proposer_block(1, genesis);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);
    let p2 = proposer_block(2, genesis);
    let p2_hash = block_hash(&p2.header);
    chain.insert_block(p2);

    for c in 0..4u16 {
        nonce += 1;
        chain.insert_block(voter_block(nonce, c, chain.voter_genesis[c as usize], vec![p1_hash]));
        extend(&mut chain, c, 9, &mut nonce);
    }
    for c in 4..8u16 {
        nonce += 1;
        chain.insert_block(voter_block(nonce, c, chain.voter_genesis[c as usize], vec![p2_hash]));
        extend(&mut chain, c, 9, &mut nonce);
    }

    // B-12: opt stage-2 on -- this test's point is that confirm_unique_leader
    // returns None on the MATH (evenly matched), not on the gate.
    let params = ConfirmationParams { stage2_enabled: true, ..ConfirmationParams::default() };
    let mut list = confirm_proposer_list(&chain, 1, &params);
    list.sort();
    let mut expected = vec![p1_hash, p2_hash];
    expected.sort();
    assert_eq!(list, expected);
    assert_eq!(confirm_unique_leader(&chain, 1, &params), None);
}
