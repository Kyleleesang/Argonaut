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
    let genesis = chain.proposer_genesis;
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
    let genesis = chain.proposer_genesis;
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
fn proposers_at_level_includes_siblings_never_added_to_unvoted_levels() {
    let mut chain = PrismChain::new(params());
    let genesis = chain.proposer_genesis;
    let p1 = proposer_block(1, genesis, vec![], vec![]);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);

    let p1b = proposer_block(2, genesis, vec![], vec![]);
    let p1b_hash = block_hash(&p1b.header);
    chain.insert_block(p1b);

    let mut at_level_1 = chain.proposers_at_level(1);
    at_level_1.sort();
    let mut expected = vec![p1_hash, p1b_hash];
    expected.sort();
    assert_eq!(at_level_1, expected);
    assert_eq!(chain.proposers_at_level(2), Vec::<H256>::new());
}

#[test]
fn tx_block_referenced_by_proposer_becomes_referenced() {
    let mut chain = PrismChain::new(params());
    let genesis = chain.proposer_genesis;
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
    let genesis = chain.proposer_genesis;
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
    let genesis = chain.voter_genesis[0];
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
    let proposer_genesis = chain.proposer_genesis;
    let voter_genesis0 = chain.voter_genesis[0];

    let p1 = proposer_block(1, proposer_genesis, vec![], vec![]);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);
    let p2 = proposer_block(2, p1_hash, vec![], vec![]);
    let p2_hash = block_hash(&p2.header);
    chain.insert_block(p2);

    let mut nonce = 0;
    let v1 = ins(&mut chain, voter_genesis0, vec![p1_hash], &mut nonce);
    ins(&mut chain, v1, vec![p2_hash], &mut nonce);

    assert_eq!(chain.votes.get(&p1_hash).map(|v| v.total()), Some(1));
    assert_eq!(chain.votes.get(&p2_hash).map(|v| v.total()), Some(1));
    assert_eq!(chain.vote_totals(), chain.recount_votes());
}

/// The central B1 acceptance scenario: a competing, longer voter-chain fork
/// must roll back the abandoned segment's votes and apply the new segment's,
/// ending in exact agreement with an independent recount.
#[test]
fn voter_chain_reorg_rolls_back_and_reapplies_votes() {
    let mut chain = PrismChain::new(params());
    let proposer_genesis = chain.proposer_genesis;
    let voter_genesis0 = chain.voter_genesis[0];

    let mut prop_hash = proposer_genesis;
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
    let a1 = ins(&mut chain, voter_genesis0, vec![p1_hash], &mut nonce);
    let a2 = ins(&mut chain, a1, vec![p2_hash], &mut nonce);

    assert_eq!(chain.voter_tips[0].0, a2);
    assert_eq!(chain.votes.get(&p1_hash).map(|v| v.total()), Some(1));
    assert_eq!(chain.votes.get(&p2_hash).map(|v| v.total()), Some(1));

    // Competing fork from genesis, longer: b1 (votes p3) -> b2 (votes p4) -> b3 (no votes).
    let b1 = ins(&mut chain, voter_genesis0, vec![p3_hash], &mut nonce);
    let b2 = ins(&mut chain, b1, vec![p4_hash], &mut nonce);
    let b3 = ins(&mut chain, b2, vec![], &mut nonce);

    // New fork (depth 3) should have won over the old one (depth 2).
    assert_eq!(chain.voter_tips[0], (b3, 3));

    // Old votes rolled back entirely.
    assert!(chain.votes.get(&p1_hash).is_none());
    assert!(chain.votes.get(&p2_hash).is_none());
    // New votes applied.
    assert_eq!(chain.votes.get(&p3_hash).map(|v| v.total()), Some(1));
    assert_eq!(chain.votes.get(&p4_hash).map(|v| v.total()), Some(1));

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
    let proposer_genesis = chain.proposer_genesis;
    let voter_genesis0 = chain.voter_genesis[0];
    let voter_genesis1 = chain.voter_genesis[1];
    let p1 = proposer_block(1, proposer_genesis, vec![], vec![]);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);

    // Chain 1 votes for p1 and must stay untouched by chain 0's reorg below.
    let v_chain1 = voter_block(50, 1, voter_genesis1, vec![p1_hash]);
    chain.insert_block(v_chain1);
    assert_eq!(chain.votes.get(&p1_hash).map(|v| v.total()), Some(1));

    let mut nonce = 0;
    ins(&mut chain, voter_genesis0, vec![p1_hash], &mut nonce);
    assert_eq!(chain.votes.get(&p1_hash).map(|v| v.total()), Some(2));

    // A longer competing fork on chain 0 that casts no votes at all.
    let c1 = ins(&mut chain, voter_genesis0, vec![], &mut nonce);
    ins(&mut chain, c1, vec![], &mut nonce);

    // Chain 0's vote for p1 is rolled back; chain 1's vote remains.
    assert_eq!(chain.votes.get(&p1_hash).map(|v| v.total()), Some(1));
    assert_eq!(chain.vote_totals(), chain.recount_votes());
}

