//! B1 acceptance tests: scripted block sequences (incl. voter-chain reorgs with
//! vote rollback), and the property that vote totals always match a from-scratch
//! recount, per architecture guide §3.9.
use super::*;
use argonaut_primitives_prism::{MerkleProof, SuperHeader};
use sp_core::U256;

fn params() -> Params {
    let mut p = Params::testnet();
    // Small m for readable tests; insert_block only reads `m_voter_chains`
    // (sizing voter_tips/unvoted_levels), never the difficulty/f_* fields, so
    // leaving those at their (now-inconsistent) testnet(100) values is fine --
    // no test here calls `sortition`/`validate_block`.
    p.m_voter_chains = 4;
    p
}

fn header(nonce: u64) -> SuperHeader {
    SuperHeader {
        parents_root: H256::zero(),
        content_root: H256::zero(),
        timestamp_ms: 0,
        nonce,
        difficulty: U256::max_value(),
    }
}

fn tx_block(nonce: u64, parent: H256) -> PrismBlock {
    PrismBlock {
        header: header(nonce),
        parent,
        content: BlockContent::Transaction { txs: vec![vec![nonce as u8]] },
        parent_proof: MerkleProof::default(),
        content_proof: MerkleProof::default(),
    }
}

fn proposer_block(nonce: u64, parent: H256, tx_refs: Vec<H256>, prop_refs: Vec<H256>) -> PrismBlock {
    PrismBlock {
        header: header(nonce),
        parent,
        content: BlockContent::Proposer { tx_refs, prop_refs },
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

/// Insert a voter block on chain 0 and assert the incremental vote totals
/// still agree with an independent from-scratch recount.
fn ins(chain: &mut PrismChain, parent: H256, votes: Vec<H256>, nonce: &mut u64) -> H256 {
    *nonce += 1;
    let b = voter_block(1000 + *nonce, 0, parent, votes);
    let h = block_hash(&b.header);
    chain.insert_block(b);
    assert_eq!(
        chain.vote_totals(),
        chain.recount_votes(),
        "vote totals diverged from recount after insertion #{}",
        *nonce
    );
    h
}

#[test]
fn proposer_levels_advance_and_mark_unvoted() {
    let mut chain = PrismChain::new(params());
    let genesis = genesis_sentinel();
    let p1 = proposer_block(1, genesis, vec![], vec![]);
    let p1_hash = block_hash(&p1.header);
    let outcome = chain.insert_block(p1);

    assert!(outcome.missing.is_empty());
    assert!(outcome.may_confirm);
    assert_eq!(chain.proposer_best_level, 1);
    for chain_map in &chain.unvoted_levels {
        assert_eq!(chain_map.get(&1), Some(&p1_hash));
    }
}

#[test]
fn sibling_proposers_at_same_level_do_not_reset_unvoted_marker() {
    let mut chain = PrismChain::new(params());
    let genesis = genesis_sentinel();
    let p1 = proposer_block(1, genesis, vec![], vec![]);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);

    // A sibling at the same level (also parented on genesis).
    let p1b = proposer_block(2, genesis, vec![], vec![]);
    chain.insert_block(p1b);

    assert_eq!(chain.proposer_best_level, 1);
    // The first-seen proposer at level 1 remains the unvoted marker.
    assert_eq!(chain.unvoted_levels[0].get(&1), Some(&p1_hash));
}

#[test]
fn tx_block_referenced_by_proposer_becomes_referenced() {
    let mut chain = PrismChain::new(params());
    let genesis = genesis_sentinel();
    let tx = tx_block(1, genesis);
    let tx_hash = block_hash(&tx.header);
    chain.insert_block(tx);
    assert!(chain.unreferenced_tx_blocks.contains(&tx_hash));

    let p1 = proposer_block(2, genesis, vec![tx_hash], vec![]);
    chain.insert_block(p1);
    assert!(!chain.unreferenced_tx_blocks.contains(&tx_hash));
}

#[test]
fn proposer_orphan_resolves_when_parent_arrives() {
    let mut chain = PrismChain::new(params());
    let genesis = genesis_sentinel();
    let p1 = proposer_block(1, genesis, vec![], vec![]);
    let p1_hash = block_hash(&p1.header);
    let p2 = proposer_block(2, p1_hash, vec![], vec![]);
    let p2_hash = block_hash(&p2.header);

    // p2 arrives before its parent p1 -> orphaned.
    let outcome = chain.insert_block(p2.clone());
    assert_eq!(outcome.missing, vec![p1_hash]);
    assert!(chain.orphans.contains_key(&p2_hash));
    assert_eq!(chain.proposer_best_level, 0);

    // p1 arrives -> both get processed; p2 drains from the orphan pool.
    chain.insert_block(p1);
    assert!(!chain.orphans.contains_key(&p2_hash));
    assert_eq!(chain.proposer_best_level, 2);
}

#[test]
fn voter_orphan_resolves_when_parent_arrives() {
    let mut chain = PrismChain::new(params());
    let genesis = genesis_sentinel();
    let v1 = voter_block(1, 0, genesis, vec![]);
    let v1_hash = block_hash(&v1.header);
    let v2 = voter_block(2, 0, v1_hash, vec![]);
    let v2_hash = block_hash(&v2.header);

    let outcome = chain.insert_block(v2.clone());
    assert_eq!(outcome.missing, vec![v1_hash]);
    assert!(chain.orphans.contains_key(&v2_hash));

    chain.insert_block(v1);
    assert!(!chain.orphans.contains_key(&v2_hash));
    assert_eq!(chain.voter_tips[0].1, 2);
}

#[test]
fn voter_chain_linear_growth_matches_recount() {
    let mut chain = PrismChain::new(params());
    let genesis = genesis_sentinel();

    let p1 = proposer_block(1, genesis, vec![], vec![]);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);
    let p2 = proposer_block(2, p1_hash, vec![], vec![]);
    let p2_hash = block_hash(&p2.header);
    chain.insert_block(p2);

    let mut nonce = 0;
    let v1 = ins(&mut chain, genesis, vec![p1_hash], &mut nonce);
    ins(&mut chain, v1, vec![p2_hash], &mut nonce);

    assert_eq!(chain.votes.get(&p1_hash).map(|v| v.total), Some(1));
    assert_eq!(chain.votes.get(&p2_hash).map(|v| v.total), Some(1));
    assert_eq!(chain.vote_totals(), chain.recount_votes());
}

