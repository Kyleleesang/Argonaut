//! BuildLedger tests against a real `PrismChain` + `PrismDb` (tempfile-backed,
//! no mocking) -- the DFS ref-expansion, cross-level dedup, and confirmed-level
//! sequencing are the parts this crate actually adds on top of
//! `PrismChain::confirmed_leader_at_level` (already covered by prism-chain's
//! own tests).
use super::*;
use argonaut_primitives_prism::{block_hash, ChainId, MerkleProof, Params, PrismBlock, SuperHeader};
use sp_core::U256;

fn params() -> Params {
    let mut p = Params::testnet();
    p.m_voter_chains = 4;
    p
}

fn header(nonce: u64) -> SuperHeader {
    SuperHeader { parents_root: H256::zero(), content_root: H256::zero(), timestamp_ms: 0, nonce, difficulty: U256::max_value() }
}

fn tx_block(nonce: u64, parent: H256, txs: Vec<Vec<u8>>) -> PrismBlock {
    PrismBlock {
        header: header(nonce),
        parent,
        content: BlockContent::Transaction { txs },
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

fn setup() -> (tempfile::TempDir, PrismChain, Arc<PrismDb>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(PrismDb::open(dir.path()).expect("open PrismDb"));
    let chain = PrismChain::with_db(params(), db.clone());
    (dir, chain, db)
}

/// Every voter chain casts a vote for `proposer_hash`, so the level it lives
/// at becomes confirmable at k=0 (each vote is 0-deep on a depth-1 tip).
fn vote_from_every_chain(chain: &mut PrismChain, nonce_start: u64, proposer_hash: H256) {
    for c in 0..4u16 {
        let tip = chain.voter_tips[c as usize].0;
        chain.insert_block(voter_block(nonce_start + c as u64, c, tip, vec![proposer_hash]));
    }
}

#[test]
fn advance_reports_the_leader_blocks_own_timestamp() {
    // ConfirmedLevel.timestamp_ms must come from the leader PrismBlock's own
    // header (deterministic across nodes), not local wall-clock time -- see
    // node/src/bridge.rs, which was found live to use SystemTime::now() and
    // so produced different block hashes on different honest nodes.
    let (_dir, mut chain, db) = setup();
    let genesis = chain.proposer_genesis;
    let mut p1 = proposer_block(1, genesis, vec![], vec![]);
    p1.header.timestamp_ms = 424_242;
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);
    vote_from_every_chain(&mut chain, 100, p1_hash);

    let mut ledger = PrismLedger::new(db);
    let confirmed = ledger.advance(&chain, 0);
    assert_eq!(confirmed.len(), 1);
    assert_eq!(confirmed[0].timestamp_ms, 424_242);
}

#[test]
fn advance_returns_nothing_before_any_level_is_confirmed() {
    let (_dir, chain, db) = setup();
    let mut ledger = PrismLedger::new(db);
    assert_eq!(ledger.advance(&chain, 0), vec![]);
    assert_eq!(ledger.confirmed_tip(), 0);
}

#[test]
fn build_ledger_delta_expands_direct_refs_in_order_and_dedups_within_a_level() {
    let (_dir, mut chain, db) = setup();
    let genesis = chain.proposer_genesis;

    // p1 directly references two tx blocks, t1 then t2.
    let t1 = tx_block(1, genesis, vec![vec![10], vec![11]]);
    let t1_hash = block_hash(&t1.header);
    let t2 = tx_block(2, genesis, vec![vec![12]]);
    let t2_hash = block_hash(&t2.header);
    chain.insert_block(t1);
    chain.insert_block(t2);

    let p1 = proposer_block(3, genesis, vec![t1_hash, t2_hash], vec![]);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);

    vote_from_every_chain(&mut chain, 100, p1_hash);

    let mut ledger = PrismLedger::new(db);
    let confirmed = ledger.advance(&chain, 0);

    assert_eq!(confirmed.len(), 1);
    assert_eq!(confirmed[0].level, 1);
    assert_eq!(confirmed[0].leader, p1_hash);
    // Ref order: t1's txs (in their own listed order), then t2's.
    assert_eq!(confirmed[0].txs, vec![vec![10], vec![11], vec![12]]);
    assert_eq!(ledger.confirmed_tip(), 1);
}