/// Longer scripted sequence: build, fork, reorg, extend, then reorg again from
/// genesis with a third fork -- checking the recount invariant after every
/// single insertion, not just at the end.
#[test]
fn vote_totals_match_recount_after_every_insertion_in_a_long_scripted_sequence() {
    let mut chain = PrismChain::new(params());
    let proposer_genesis = chain.proposer_genesis;
    let voter_genesis0 = chain.voter_genesis[0];

    let mut prop_hash = proposer_genesis;
    let mut proposers = Vec::new();
    for i in 0..6u64 {
        let p = proposer_block(100 + i, prop_hash, vec![], vec![]);
        prop_hash = block_hash(&p.header);
        proposers.push(prop_hash);
        chain.insert_block(p);
    }

    let mut nonce = 0u64;
    let a1 = ins(&mut chain, voter_genesis0, vec![proposers[0]], &mut nonce);
    ins(&mut chain, a1, vec![proposers[1]], &mut nonce);

    // Competing fork from genesis, initially shorter (depth 1) -- must NOT
    // replace the tip or change vote totals yet.
    let b1 = ins(&mut chain, voter_genesis0, vec![proposers[2]], &mut nonce);
    assert_eq!(chain.voter_tips[0].1, 2, "shorter fork must not replace the tip");

    // Extend b to depth 3 -- now longer than a, must reorg.
    let b2 = ins(&mut chain, b1, vec![proposers[3]], &mut nonce);
    let b3 = ins(&mut chain, b2, vec![proposers[4]], &mut nonce);
    assert_eq!(chain.voter_tips[0].1, 3);
    assert!(chain.votes.get(&proposers[0]).is_none());
    assert!(chain.votes.get(&proposers[1]).is_none());
    assert_eq!(chain.votes.get(&proposers[2]).map(|v| v.total()), Some(1));
    assert_eq!(chain.votes.get(&proposers[3]).map(|v| v.total()), Some(1));
    assert_eq!(chain.votes.get(&proposers[4]).map(|v| v.total()), Some(1));

    // Extend further, then a fresh, longer-still fork from genesis again.
    let b4 = ins(&mut chain, b3, vec![proposers[5]], &mut nonce);
    let _ = b4;
    let c1 = ins(&mut chain, voter_genesis0, vec![], &mut nonce);
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
    let proposer_genesis = chain.proposer_genesis;
    let voter_genesis0 = chain.voter_genesis[0];
    let voter_genesis1 = chain.voter_genesis[1];
    let voter_genesis2 = chain.voter_genesis[2];

    let p1 = proposer_block(1, proposer_genesis, vec![], vec![]);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);
    let p1b = proposer_block(2, proposer_genesis, vec![], vec![]);
    let p1b_hash = block_hash(&p1b.header);
    chain.insert_block(p1b);

    // p1_hash gets votes from chains 0 and 1; p1b_hash gets one vote from chain 2.
    chain.insert_block(voter_block(101, 0, voter_genesis0, vec![p1_hash]));
    chain.insert_block(voter_block(102, 1, voter_genesis1, vec![p1_hash]));
    chain.insert_block(voter_block(103, 2, voter_genesis2, vec![p1b_hash]));

    assert_eq!(chain.leader_at_level(1), Some(p1_hash));
}

