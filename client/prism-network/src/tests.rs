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
    Params {
        m_voter_chains: m,
        difficulty,
        f_v: f,
        f_t: f,
        f_p: f,
        target_interval_ms: 1_000,
        chain_spec_genesis: H256::zero(),
        retarget_enabled: false,
    }
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
    let mut seen = SeenCache::new();

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
    let mut seen = SeenCache::new();

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
    let mut seen = SeenCache::new();

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
    let mut seen = SeenCache::new();

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
    let mut seen = SeenCache::new();

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

// N-3: a header relayed with a corrupt proof must not censor the genuine block.
#[test]
fn corrupt_proof_relay_does_not_censor_genuine_block() {
    let params = fast_params(4);
    let mut chain = PrismChain::new(params.clone());
    let mut seen = SeenCache::new();

    let genuine = mine_one(&chain, &params, 0);
    let mut corrupt = genuine.clone();
    assert!(!corrupt.parent_proof.0.is_empty());
    corrupt.parent_proof.0[0].0 = H256::from([0xffu8; 32]);
    // Same header => same hash.
    assert_eq!(block_hash(&corrupt.header), block_hash(&genuine.header));

    let bad = handle_announcement(BlockAnnouncement::Full(corrupt), &params, &mut seen, &mut chain);
    assert!(bad.invalid && bad.to_gossip.is_empty());
    assert!(seen.is_empty(), "invalid block must not enter the seen-cache");

    let good = handle_announcement(BlockAnnouncement::Full(genuine), &params, &mut seen, &mut chain);
    assert!(!good.invalid);
    assert_eq!(good.to_gossip.len(), 1, "genuine block must still be accepted after the corrupt relay");
}

#[test]
fn seen_cache_is_bounded_and_removable() {
    let mut seen = SeenCache::with_capacity(3);
    let h = |b: u8| H256::from([b; 32]);
    for b in 1..=5u8 {
        seen.insert(h(b));
    }
    assert_eq!(seen.len(), 3);
    assert!(!seen.contains(&h(1)) && !seen.contains(&h(2)));
    assert!(seen.contains(&h(5)));
    seen.remove(&h(5));
    assert!(!seen.contains(&h(5)));
    assert_eq!(seen.len(), 2);
}

// N-2: oversized / duplicate / over-budget GetBlocks.
#[test]
fn get_blocks_rejects_oversized_requests() {
    use crate::wire::{serve_get_blocks, MAX_GET_BLOCKS_HASHES};
    let hashes: Vec<H256> =
        (0..(MAX_GET_BLOCKS_HASHES as u64 + 1)).map(H256::from_low_u64_be).collect();
    let mut lookups = 0;
    let r = serve_get_blocks(hashes, |_| {
        lookups += 1;
        None
    });
    assert!(r.is_err());
    assert_eq!(lookups, 0, "an oversized request must be rejected before touching the DB");
    // 32k-hash attack shape.
    let huge: Vec<H256> = (0..32_000u64).map(H256::from_low_u64_be).collect();
    assert!(serve_get_blocks(huge, |_| None).is_err());
}

#[test]
fn get_blocks_dedups_and_respects_byte_budget() {
    use crate::wire::{serve_get_blocks, serve_get_blocks_with_budget};
    let params = fast_params(4);
    let mut chain = PrismChain::new(params.clone());
    let mut blocks = Vec::new();
    for round in 0..3u64 {
        let b = mine_one(&chain, &params, round);
        chain.insert_block(b.clone());
        blocks.push(b);
    }
    let hashes: Vec<H256> = blocks.iter().map(|b| block_hash(&b.header)).collect();
    let lookup = |h: &H256| blocks.iter().find(|b| block_hash(&b.header) == *h).cloned();

    // Same hash 100x -> one block.
    let dup = vec![hashes[0]; 100];
    assert_eq!(serve_get_blocks(dup, lookup).unwrap().len(), 1);

    // Budget of one block's size admits exactly one of the three.
    let one = parity_scale_codec::Encode::encoded_size(&blocks[0]);
    let out = serve_get_blocks_with_budget(hashes.clone(), lookup, one).unwrap();
    assert_eq!(out.len(), 1);
    // Ample budget returns all three, in request order.
    let all = serve_get_blocks(hashes.clone(), lookup).unwrap();
    assert_eq!(all.iter().map(|b| block_hash(&b.header)).collect::<Vec<_>>(), hashes);
}

// N-6: a ref a peer failed to serve is not re-sent to it until the TTL lapses.
#[test]
fn cant_serve_cache_is_per_peer_and_expires() {
    use crate::wire::{CantServe, CANT_SERVE_TTL};
    use sc_network::PeerId;
    use std::time::Instant;
    let (a, b) = (PeerId::random(), PeerId::random());
    let h = H256::from([7u8; 32]);
    let mut c = CantServe::default();
    let t0 = Instant::now();
    assert!(!c.is_blocked(&a, &h, t0));
    c.record(a, h, t0);
    assert!(c.is_blocked(&a, &h, t0));
    assert!(!c.is_blocked(&b, &h, t0), "other peers (e.g. the archive node) must still be asked");
    assert!(!c.is_blocked(&a, &h, t0 + CANT_SERVE_TTL + std::time::Duration::from_secs(1)));
    c.forget_peer(&a);
    assert!(!c.is_blocked(&a, &h, t0));
}
