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
fn proposers_at_level_includes_siblings_never_added_to_unvoted_levels() {
    let mut chain = PrismChain::new(params());
    let genesis = genesis_sentinel();
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
    assert_eq!(chain.votes.get(&p1_hash).map(|v| v.total()), Some(1));
    assert_eq!(chain.votes.get(&p2_hash).map(|v| v.total()), Some(1));

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
    let genesis = genesis_sentinel();
    let p1 = proposer_block(1, genesis, vec![], vec![]);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);

    // Chain 1 votes for p1 and must stay untouched by chain 0's reorg below.
    let v_chain1 = voter_block(50, 1, genesis, vec![p1_hash]);
    chain.insert_block(v_chain1);
    assert_eq!(chain.votes.get(&p1_hash).map(|v| v.total()), Some(1));

    let mut nonce = 0;
    ins(&mut chain, genesis, vec![p1_hash], &mut nonce);
    assert_eq!(chain.votes.get(&p1_hash).map(|v| v.total()), Some(2));

    // A longer competing fork on chain 0 that casts no votes at all.
    let c1 = ins(&mut chain, genesis, vec![], &mut nonce);
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
    assert_eq!(chain.votes.get(&proposers[2]).map(|v| v.total()), Some(1));
    assert_eq!(chain.votes.get(&proposers[3]).map(|v| v.total()), Some(1));
    assert_eq!(chain.votes.get(&proposers[4]).map(|v| v.total()), Some(1));

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
    let mut chain = PrismChain::new(params());
    let genesis = genesis_sentinel();
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
    let genesis = genesis_sentinel();

    let mut chain = PrismChain::with_db(params(), db.clone());

    // Proposer chain, with a tx block referenced by the second proposer.
    let p1 = proposer_block(1, genesis, vec![], vec![]);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);

    let tx = tx_block(2, genesis);
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
    let unreferenced_tx = tx_block(5, genesis);
    let unreferenced_tx_hash = block_hash(&unreferenced_tx.header);
    chain.insert_block(unreferenced_tx);

    // A voter-chain reorg (same shape as `voter_chain_reorg_rolls_back_and_reapplies_votes`)
    // so restart recovery is checked against non-trivial vote accounting, not
    // just a freshly-accepted, never-superseded vote.
    let mut nonce = 0u64;
    let a1 = ins(&mut chain, genesis, vec![p1_hash], &mut nonce);
    ins(&mut chain, a1, vec![p2_hash], &mut nonce);
    let b1 = ins(&mut chain, genesis, vec![p1_hash], &mut nonce);
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
    let genesis = genesis_sentinel();
    let mut nonce = 0u64;

    let tx = tx_block(1, genesis);
    let tx_hash = block_hash(&tx.header);
    chain.insert_block(tx.clone());

    let prop = proposer_block(2, genesis, vec![tx_hash], vec![]);
    let prop_hash = block_hash(&prop.header);
    chain.insert_block(prop.clone());

    let voter_hash = ins(&mut chain, genesis, vec![prop_hash], &mut nonce);

    // An orphan: a proposer block whose parent is never accepted.
    let dangling_parent = H256::from([7u8; 32]);
    let orphan = proposer_block(3, dangling_parent, vec![], vec![]);
    let orphan_hash = block_hash(&orphan.header);
    chain.insert_block(orphan);

    assert!(chain.has_block(&genesis));
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
    let genesis = genesis_sentinel();

    let prop = proposer_block(1, genesis, vec![], vec![]);
    let prop_hash = block_hash(&prop.header);
    chain.insert_block(prop);

    // 30-deep chain 0; only the first block casts a vote, so that vote's
    // casting block (depth 1) is exactly what a k_prune=10 frontier (tip
    // depth 30 -> frontier 20) should sweep.
    let mut nonce = 0u64;
    let mut tip = genesis;
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
    let genesis = genesis_sentinel();

    let mut nonce = 0u64;
    let mut hashes = Vec::new();
    let mut tip = genesis;
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
    let genesis = genesis_sentinel();

    let tx = tx_block(1, genesis);
    let tx_hash = block_hash(&tx.header);
    chain.insert_block(tx);

    let prop = proposer_block(2, genesis, vec![tx_hash], vec![]);
    let prop_hash = block_hash(&prop.header);
    chain.insert_block(prop);

    assert!(db.get_block(&prop_hash).unwrap().is_some());
    assert!(db.get_block(&tx_hash).unwrap().is_some());

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
    let genesis = genesis_sentinel();

    let mut chain = PrismChain::with_db(params(), db.clone());
    let prop = proposer_block(1, genesis, vec![], vec![]);
    let prop_hash = block_hash(&prop.header);
    chain.insert_block(prop);

    let mut nonce = 0u64;
    let mut tip = genesis;
    for i in 0..30u64 {
        let votes = if i == 0 { vec![prop_hash] } else { vec![] };
        tip = ins(&mut chain, tip, votes, &mut nonce);
    }
    let final_tip = tip;

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