#[test]
fn leader_at_level_tie_breaks_deterministically_by_hash() {
    let mut chain = PrismChain::new(params());
    let genesis = chain.proposer_genesis;

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

// AF-2: the stage-1 "conservative k-deep" confirmation *policy* (previously
// `PrismChain::confirmed_leader_at_level`) moved to `client/prism-ledger`
// (architecture guide §3.8 assigns confirmation there, not to the DAG state
// machine) -- see `argonaut_prism_ledger::confirmed_leader_at_level` and its
// tests. `PrismChain` itself only exposes the raw facts that policy needs
// (`level_proposer`, `unvoted_levels`, `votes`, `voter_tips`), all already
// public fields.

/// B-2: `insert_block` must feed the difficulty retarget with each accepted
/// block's *real* timestamp (not the shared `header()` test helper's fixed
/// `timestamp_ms: 0`, which every other test in this file relies on to never
/// trigger a retarget -- see `argonaut_primitives_prism::DifficultyRetarget`'s
/// own "ignores non-advancing timestamps" test for that half of the
/// contract). Tx blocks are the simplest path: always accepted immediately,
/// no votes/deps required.
#[test]
fn insert_block_feeds_the_difficulty_retarget_with_real_timestamps() {
    // B-11: retargeting is off by default (see `Params::retarget_enabled`'s
    // doc comment); this test exercises `observe_for_retarget` -> `retarget`
    // itself, so opt it on explicitly.
    let mut p = params();
    p.retarget_enabled = true;
    let mut chain = PrismChain::new(p);
    let genesis = chain.proposer_genesis;
    let initial_difficulty = chain.params.difficulty;

    let advancing_header = |nonce: u64, timestamp_ms: u64| SuperHeader {
        parents_root: H256::zero(),
        content_root: H256::zero(),
        timestamp_ms,
        nonce,
        difficulty: U256::max_value(),
    };
    let advancing_tx_block = |nonce: u64, timestamp_ms: u64| PrismBlock {
        header: advancing_header(nonce, timestamp_ms),
        parent: genesis,
        content: BlockContent::Transaction { txs: vec![vec![nonce as u8]] },
        parent_proof: MerkleProof::default(),
        content_proof: MerkleProof::default(),
    };

    // Blocks arriving much faster than the (default 1000ms) target interval
    // should make mining *harder* -- i.e. shrink the `difficulty` threshold
    // value (a block is valid when `hash <= difficulty`, so a smaller value
    // is a smaller, harder-to-hit acceptance range) -- once the retarget
    // window fills (`RETARGET_WINDOW + 1` samples needed for the first
    // result).
    for i in 0..=argonaut_primitives_prism::RETARGET_WINDOW {
        chain.insert_block(advancing_tx_block(i, i * 10));
    }

    assert!(
        chain.params.difficulty < initial_difficulty,
        "difficulty threshold should have shrunk (harder) in response to fast block arrivals"
    );
    // The equal m+2-way split invariant must survive the retarget.
    let total_slots = U256::from(chain.params.m_voter_chains + 2);
    assert_eq!(chain.params.f_v, chain.params.difficulty / total_slots);
    assert_eq!(chain.params.f_v, chain.params.f_t);
    assert_eq!(chain.params.f_v, chain.params.f_p);
}

/// B-7: a restarted node must not forget its DAG. Builds a non-trivial chain
/// (proposers, tx blocks referenced by a proposer, a voter-chain reorg) against
/// a real `PrismDb`, then opens a *second*, independent `PrismChain::with_db`
/// against the same on-disk path (simulating a process restart) and checks
/// every piece of in-memory state a restarted node actually depends on --
/// mining/voting position, vote accounting, and the unreferenced-block sets
/// the miner uses to pick what to reference next -- matches exactly.
#[test]
fn with_db_recovers_full_dag_state_after_a_restart() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let db = std::sync::Arc::new(PrismDb::open(dir.path()).expect("open PrismDb"));

    let mut chain = PrismChain::with_db(params(), db.clone());
    let proposer_genesis = chain.proposer_genesis;
    let voter_genesis0 = chain.voter_genesis[0];

    // Proposer chain, with a tx block referenced by the second proposer.
    let p1 = proposer_block(1, proposer_genesis, vec![], vec![]);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);

    let tx = tx_block(2, proposer_genesis);
    let tx_hash = block_hash(&tx.header);
    chain.insert_block(tx);

    let p2 = proposer_block(3, p1_hash, vec![tx_hash], vec![]);
    let p2_hash = block_hash(&p2.header);
    chain.insert_block(p2);

    // p3 explicitly references p2 via `prop_refs` (on top of `p2` also being
    // its literal DAG parent) so `unreferenced_proposers`' removal path -- the
    // one that actually depends on `prop_refs`, not the `parent` link -- gets
    // exercised too: p2 becomes referenced, p1 and p3 do not.
    let p3 = proposer_block(4, p2_hash, vec![], vec![p2_hash]);
    let p3_hash = block_hash(&p3.header);
    chain.insert_block(p3);
    let unreferenced_tx = tx_block(5, proposer_genesis);
    let unreferenced_tx_hash = block_hash(&unreferenced_tx.header);
    chain.insert_block(unreferenced_tx);

    // A voter-chain reorg (same shape as `voter_chain_reorg_rolls_back_and_reapplies_votes`)
    // so restart recovery is checked against non-trivial vote accounting, not
    // just a freshly-accepted, never-superseded vote.
    let mut nonce = 0u64;
    let a1 = ins(&mut chain, voter_genesis0, vec![p1_hash], &mut nonce);
    ins(&mut chain, a1, vec![p2_hash], &mut nonce);
    let b1 = ins(&mut chain, voter_genesis0, vec![p1_hash], &mut nonce);
    let b2 = ins(&mut chain, b1, vec![], &mut nonce);
    ins(&mut chain, b2, vec![], &mut nonce);

    assert_eq!(chain.vote_totals(), chain.recount_votes());

    // "Restart": drop the first `PrismChain` (the `PrismDb` handle stays
    // alive via the `Arc`, exactly as it would across a real process restart
    // where the *files on disk* persist, not the in-process handle) and open
    // a brand new one against the same path.
    drop(chain);
    let recovered = PrismChain::with_db(params(), db);

    assert_eq!(recovered.proposer_best_level, 3);
    assert_eq!(recovered.level_proposer.get(&1), Some(&p1_hash));
    assert_eq!(recovered.level_proposer.get(&2), Some(&p2_hash));
    assert_eq!(recovered.level_proposer.get(&3), Some(&p3_hash));
    assert_eq!(recovered.voter_tips[0].1, 3, "chain 0's tip should be the longer b-fork (b1,b2,b3)");
    assert!(!recovered.unreferenced_tx_blocks.contains(&tx_hash), "tx referenced by p2 stays referenced");
    assert!(recovered.unreferenced_tx_blocks.contains(&unreferenced_tx_hash));
    assert!(recovered.unreferenced_proposers.contains(&p1_hash), "p1 is never listed in anyone's prop_refs");
    assert!(!recovered.unreferenced_proposers.contains(&p2_hash), "p3's prop_refs references p2");
    assert!(recovered.unreferenced_proposers.contains(&p3_hash), "p3 is the tip, referenced by nobody");
    assert_eq!(recovered.vote_totals(), recovered.recount_votes());
    assert_eq!(
        recovered.vote_totals(),
        [(p1_hash, 1u64)].into_iter().collect::<std::collections::HashMap<_, _>>(),
        "only the surviving b-fork's vote for p1 should remain after the reorg"
    );
}