/// The central B1 acceptance scenario: a competing, longer voter-chain fork
/// must roll back the abandoned segment's votes and apply the new segment's,
/// ending in exact agreement with an independent recount.
#[test]
fn voter_chain_reorg_rolls_back_and_reapplies_votes() {
    let mut chain = PrismChain::new(params());
    let genesis = genesis_sentinel();

    let mut prop_hash = genesis;
    let mut proposers = Vec::new();
    for i in 0..4u64 {
        let p = proposer_block(100 + i, prop_hash, vec![], vec![]);
        prop_hash = block_hash(&p.header);
        proposers.push(prop_hash);
        chain.insert_block(p);
    }
    let (p1_hash, p2_hash, p3_hash, p4_hash) = (proposers[0], proposers[1], proposers[2], proposers[3]);

    // Old fork: genesis -> a1 (votes p1) -> a2 (votes p2). Depth 2.
    let mut nonce = 0;
    let a1 = ins(&mut chain, genesis, vec![p1_hash], &mut nonce);
    let a2 = ins(&mut chain, a1, vec![p2_hash], &mut nonce);

    assert_eq!(chain.voter_tips[0].0, a2);
    assert_eq!(chain.votes.get(&p1_hash).map(|v| v.total), Some(1));
    assert_eq!(chain.votes.get(&p2_hash).map(|v| v.total), Some(1));

    // Competing fork from genesis, longer: b1 (votes p3) -> b2 (votes p4) -> b3 (no votes).
    let b1 = ins(&mut chain, genesis, vec![p3_hash], &mut nonce);
    let b2 = ins(&mut chain, b1, vec![p4_hash], &mut nonce);
    let b3 = ins(&mut chain, b2, vec![], &mut nonce);

    // New fork (depth 3) should have won over the old one (depth 2).
    assert_eq!(chain.voter_tips[0], (b3, 3));

    // Old votes rolled back entirely.
    assert!(chain.votes.get(&p1_hash).is_none());
    assert!(chain.votes.get(&p2_hash).is_none());
    // New votes applied.
    assert_eq!(chain.votes.get(&p3_hash).map(|v| v.total), Some(1));
    assert_eq!(chain.votes.get(&p4_hash).map(|v| v.total), Some(1));

    // p1/p2's levels should be unvoted again on chain 0; p3/p4's should not.
    assert_eq!(chain.unvoted_levels[0].get(&1), Some(&p1_hash));
    assert_eq!(chain.unvoted_levels[0].get(&2), Some(&p2_hash));
    assert_eq!(chain.unvoted_levels[0].get(&3), None);
    assert_eq!(chain.unvoted_levels[0].get(&4), None);

    assert_eq!(chain.vote_totals(), chain.recount_votes());
}

#[test]
fn reorg_on_one_chain_does_not_affect_another() {
    let mut chain = PrismChain::new(params());
    let genesis = genesis_sentinel();
    let p1 = proposer_block(1, genesis, vec![], vec![]);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);

    // Chain 1 votes for p1 and must stay untouched by chain 0's reorg below.
    let v_chain1 = voter_block(50, 1, genesis, vec![p1_hash]);
    chain.insert_block(v_chain1);
    assert_eq!(chain.votes.get(&p1_hash).map(|v| v.total), Some(1));

    let mut nonce = 0;
    ins(&mut chain, genesis, vec![p1_hash], &mut nonce);
    assert_eq!(chain.votes.get(&p1_hash).map(|v| v.total), Some(2));

    // A longer competing fork on chain 0 that casts no votes at all.
    let c1 = ins(&mut chain, genesis, vec![], &mut nonce);
    ins(&mut chain, c1, vec![], &mut nonce);

    // Chain 0's vote for p1 is rolled back; chain 1's vote remains.
    assert_eq!(chain.votes.get(&p1_hash).map(|v| v.total), Some(1));
    assert_eq!(chain.vote_totals(), chain.recount_votes());
}

