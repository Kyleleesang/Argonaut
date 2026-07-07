//! B2 "miner + network on a single node (self-mining loopback)" scenario:
//! mine real blocks via `prism-miner`, feed them through this crate's
//! `handle_announcement` (the same entry point real gossip messages go
//! through), and confirm they're validated, inserted, and (re-)gossiped
//! correctly -- including across a longer mixed-block-type run where the
//! chain's vote-total/recount invariant (see `prism-chain`'s B1 tests) must
//! keep holding.
use super::*;
use argonaut_primitives_prism::block_hash;
use argonaut_prism_chain::PrismChain;
use argonaut_prism_miner::{build_template, try_mine};
use sp_core::U256;
use std::collections::HashSet;

/// Same easy, almost-Fail-free params as `prism-miner`'s own tests: the whole
/// U256 range split (almost) entirely across the m+2 slots, so mining
/// completes in very few attempts.
fn fast_params(m: u16) -> Params {
    let difficulty = U256::max_value();
    let total_slots = U256::from(m + 2);
    let f = difficulty / total_slots;
    Params { m_voter_chains: m, difficulty, f_v: f, f_t: f, f_p: f, target_interval_ms: 1_000 }
}

fn mine_one(chain: &PrismChain, params: &Params, queue: u64) -> PrismBlock {
    let tmpl = build_template(chain, &[], queue, 8, params);
    let (found, _) = try_mine(&tmpl, params, 0, 1_000_000);
    found.expect("fast_params should yield a block well within 1_000_000 attempts")
}

#[test]
fn loopback_valid_block_is_accepted_and_gossiped() {
    let params = fast_params(4);
    let mut chain = PrismChain::new(params.clone());
    let mut seen = HashSet::new();

    let block = mine_one(&chain, &params, 0);
    let outcome = handle_announcement(BlockAnnouncement::Full(block.clone()), &params, &mut seen, &mut chain);

    assert_eq!(outcome.to_gossip.len(), 1);
    assert!(outcome.to_fetch.is_empty());
    // The block must actually have landed in the chain -- spot-check via one
    // of its possible effects (any of these sets/tips would have moved).
    let hash = block_hash(&block.header);
    let landed = chain.unreferenced_tx_blocks.contains(&hash)
        || chain.unreferenced_proposers.contains(&hash)
        || chain.voter_tips.iter().any(|(tip, _)| *tip == hash);
    assert!(landed, "mined block wasn't reflected in chain state after insertion");
}

#[test]
fn duplicate_announcement_is_ignored_via_seen_cache() {
    let params = fast_params(4);
    let mut chain = PrismChain::new(params.clone());
    let mut seen = HashSet::new();

    let block = mine_one(&chain, &params, 0);
    let first = handle_announcement(BlockAnnouncement::Full(block.clone()), &params, &mut seen, &mut chain);
    assert_eq!(first.to_gossip.len(), 1);

    let second = handle_announcement(BlockAnnouncement::Full(block), &params, &mut seen, &mut chain);
    assert!(second.to_gossip.is_empty());
    assert!(second.to_fetch.is_empty());
}

#[test]
fn structurally_invalid_block_is_rejected_and_not_inserted() {
    let params = fast_params(4);
    let mut chain = PrismChain::new(params.clone());
    let mut seen = HashSet::new();

    let mut block = mine_one(&chain, &params, 0);
    // Corrupt the parent Merkle proof so `validate_block` must reject it.
    assert!(!block.parent_proof.0.is_empty(), "expected a non-trivial Merkle proof at m=4");
    block.parent_proof.0[0].0 = H256::from([0xffu8; 32]);

    let outcome = handle_announcement(BlockAnnouncement::Full(block.clone()), &params, &mut seen, &mut chain);
    assert!(outcome.to_gossip.is_empty());
    assert!(outcome.to_fetch.is_empty());

    let hash = block_hash(&block.header);
    assert!(!chain.unreferenced_tx_blocks.contains(&hash));
    assert!(!chain.unreferenced_proposers.contains(&hash));
    assert!(chain.voter_tips.iter().all(|(tip, _)| *tip != hash));
}

#[test]
fn hash_only_announcement_requests_nothing_and_gossips_nothing() {
    let params = fast_params(4);
    let mut chain = PrismChain::new(params.clone());
    let mut seen = HashSet::new();

    let outcome =
        handle_announcement(BlockAnnouncement::Hash(H256::from([1u8; 32])), &params, &mut seen, &mut chain);
    assert!(outcome.to_gossip.is_empty());
    assert!(outcome.to_fetch.is_empty());
}

/// The full B2 single-node loopback: mine → announce → validate → insert,
/// repeated across many rounds of mixed block types, checking the chain's
/// core correctness invariant after every round (same discipline as B1's own
/// property test).
#[test]
fn multi_round_self_mining_loopback_preserves_invariants() {
    let params = fast_params(4);
    let mut chain = PrismChain::new(params.clone());
    let mut seen = HashSet::new();

    for round in 0..40u64 {
        let block = mine_one(&chain, &params, round % 8);
        let outcome =
            handle_announcement(BlockAnnouncement::Full(block), &params, &mut seen, &mut chain);
        assert_eq!(outcome.to_gossip.len(), 1, "round {round}: valid block should always be gossiped");
        assert!(outcome.to_fetch.is_empty(), "round {round}: self-mined block should never be missing refs");
        assert_eq!(
            chain.vote_totals(),
            chain.recount_votes(),
            "round {round}: vote totals diverged from recount after a networked insertion"
        );
    }
}