/// B-6: `has_block` is what `client/prism-network`'s backfill path uses to
/// filter a peer's `Status` response down to genuinely-unknown hashes --
/// get this wrong and a backfill can re-run `insert_block` on an
/// already-accepted proposer, corrupting `unreferenced_proposers`.
#[test]
fn has_block_reports_every_known_block_kind_and_not_unknown_ones() {
    let mut chain = PrismChain::new(params());
    let proposer_genesis = chain.proposer_genesis;
    let voter_genesis0 = chain.voter_genesis[0];
    let mut nonce = 0u64;

    let tx = tx_block(1, proposer_genesis);
    let tx_hash = block_hash(&tx.header);
    chain.insert_block(tx.clone());

    let prop = proposer_block(2, proposer_genesis, vec![tx_hash], vec![]);
    let prop_hash = block_hash(&prop.header);
    chain.insert_block(prop.clone());

    let voter_hash = ins(&mut chain, voter_genesis0, vec![prop_hash], &mut nonce);

    // An orphan: a proposer block whose parent is never accepted.
    let dangling_parent = H256::from([7u8; 32]);
    let orphan = proposer_block(3, dangling_parent, vec![], vec![]);
    let orphan_hash = block_hash(&orphan.header);
    chain.insert_block(orphan);

    assert!(chain.has_block(&proposer_genesis));
    assert!(chain.has_block(&voter_genesis0));
    assert!(chain.has_block(&tx_hash));
    assert!(chain.has_block(&prop_hash));
    assert!(chain.has_block(&voter_hash));
    assert!(chain.has_block(&orphan_hash), "an orphaned block is still known -- only its deps are missing");
    assert!(!chain.has_block(&dangling_parent), "the orphan's missing parent itself was never inserted");
    assert!(!chain.has_block(&H256::from([9u8; 32])), "a hash never seen at all");
}