#[test]
fn advance_is_idempotent_once_no_new_level_is_confirmable() {
    let (_dir, mut chain, db) = setup();
    let genesis = chain.proposer_genesis;
    let t1 = tx_block(1, genesis, vec![vec![1]]);
    let t1_hash = block_hash(&t1.header);
    chain.insert_block(t1);
    let p1 = proposer_block(2, genesis, vec![t1_hash], vec![]);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);
    vote_from_every_chain(&mut chain, 100, p1_hash);

    let mut ledger = PrismLedger::new(db);
    assert_eq!(ledger.advance(&chain, 0).len(), 1);
    // No new proposer/votes since -- nothing further to confirm.
    assert_eq!(ledger.advance(&chain, 0), vec![]);
    assert_eq!(ledger.confirmed_tip(), 1);
}

#[test]
fn build_ledger_delta_follows_indirect_refs_and_does_not_dedup_across_levels() {
    let (_dir, mut chain, db) = setup();
    let genesis = chain.proposer_genesis;

    let t1 = tx_block(1, genesis, vec![vec![1], vec![2]]);
    let t1_hash = block_hash(&t1.header);
    chain.insert_block(t1);
    let p1 = proposer_block(2, genesis, vec![t1_hash], vec![]);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);
    vote_from_every_chain(&mut chain, 100, p1_hash);

    let mut ledger = PrismLedger::new(db.clone());
    let first = ledger.advance(&chain, 0);
    assert_eq!(first[0].txs, vec![vec![1], vec![2]]);

    // p2 references t3 directly (which repeats tx [1], already emitted at
    // level 1) and p1 indirectly via prop_refs (whose t1 refs are therefore
    // walked again but must not be re-emitted).
    let t3 = tx_block(3, p1_hash, vec![vec![1], vec![3]]);
    let t3_hash = block_hash(&t3.header);
    chain.insert_block(t3);
    let p2 = proposer_block(4, p1_hash, vec![t3_hash], vec![p1_hash]);
    let p2_hash = block_hash(&p2.header);
    chain.insert_block(p2);
    vote_from_every_chain(&mut chain, 200, p2_hash);

    let second = ledger.advance(&chain, 0);
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].level, 2);
    assert_eq!(second[0].leader, p2_hash);
    // R-6: no cross-level dedup. [1] (repeated by t3) IS emitted again at
    // level 2 (the runtime rejects it by nonce); p1's already-settled subtree
    // (t1 = [1],[2]) is not re-walked, since p1 is level 1's confirmed leader.
    assert_eq!(second[0].txs, vec![vec![1], vec![3]]);
}

#[test]
fn advance_persists_leader_sequence_and_ledger_order_to_db() {
    let (_dir, mut chain, db) = setup();
    let genesis = chain.proposer_genesis;
    let p1 = proposer_block(1, genesis, vec![], vec![]);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);
    vote_from_every_chain(&mut chain, 100, p1_hash);

    let mut ledger = PrismLedger::new(db.clone());
    ledger.advance(&chain, 0);

    assert_eq!(
        db.get_h256(columns::PROPOSER_LEADER_SEQUENCE, &1u64.to_le_bytes()).unwrap(),
        Some(p1_hash)
    );
    assert_eq!(
        db.get_h256(columns::PROPOSER_LEDGER_ORDER, &1u64.to_le_bytes()).unwrap(),
        Some(p1_hash)
    );
}

#[test]
fn advance_respects_k_deep_and_confirms_once_votes_are_deep_enough() {
    let (_dir, mut chain, db) = setup();
    let genesis = chain.proposer_genesis;
    let p1 = proposer_block(1, genesis, vec![], vec![]);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);
    vote_from_every_chain(&mut chain, 100, p1_hash);

    let mut ledger = PrismLedger::new(db);
    // Votes are only depth-1 on a depth-1 tip (0-deep); not 3-deep yet.
    assert_eq!(ledger.advance(&chain, 3), vec![]);

    // Extend every chain 3 further voteless blocks (each needs a distinct
    // nonce -- these test headers hash purely off `nonce`, so reusing one
    // across iterations would silently no-op instead of advancing the tip).
    let mut nonce = 2000u64;
    for _ in 0..3 {
        for c in 0..4u16 {
            let tip = chain.voter_tips[c as usize].0;
            nonce += 1;
            chain.insert_block(voter_block(nonce, c, tip, vec![]));
        }
    }
    let confirmed = ledger.advance(&chain, 3);
    assert_eq!(confirmed.len(), 1);
    assert_eq!(confirmed[0].leader, p1_hash);
}

// AF-2: migrated from `client/prism-chain/src/tests.rs` alongside moving
// `confirmed_leader_at_level` itself into this crate.

#[test]
fn confirmed_leader_at_level_none_for_unknown_level() {
    let (_dir, chain, _db) = setup();
    assert_eq!(confirmed_leader_at_level(&chain, 1, 0), None);
}

