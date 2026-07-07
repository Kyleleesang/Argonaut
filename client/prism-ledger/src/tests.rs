//! BuildLedger tests against a real `PrismChain` + `PrismDb` (tempfile-backed,
//! no mocking) -- the DFS ref-expansion, cross-level dedup, and confirmed-level
//! sequencing are the parts this crate actually adds on top of
//! `PrismChain::confirmed_leader_at_level` (already covered by prism-chain's
//! own tests).
use super::*;
use argonaut_prism_chain::genesis_sentinel;
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
fn advance_returns_nothing_before_any_level_is_confirmed() {
    let (_dir, chain, db) = setup();
    let mut ledger = PrismLedger::new(db);
    assert_eq!(ledger.advance(&chain, 0), vec![]);
    assert_eq!(ledger.confirmed_tip(), 0);
}

#[test]
fn build_ledger_delta_expands_direct_refs_in_order_and_dedups_within_a_level() {
    let (_dir, mut chain, db) = setup();
    let genesis = genesis_sentinel();

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
    let genesis = genesis_sentinel();
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
fn build_ledger_delta_follows_indirect_refs_and_dedups_across_levels() {
    let (_dir, mut chain, db) = setup();
    let genesis = genesis_sentinel();

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
    // [1] deduped away (already emitted at level 1); only [3] is genuinely new.
    assert_eq!(second[0].txs, vec![vec![3]]);
}

#[test]
fn advance_persists_leader_sequence_and_ledger_order_to_db() {
    let (_dir, mut chain, db) = setup();
    let genesis = genesis_sentinel();
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
    let genesis = genesis_sentinel();
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