/// B5 pruning: the load-bearing invariant is that pruning `voter_meta` never
/// changes vote accounting (`chain.votes`) -- both confirmation stages read
/// only that incrementally-maintained map, never voter-block history. Also
/// locks in the accepted, documented side effect: `recount_votes` (a
/// from-scratch replay of history) *does* start undercounting once its walk
/// crosses the pruned frontier, since that's exactly the history pruning
/// deletes -- this is expected, not a regression, per `PrismChain::prune`'s
/// doc comment.
#[test]
fn prune_removes_deep_voter_history_without_changing_vote_totals() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let db = std::sync::Arc::new(PrismDb::open(dir.path()).expect("open PrismDb"));
    let mut chain = PrismChain::with_db(params(), db);
    let proposer_genesis = chain.proposer_genesis;
    let voter_genesis0 = chain.voter_genesis[0];

    let prop = proposer_block(1, proposer_genesis, vec![], vec![]);
    let prop_hash = block_hash(&prop.header);
    chain.insert_block(prop);

    // 30-deep chain 0; only the first block casts a vote, so that vote's
    // casting block (depth 1) is exactly what a k_prune=10 frontier (tip
    // depth 30 -> frontier 20) should sweep.
    let mut nonce = 0u64;
    let mut tip = voter_genesis0;
    for i in 0..30u64 {
        let votes = if i == 0 { vec![prop_hash] } else { vec![] };
        tip = ins(&mut chain, tip, votes, &mut nonce);
    }
    let _ = tip;

    let before = chain.vote_totals();
    assert_eq!(before.get(&prop_hash), Some(&1));
    assert_eq!(before, chain.recount_votes(), "sanity check before pruning");

    chain.prune(10, 0);

    assert_eq!(chain.vote_totals(), before, "pruning must never change vote accounting");
    assert_ne!(
        chain.recount_votes(),
        before,
        "recount_votes walks history and must now undercount past the pruned frontier -- \
         if this starts passing again, either pruning stopped removing anything or \
         recount_votes started reading something other than voter_meta"
    );
}

/// B5 pruning: `has_block` must keep reporting `true` for a pruned hash (via
/// the `BlockStatus::Pruned` tombstone), and a stale re-delivery of a pruned
/// voter block's *child* must be rejected outright, not orphaned forever
/// waiting on a parent that can never arrive.
#[test]
fn pruned_blocks_stay_known_and_reject_rather_than_orphan_descendants() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let db = std::sync::Arc::new(PrismDb::open(dir.path()).expect("open PrismDb"));
    let mut chain = PrismChain::with_db(params(), db);
    let voter_genesis0 = chain.voter_genesis[0];

    let mut nonce = 0u64;
    let mut hashes = Vec::new();
    let mut tip = voter_genesis0;
    for _ in 0..30u64 {
        tip = ins(&mut chain, tip, vec![], &mut nonce);
        hashes.push(tip);
    }

    chain.prune(10, 0); // frontier = 30 - 10 = 20 -> hashes[0..20) pruned (depths 1..=20)

    let pruned_hash = hashes[5]; // depth 6, well within the pruned range
    assert!(chain.has_block(&pruned_hash), "a pruned block must still be reported as known");

    // A block whose parent is the pruned hash, re-delivered as if from a
    // stale peer: must be rejected (no missing refs requested, not stuck in
    // `orphans`), not orphaned forever.
    let dangling_child = voter_block(9999, 0, pruned_hash, vec![]);
    let dangling_hash = block_hash(&dangling_child.header);
    let outcome = chain.insert_block(dangling_child);
    assert!(outcome.missing.is_empty(), "a pruned parent must never be requested via fetch");
    assert!(!chain.orphans.contains_key(&dangling_hash), "must be rejected, not left waiting in orphans");
}