/// Longer scripted sequence: build, fork, reorg, extend, then reorg again from
/// genesis with a third fork -- checking the recount invariant after every
/// single insertion, not just at the end.
#[test]
fn vote_totals_match_recount_after_every_insertion_in_a_long_scripted_sequence() {
    let mut chain = PrismChain::new(params());
    let genesis = genesis_sentinel();

    let mut prop_hash = genesis;
    let mut proposers = Vec::new();
    for i in 0..6u64 {
        let p = proposer_block(100 + i, prop_hash, vec![], vec![]);
        prop_hash = block_hash(&p.header);
        proposers.push(prop_hash);
        chain.insert_block(p);
    }

    let mut nonce = 0u64;
    let a1 = ins(&mut chain, genesis, vec![proposers[0]], &mut nonce);
    ins(&mut chain, a1, vec![proposers[1]], &mut nonce);

    // Competing fork from genesis, initially shorter (depth 1) -- must NOT
    // replace the tip or change vote totals yet.
    let b1 = ins(&mut chain, genesis, vec![proposers[2]], &mut nonce);
    assert_eq!(chain.voter_tips[0].1, 2, "shorter fork must not replace the tip");

    // Extend b to depth 3 -- now longer than a, must reorg.
    let b2 = ins(&mut chain, b1, vec![proposers[3]], &mut nonce);
    let b3 = ins(&mut chain, b2, vec![proposers[4]], &mut nonce);
    assert_eq!(chain.voter_tips[0].1, 3);
    assert!(chain.votes.get(&proposers[0]).is_none());
    assert!(chain.votes.get(&proposers[1]).is_none());
    assert_eq!(chain.votes.get(&proposers[2]).map(|v| v.total), Some(1));
    assert_eq!(chain.votes.get(&proposers[3]).map(|v| v.total), Some(1));
    assert_eq!(chain.votes.get(&proposers[4]).map(|v| v.total), Some(1));

    // Extend further, then a fresh, longer-still fork from genesis again.
    let b4 = ins(&mut chain, b3, vec![proposers[5]], &mut nonce);
    let _ = b4;
    let c1 = ins(&mut chain, genesis, vec![], &mut nonce);
    let c2 = ins(&mut chain, c1, vec![], &mut nonce);
    let c3 = ins(&mut chain, c2, vec![], &mut nonce);
    let c4 = ins(&mut chain, c3, vec![], &mut nonce);
    ins(&mut chain, c4, vec![], &mut nonce);

    assert_eq!(chain.voter_tips[0].1, 5);
    // The whole b-fork's votes should now be gone; the c-fork cast none.
    for p in &proposers {
        assert!(chain.votes.get(p).is_none(), "all votes should be rolled back");
    }
    assert_eq!(chain.vote_totals(), chain.recount_votes());
}

#[test]
fn leader_at_level_returns_none_before_any_proposer_exists() {
    let chain = PrismChain::new(params());
    assert_eq!(chain.leader_at_level(1), None);
}

#[test]
fn leader_at_level_picks_the_proposer_with_more_votes() {
    let mut chain = PrismChain::new(params());
    let genesis = genesis_sentinel();

    let p1 = proposer_block(1, genesis, vec![], vec![]);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);
    let p1b = proposer_block(2, genesis, vec![], vec![]);
    let p1b_hash = block_hash(&p1b.header);
    chain.insert_block(p1b);

    // p1_hash gets votes from chains 0 and 1; p1b_hash gets one vote from chain 2.
    chain.insert_block(voter_block(101, 0, genesis, vec![p1_hash]));
    chain.insert_block(voter_block(102, 1, genesis, vec![p1_hash]));
    chain.insert_block(voter_block(103, 2, genesis, vec![p1b_hash]));

    assert_eq!(chain.leader_at_level(1), Some(p1_hash));
}

#[test]
fn leader_at_level_tie_breaks_deterministically_by_hash() {
    let mut chain = PrismChain::new(params());
    let genesis = genesis_sentinel();

    let p1 = proposer_block(1, genesis, vec![], vec![]);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);
    let p1b = proposer_block(2, genesis, vec![], vec![]);
    let p1b_hash = block_hash(&p1b.header);
    chain.insert_block(p1b);

    // Neither proposer has any votes -- 0 == 0, so the tie-break (larger hash)
    // decides. Compute independently and check it matches, rather than
    // asserting a specific hash literal that would be brittle to change.
    let expected = std::cmp::max(p1_hash, p1b_hash);
    assert_eq!(chain.leader_at_level(1), Some(expected));
}