#[test]
fn confirmed_leader_at_level_requires_every_chain_to_vote() {
    let (_dir, mut chain, _db) = setup();
    let genesis = chain.proposer_genesis;

    let p1 = proposer_block(1, genesis, vec![], vec![]);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);

    // Only 3 of the 4 chains (params() uses m=4) vote.
    chain.insert_block(voter_block(101, 0, chain.voter_genesis[0], vec![p1_hash]));
    chain.insert_block(voter_block(102, 1, chain.voter_genesis[1], vec![p1_hash]));
    chain.insert_block(voter_block(103, 2, chain.voter_genesis[2], vec![p1_hash]));

    assert_eq!(confirmed_leader_at_level(&chain, 1, 0), None);
}

#[test]
fn confirmed_leader_at_level_becomes_confirmed_once_votes_are_k_deep() {
    let (_dir, mut chain, _db) = setup();
    let genesis = chain.proposer_genesis;

    let p1 = proposer_block(1, genesis, vec![], vec![]);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);

    let mut nonce = 300u64;
    for c in 0..4u16 {
        nonce += 1;
        chain.insert_block(voter_block(nonce, c, chain.voter_genesis[c as usize], vec![p1_hash]));
    }
    // Every chain has voted (satisfies k=0), but each vote is only depth 1 on
    // a depth-1 tip -- 0 blocks behind, so not yet 3-deep.
    assert_eq!(confirmed_leader_at_level(&chain, 1, 0), Some(p1_hash));
    assert_eq!(confirmed_leader_at_level(&chain, 1, 3), None);

    // Extend every chain 3 further (voteless) blocks, so the original vote is
    // now exactly 3 blocks behind each chain's tip.
    for _ in 0..3 {
        for c in 0..4u16 {
            let tip = chain.voter_tips[c as usize].0;
            nonce += 1;
            chain.insert_block(voter_block(nonce, c, tip, vec![]));
        }
    }

    assert_eq!(confirmed_leader_at_level(&chain, 1, 3), Some(p1_hash));
}

// B-7: `PrismLedger::recover` restart-correctness tests. Without it, a
// restarted bridge would re-confirm already-settled levels from scratch
// (`PrismLedger::new` always starts `confirmed_tip` at 0) and re-emit their
// txs as brand-new extrinsics on top of the real chain tip -- these check
// both halves: the cursor doesn't rewind, and the cross-level tx-dedup state
// carries over so a tx repeated at a level confirmed *after* the "restart"
// isn't re-emitted just because the in-memory ledger is fresh.

#[test]
fn recover_does_not_rewind_the_confirmed_cursor() {
    let (_dir, mut chain, db) = setup();
    let genesis = chain.proposer_genesis;
    let p1 = proposer_block(1, genesis, vec![], vec![]);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);
    vote_from_every_chain(&mut chain, 100, p1_hash);

    let mut ledger = PrismLedger::new(db.clone());
    assert_eq!(ledger.advance(&chain, 0).len(), 1);
    assert_eq!(ledger.confirmed_tip(), 1);
    drop(ledger); // simulate the process (and this in-memory ledger) going away

    // `1` stands in for `client.info().best_number` in `node/src/bridge.rs`:
    // one Substrate block per confirmed level, so the chain height after one
    // confirmation is 1.
    let mut recovered = PrismLedger::recover(db, 1);
    assert_eq!(recovered.confirmed_tip(), 1);
    // Nothing new to confirm -- a fresh `PrismLedger::new` would instead
    // re-confirm level 1 all over again here, which is exactly the bug.
    assert_eq!(recovered.advance(&chain, 0), vec![]);
    assert_eq!(recovered.confirmed_tip(), 1);
}