/// B5 pruning: proposer/tx-block *bodies* referenced only by already-confirmed
/// history are dropped from `BLOCKS`, but their metadata (level, existence)
/// stays queryable forever -- confirmation logic never needs body content.
#[test]
fn prune_drops_confirmed_bodies_but_keeps_meta_queryable() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let db = std::sync::Arc::new(PrismDb::open(dir.path()).expect("open PrismDb"));
    let mut chain = PrismChain::with_db(params(), db.clone());
    let genesis = chain.proposer_genesis;

    let tx = tx_block(1, genesis);
    let tx_hash = block_hash(&tx.header);
    chain.insert_block(tx);

    let prop = proposer_block(2, genesis, vec![tx_hash], vec![]);
    let prop_hash = block_hash(&prop.header);
    chain.insert_block(prop);

    assert!(db.get_block(&prop_hash).unwrap().is_some());
    assert!(db.get_block(&tx_hash).unwrap().is_some());

    // R-5: prune reads the ledger-recorded leader, not `level_proposer`.
    db.put_h256(argonaut_prism_db::columns::PROPOSER_LEADER_SEQUENCE, &1u64.to_le_bytes(), &prop_hash).unwrap();
    chain.prune(1_000_000, 1); // level 1 is confirmed; no voter chain deep enough to prune

    assert!(db.get_block(&prop_hash).unwrap().is_none(), "confirmed proposer body should be dropped");
    assert!(db.get_block(&tx_hash).unwrap().is_none(), "tx body referenced only by confirmed history should be dropped");
    assert_eq!(
        db.get_block_status(&prop_hash).unwrap(),
        Some(argonaut_prism_db::BlockStatus::Pruned)
    );

    // Meta survives: level/existence queries still work with the body gone.
    assert_eq!(chain.proposer_level(&prop_hash), Some(1));
    assert_eq!(chain.leader_at_level(1), Some(prop_hash));
    assert_eq!(chain.proposers_at_level(1), vec![prop_hash]);
    assert!(chain.has_block(&prop_hash));
    assert!(chain.has_block(&tx_hash));
}

/// B5 pruning + B-7 restart: after pruning, a restart must still converge --
/// this is the whole reason `ChainCheckpoint` exists (full replay would
/// otherwise try to re-derive state from bodies `prune` just deleted).
#[test]
fn restart_after_pruning_still_recovers_correctly() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let db = std::sync::Arc::new(PrismDb::open(dir.path()).expect("open PrismDb"));

    let mut chain = PrismChain::with_db(params(), db.clone());
    let proposer_genesis = chain.proposer_genesis;
    let voter_genesis0 = chain.voter_genesis[0];
    let prop = proposer_block(1, proposer_genesis, vec![], vec![]);
    let prop_hash = block_hash(&prop.header);
    chain.insert_block(prop);

    let mut nonce = 0u64;
    let mut tip = voter_genesis0;
    for i in 0..30u64 {
        let votes = if i == 0 { vec![prop_hash] } else { vec![] };
        tip = ins(&mut chain, tip, votes, &mut nonce);
    }
    let final_tip = tip;

    db.put_h256(argonaut_prism_db::columns::PROPOSER_LEADER_SEQUENCE, &1u64.to_le_bytes(), &prop_hash).unwrap(); // R-5
    chain.prune(10, 1); // prunes deep chain-0 history + prop's now-confirmed body
    let before_votes = chain.vote_totals();
    let before_tip_depth = chain.voter_tips[0].1;

    drop(chain);
    let recovered = PrismChain::with_db(params(), db);

    assert_eq!(recovered.voter_tips[0], (final_tip, before_tip_depth));
    assert_eq!(recovered.vote_totals(), before_votes);
    assert_eq!(recovered.proposer_level(&prop_hash), Some(1));
    assert!(recovered.has_block(&prop_hash), "pruned-but-confirmed proposer must still be known after restart");
}

