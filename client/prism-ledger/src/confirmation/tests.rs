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
    let params = ConfirmationParams::default();
    assert_eq!(confirm_unique_leader(&chain, 1, &params), Some(p1_hash));
}

#[test]
fn no_proposer_at_level_yields_empty_list_and_no_leader() {
    let chain = PrismChain::new(params(4));
    let params = ConfirmationParams::default();
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

    let params = ConfirmationParams::default();
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

    let params = ConfirmationParams::default();
    let mut list = confirm_proposer_list(&chain, 1, &params);
    list.sort();
    let mut expected = vec![p1_hash, p2_hash];
    expected.sort();
    assert_eq!(list, expected);
    assert_eq!(confirm_unique_leader(&chain, 1, &params), None);
}