#[test]
fn recover_does_not_carry_cross_level_tx_dedup_state() {
    let (_dir, mut chain, db) = setup();
    let genesis = chain.proposer_genesis;

    let t1 = tx_block(1, genesis, vec![vec![1]]);
    let t1_hash = block_hash(&t1.header);
    chain.insert_block(t1);
    let p1 = proposer_block(2, genesis, vec![t1_hash], vec![]);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);
    vote_from_every_chain(&mut chain, 100, p1_hash);

    let t2 = tx_block(3, p1_hash, vec![vec![2]]);
    let t2_hash = block_hash(&t2.header);
    chain.insert_block(t2);
    let p2 = proposer_block(4, p1_hash, vec![t2_hash], vec![p1_hash]);
    let p2_hash = block_hash(&p2.header);
    chain.insert_block(p2);
    vote_from_every_chain(&mut chain, 200, p2_hash);

    let mut ledger = PrismLedger::new(db.clone());
    let confirmed = ledger.advance(&chain, 0);
    assert_eq!(confirmed.len(), 2, "p1 and p2 both become confirmable in the same `advance` call");
    assert_eq!(ledger.confirmed_tip(), 2);
    drop(ledger); // "restart": this ledger's in-memory state is gone

    let mut recovered = PrismLedger::recover(db, 2); // 2 confirmed levels so far -> chain height 2
    assert_eq!(recovered.confirmed_tip(), 2);

    // p3 repeats tx [1] (already emitted at level 1) and introduces tx [3].
    let t3 = tx_block(5, p2_hash, vec![vec![1], vec![3]]);
    let t3_hash = block_hash(&t3.header);
    chain.insert_block(t3);
    let p3 = proposer_block(6, p2_hash, vec![t3_hash], vec![p2_hash]);
    let p3_hash = block_hash(&p3.header);
    chain.insert_block(p3);
    vote_from_every_chain(&mut chain, 300, p3_hash);

    let third = recovered.advance(&chain, 0);
    assert_eq!(third.len(), 1);
    assert_eq!(third[0].level, 3);
    assert_eq!(third[0].leader, p3_hash);
    // R-6: no persisted dedup -- [1] is emitted again (runtime rejects by nonce).
    assert_eq!(third[0].txs, vec![vec![1], vec![3]]);
}

// R-6: a crash between `advance` and the Substrate import must not change
// what level `tip` contains when it is re-confirmed after restart.
#[test]
fn recover_after_crash_reemits_the_uncommitted_level_with_identical_txs() {
    let (_dir, mut chain, db) = setup();
    let genesis = chain.proposer_genesis;
    let t1 = tx_block(1, genesis, vec![vec![1]]);
    let t1_hash = block_hash(&t1.header);
    chain.insert_block(t1);
    let p1 = proposer_block(2, genesis, vec![t1_hash], vec![]);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);
    vote_from_every_chain(&mut chain, 100, p1_hash);
    let t2 = tx_block(3, p1_hash, vec![vec![2], vec![3]]);
    let t2_hash = block_hash(&t2.header);
    chain.insert_block(t2);
    let p2 = proposer_block(4, p1_hash, vec![t2_hash], vec![p1_hash]);
    let p2_hash = block_hash(&p2.header);
    chain.insert_block(p2);
    vote_from_every_chain(&mut chain, 200, p2_hash);

    let mut ledger = PrismLedger::new(db.clone());
    let first_run = ledger.advance(&chain, 0);
    assert_eq!(first_run.len(), 2);
    let level2 = first_run[1].clone();
    drop(ledger); // crash: level 2 was built but its block never imported

    let mut recovered = PrismLedger::recover(db, 1); // best_number = 1
    let again = recovered.advance(&chain, 0);
    assert_eq!(again, vec![level2.clone()]);
    assert_eq!(level2.txs, vec![vec![2], vec![3]]);
}

#[test]
fn same_tx_within_one_level_is_emitted_once() {
    let (_dir, mut chain, db) = setup();
    let genesis = chain.proposer_genesis;
    let t1 = tx_block(1, genesis, vec![vec![7], vec![8]]);
    let t1_hash = block_hash(&t1.header);
    chain.insert_block(t1);
    let t2 = tx_block(2, genesis, vec![vec![7], vec![9]]);
    let t2_hash = block_hash(&t2.header);
    chain.insert_block(t2);
    let p1 = proposer_block(3, genesis, vec![t1_hash, t2_hash], vec![]);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);
    vote_from_every_chain(&mut chain, 100, p1_hash);
    let mut ledger = PrismLedger::new(db);
    let out = ledger.advance(&chain, 0);
    assert_eq!(out[0].txs, vec![vec![7], vec![8], vec![9]]);
}

// R-5: a missing tx-block body halts the ledger without advancing or
// persisting anything for that level.
#[test]
fn advance_halts_when_a_needed_body_is_missing() {
    let (_dir, mut chain, db) = setup();
    let genesis = chain.proposer_genesis;
    let t1 = tx_block(1, genesis, vec![vec![1]]);
    let t1_hash = block_hash(&t1.header);
    chain.insert_block(t1);
    let p1 = proposer_block(2, genesis, vec![t1_hash], vec![]);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);
    vote_from_every_chain(&mut chain, 100, p1_hash);
    db.delete_block_body(&t1_hash).unwrap();
    let mut ledger = PrismLedger::new(db.clone());
    assert_eq!(ledger.advance(&chain, 0), vec![]);
    assert_eq!(ledger.confirmed_tip(), 0);
    assert_eq!(db.get_h256(columns::PROPOSER_LEADER_SEQUENCE, &1u64.to_le_bytes()).unwrap(), None);
}