/// B-5: per-chain genesis hashes make foreign-network and wrong-chain
/// blocks structurally rejected, not merely conventionally invalid.
#[test]
fn foreign_chain_spec_genesis_produces_unresolvable_orphans() {
    let mut params_a = params();
    params_a.chain_spec_genesis = H256::from([1u8; 32]);
    let mut params_b = params();
    params_b.chain_spec_genesis = H256::from([2u8; 32]);

    let chain_a = PrismChain::new(params_a);
    let chain_b = PrismChain::new(params_b);

    // Domain separation: same slot convention, different chain_spec_genesis
    // -> different genesis hashes, with overwhelming probability.
    assert_ne!(chain_a.proposer_genesis, chain_b.proposer_genesis);
    assert_ne!(chain_a.voter_genesis[0], chain_b.voter_genesis[0]);

    // A block built against network B's proposer genesis, fed to network A's
    // chain, doesn't resolve to anything -- it's simply unknown, not
    // accidentally accepted as valid ancestry.
    let mut chain_a = chain_a;
    let foreign = proposer_block(1, chain_b.proposer_genesis, vec![], vec![]);
    let outcome = chain_a.insert_block(foreign);
    assert_eq!(outcome.missing, vec![chain_b.proposer_genesis]);
    assert_eq!(chain_a.proposer_best_level, 0, "a foreign-genesis block must never be accepted");
}

/// B-5: a voter block claiming chain `i` but parented on chain `j`'s genesis
/// (a different chain of the *same* network) is rejected outright, not
/// orphaned -- this is exactly the cross-chain-contamination bug per-chain
/// genesis hashes make detectable at the genesis boundary for the first time.
#[test]
fn wrong_chain_genesis_parent_is_rejected_not_orphaned() {
    let mut chain = PrismChain::new(params());
    let voter_genesis1 = chain.voter_genesis[1];

    // Claims chain 0, but its parent is chain 1's genesis.
    let bad = voter_block(1, 0, voter_genesis1, vec![]);
    let bad_hash = block_hash(&bad.header);
    let outcome = chain.insert_block(bad);

    assert!(outcome.missing.is_empty(), "must be rejected outright, not treated as missing a fetch");
    assert!(!chain.orphans.contains_key(&bad_hash), "must not sit in orphans waiting on a parent that will never legitimately arrive");
    assert_eq!(chain.voter_tips[0].1, 0, "chain 0's tip must not have advanced");
}

/// B-13: the orphan pool is bounded (`MAX_ORPHANS`), evicts oldest-first once
/// full, and a *retained* (not yet evicted) orphan still resolves normally
/// once its parent arrives -- the cap doesn't break the ordinary
/// orphan-resolution path, only bounds it.
#[test]
fn orphan_pool_is_bounded_and_a_retained_orphan_still_resolves() {
    let mut chain = PrismChain::new(params());

    // A real (not-yet-inserted) future parent, so we can construct one
    // specific orphan whose parent we'll deliver later.
    let target = voter_block(999_999, 0, chain.voter_genesis[0], vec![]);
    let target_hash = block_hash(&target.header);

    // Fill well past MAX_ORPHANS with orphans that reference distinct,
    // never-to-arrive fake parents -- each `unknown_parent(i)` is a distinct
    // H256, so each of these is itself a distinct orphan (no dedup).
    fn unknown_parent(i: u64) -> H256 {
        let mut bytes = [0u8; 32];
        bytes[..8].copy_from_slice(&i.to_le_bytes());
        H256::from_slice(&bytes)
    }
    const FILL: u64 = 10_100; // MAX_ORPHANS (10_000) + 100 margin
    for i in 0..FILL {
        chain.insert_block(voter_block(i, 1, unknown_parent(i), vec![]));
    }
    assert!(chain.orphans.len() <= 10_000, "orphan pool must be capped, got {}", chain.orphans.len());

    // Inserted last -- must still be present (eviction is oldest-first).
    let retained = voter_block(1_000_000, 0, target_hash, vec![]);
    let retained_hash = block_hash(&retained.header);
    chain.insert_block(retained);
    assert!(
        chain.orphans.contains_key(&retained_hash),
        "the most-recently-inserted orphan must not have been evicted"
    );

    // Deliver its parent -- must resolve via drain_orphans, cap notwithstanding.
    chain.insert_block(target);
    assert!(
        !chain.orphans.contains_key(&retained_hash),
        "a retained orphan must still resolve once its parent arrives"
    );
    assert_eq!(chain.voter_tips[0].1, 2, "chain 0's tip must reflect target + the resolved orphan");
}

/// B-18: `orphan_missing_refs` reports exactly the absent parent of an
/// orphaned voter block (never the orphan's own hash), and is empty once the
/// parent arrives and the orphan resolves.
#[test]
fn orphan_missing_refs_reports_absent_parent_then_clears_on_resolve() {
    let mut chain = PrismChain::new(params());
    assert!(chain.orphan_missing_refs().is_empty());

    let parent = voter_block(1, 0, chain.voter_genesis[0], vec![]);
    let parent_hash = block_hash(&parent.header);
    let child = voter_block(2, 0, parent_hash, vec![]);
    let child_hash = block_hash(&child.header);

    chain.insert_block(child);
    assert!(chain.orphans.contains_key(&child_hash));
    assert_eq!(chain.orphan_missing_refs(), vec![parent_hash]);

    chain.insert_block(parent);
    assert!(!chain.orphans.contains_key(&child_hash));
    assert!(chain.orphan_missing_refs().is_empty());
}

/// B-18: two-deep -- orphan A needs P, P arrives but is itself an orphan
/// needing Q. The result contains Q but not P (P is an orphan, so `has_block`).
#[test]
fn orphan_missing_refs_reports_only_the_deepest_absent_ancestor() {
    let mut chain = PrismChain::new(params());

    let q = voter_block(1, 0, chain.voter_genesis[0], vec![]);
    let q_hash = block_hash(&q.header);
    let p = voter_block(2, 0, q_hash, vec![]);
    let p_hash = block_hash(&p.header);
    let a = voter_block(3, 0, p_hash, vec![]);

    chain.insert_block(a);
    assert_eq!(chain.orphan_missing_refs(), vec![p_hash]);

    chain.insert_block(p);
    let missing = chain.orphan_missing_refs();
    assert_eq!(missing, vec![q_hash], "must contain Q only, not orphaned P");

    chain.insert_block(q);
    assert!(chain.orphan_missing_refs().is_empty());
}

/// R-5: a confirmed level with no recorded leader is NOT pruned (never
/// guessed from the first-seen `level_proposer`), and a recorded leader that
/// differs from the first-seen proposer is the one swept.
#[test]
fn prune_uses_recorded_leader_and_skips_levels_without_one() {
    let dir = tempfile::tempdir().expect("create tempdir");
    let db = std::sync::Arc::new(PrismDb::open(dir.path()).expect("open PrismDb"));
    let mut chain = PrismChain::with_db(params(), db.clone());
    let genesis = chain.proposer_genesis;
    let first_seen = proposer_block(1, genesis, vec![], vec![]);
    let first_seen_hash = block_hash(&first_seen.header);
    chain.insert_block(first_seen);
    let real_leader = proposer_block(2, genesis, vec![], vec![]);
    let real_leader_hash = block_hash(&real_leader.header);
    chain.insert_block(real_leader);

    chain.prune(1_000_000, 1); // no leader recorded -> nothing pruned
    assert!(db.get_block(&first_seen_hash).unwrap().is_some());
    assert!(db.get_block(&real_leader_hash).unwrap().is_some());

    db.put_h256(argonaut_prism_db::columns::PROPOSER_LEADER_SEQUENCE, &1u64.to_le_bytes(), &real_leader_hash).unwrap();
    chain.prune(1_000_000, 1);
    assert!(db.get_block(&real_leader_hash).unwrap().is_none());
    assert!(db.get_block(&first_seen_hash).unwrap().is_some(), "first-seen non-leader must be kept");
}
