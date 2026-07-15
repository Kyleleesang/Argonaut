//! Adversarial simulation harness (architecture guide §3.9 B4: "censorship +
//! balancing attackers as in paper §5, run in CI at small scale"), against
//! real `PrismChain` + `PrismLedger` state -- no mocked adversary, just the
//! same `insert_block`/`add_vote`/reorg machinery any honest node runs,
//! driven through attacker-chosen block sequences.
//!
//! # Balancing attack (paper §5, "Balancing Attack")
//! The paper's own description: an adversary controlling voter tree `i`
//! repeatedly forks it to keep switching its vote between two competing
//! proposer blocks, trying to prevent the level from ever reaching unanimous
//! agreement. Nothing about this requires new machinery -- `PrismChain`
//! already treats "vote for a sibling proposer" as ordinary (`add_vote` only
//! requires a *known* proposer hash, not the canonical one; honest miners
//! just never choose to reference siblings -- see `client/prism-miner`), and
//! vote-switching-via-reorg is exactly B1's already-tested rollback path.
//! What these tests add is checking *confirmation*'s behavior under it:
//! - stage-1 with an insufficient `k` can be tricked into confirming a
//!   leader that a moment later no longer has unanimous support (motivating
//!   why production defaults `k` to [`crate::K_DEEP_CONFIRMATION`], not 0);
//! - stage-1 with adequate `k` never does that, but can be stalled
//!   indefinitely by a persistent attacker (a documented liveness weakness
//!   the architecture guide's own stage-2 section exists to address).
//!
//! # Censorship attack
//! An adversarial proposer omits a reference to an honest tx block; the
//! paper's liveness argument (Theorem 4.2) says it eventually gets included
//! via some later, less-censored proposer. Checked here against
//! `PrismLedger::advance`'s actual dedup/attribution logic.
use super::*;
use argonaut_primitives_prism::{block_hash, ChainId, MerkleProof, Params, PrismBlock, SuperHeader};
use sp_core::U256;

fn params(m: u16) -> Params {
    let mut p = Params::testnet();
    p.m_voter_chains = m;
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

fn setup(m: u16) -> (tempfile::TempDir, PrismChain, Arc<PrismDb>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(PrismDb::open(dir.path()).expect("open PrismDb"));
    let chain = PrismChain::with_db(params(m), db.clone());
    (dir, chain, db)
}

/// Build and insert a fresh fork on `chain_idx`, from genesis, `length`
/// blocks long, where only the *first* block casts `first_vote` and the rest
/// are empty extensions -- the minimal shape needed to make this fork both
/// "votes differently" and "eventually longer than whatever it's replacing".
fn attacker_fork(
    chain: &mut PrismChain,
    chain_idx: ChainId,
    length: u64,
    first_vote: Vec<H256>,
    nonce: &mut u64,
) {
    let mut tip = chain.voter_genesis[chain_idx as usize];
    for i in 0..length {
        *nonce += 1;
        let votes = if i == 0 { first_vote.clone() } else { vec![] };
        let b = voter_block(*nonce, chain_idx, tip, votes);
        tip = block_hash(&b.header);
        chain.insert_block(b);
    }
}

#[test]
fn balancing_attack_with_zero_confirmation_depth_can_trick_the_ledger() {
    // Zero-depth confirmation is exactly what k-deep confirmation is
    // supposed to prevent trusting -- this test exists to demonstrate why,
    // not to suggest k=0 is a real deployment configuration.
    let (_dir, mut chain, db) = setup(4);
    let genesis = chain.proposer_genesis;
    let mut nonce = 1000u64;

    let p1 = proposer_block(1, genesis, vec![], vec![]);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);
    let p2 = proposer_block(2, genesis, vec![], vec![]);
    let p2_hash = block_hash(&p2.header);
    chain.insert_block(p2);

    // Chains 1-3 vote p1, honestly and permanently.
    for c in 1..4u16 {
        nonce += 1;
        chain.insert_block(voter_block(nonce, c, chain.voter_genesis[c as usize], vec![p1_hash]));
    }

    // The attacker's chain 0 momentarily also votes p1 -- unanimous for one instant.
    attacker_fork(&mut chain, 0, 1, vec![p1_hash], &mut nonce);
    assert_eq!(confirmed_leader_at_level(&chain, 1, 0), Some(p1_hash));

    let mut ledger = PrismLedger::new(db);
    let confirmed = ledger.advance(&chain, 0);
    assert_eq!(confirmed.len(), 1);
    assert_eq!(confirmed[0].leader, p1_hash);

    // The instant unanimity was a mirage: the attacker immediately reorgs
    // chain 0 away from p1. Chains 1-3 still back p1, so it's no longer
    // unanimous -- but the ledger already permanently committed p1's
    // (empty) transaction set moments earlier, based on a snapshot of
    // agreement that no longer holds.
    attacker_fork(&mut chain, 0, 2, vec![p2_hash], &mut nonce);
    assert_eq!(confirmed_leader_at_level(&chain, 1, 0), None); // p1 no longer unanimous
    assert_eq!(chain.votes.get(&p1_hash).map(|v| v.total()), Some(3)); // chains 1-3 only, now
    assert_eq!(chain.votes.get(&p2_hash).map(|v| v.total()), Some(1)); // chain 0 alone
    // The ledger has no way to undo what it already committed: `advance`
    // only ever moves `confirmed_tip` forward.
    assert_eq!(ledger.confirmed_tip(), 1);
    assert_eq!(ledger.advance(&chain, 0), vec![]); // level 1 is "done" as far as the ledger knows
}

#[test]
fn adequate_confirmation_depth_survives_the_same_balancing_attack() {
    let (_dir, mut chain, db) = setup(4);
    let genesis = chain.proposer_genesis;
    let mut nonce = 1000u64;
    const K: u64 = 5;

    let p1 = proposer_block(1, genesis, vec![], vec![]);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);
    let p2 = proposer_block(2, genesis, vec![], vec![]);
    let p2_hash = block_hash(&p2.header);
    chain.insert_block(p2);

    for c in 1..4u16 {
        nonce += 1;
        chain.insert_block(voter_block(nonce, c, chain.voter_genesis[c as usize], vec![p1_hash]));
    }

    let mut ledger = PrismLedger::new(db);

    // The attacker flips chain 0 back and forth several times. Each
    // successive fork must be strictly longer than the last to actually be
    // *adopted* (reorg only happens when `new_depth > tip_depth` --
    // repeating the same length from genesis would silently no-op instead
    // of genuinely flipping the vote), so track a monotonically increasing
    // depth budget across rounds.
    let mut depth = 0u64;
    for round in 0..4u64 {
        let vote = if round % 2 == 0 { p2_hash } else { p1_hash };
        depth += 1;
        attacker_fork(&mut chain, 0, depth, vec![vote], &mut nonce);
        assert_eq!(
            ledger.advance(&chain, K),
            vec![],
            "must not confirm while chain 0's vote keeps flipping (round {round})"
        );
    }

    // The attacker gives up; chain 0 settles on p1 and is left alone to
    // accumulate K honest extension blocks.
    depth += 1;
    attacker_fork(&mut chain, 0, depth, vec![p1_hash], &mut nonce);
    for _ in 0..K {
        for c in 0..4u16 {
            let tip = chain.voter_tips[c as usize].0;
            nonce += 1;
            chain.insert_block(voter_block(nonce, c, tip, vec![]));
        }
        // Still not confirmed until every chain's vote is truly K-deep.
    }

    let confirmed = ledger.advance(&chain, K);
    assert_eq!(confirmed.len(), 1);
    assert_eq!(confirmed[0].leader, p1_hash);
}

#[test]
fn persistent_balancing_attacker_stalls_stage1_alone_but_not_the_system() {
    // Documents a real liveness weakness of stage-1 (conservative k-deep) in
    // isolation: an attacker who controls even a single voter chain can keep
    // one level permanently unconfirmed *by stage-1's own rule* simply by
    // never letting that chain agree with the rest -- unanimity is
    // all-or-nothing, so depth on the *dissenting* vote doesn't help: chain
    // 0's vote for p2 can be extended arbitrarily deep and p1 never reaches
    // stage-1's k-deep unanimity, because p1's vote set can never include a
    // chain that's voting p2.
    //
    // This is exactly the gap stage-2 (confidence-interval confirmation,
    // `client/prism-ledger/src/confirmation.rs`) exists to close, and since
    // B-4 wired stage-2 into `PrismLedger::advance` (it tries
    // `confirm_unique_leader` before falling back to stage-1), the *system*
    // no longer has this weakness: once the honest chains' votes for p1 are
    // deep enough relative to the single shallow dissenting vote for p2,
    // stage-2 confirms p1 uniquely despite non-unanimous stage-1 agreement.
    // Both halves of this claim are checked explicitly below, against the
    // *same* attack sequence -- stage-1 in isolation stays stuck forever;
    // `advance()` (stage-1 *and* stage-2 together) does not.
    let (_dir, mut chain, db) = setup(4);
    let genesis = chain.proposer_genesis;
    let mut nonce = 1000u64;
    const K: u64 = 3;

    let p1 = proposer_block(1, genesis, vec![], vec![]);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);
    let p2 = proposer_block(2, genesis, vec![], vec![]);
    let p2_hash = block_hash(&p2.header);
    chain.insert_block(p2);

    for c in 1..4u16 {
        nonce += 1;
        chain.insert_block(voter_block(nonce, c, chain.voter_genesis[c as usize], vec![p1_hash]));
        // Give the honest chains plenty of depth -- irrelevant to stage-1
        // (chain 0 alone can block its unanimity regardless of how deep the
        // others are), but exactly what lets stage-2 tell p1's overwhelming,
        // deeply-rooted support apart from chain 0's shallow, isolated one.
        for _ in 0..(K + 5) {
            let tip = chain.voter_tips[c as usize].0;
            nonce += 1;
            chain.insert_block(voter_block(nonce, c, tip, vec![]));
        }
    }

    let mut ledger = PrismLedger::new(db);
    // B-12: stage2_enabled is off by default; this test's whole point is
    // demonstrating stage-2 rescuing the system from stage-1's liveness
    // weakness, so opt it on explicitly.
    let confirmation_params =
        confirmation::ConfirmationParams { stage2_enabled: true, ..Default::default() };

    // Chain 0 briefly agrees with the majority (not yet k-deep)...
    nonce += 1;
    chain.insert_block(voter_block(nonce, 0, chain.voter_genesis[0], vec![p1_hash]));
    assert_eq!(ledger.advance_with_params(&chain, K, &confirmation_params), vec![]);

    // ...then the attacker reorgs it onto a longer fork voting the minority
    // block instead (depth 2 > the depth-1 tip it replaces), and simply
    // keeps extending that same fork from then on -- no more reorgs needed.
    attacker_fork(&mut chain, 0, 2, vec![p2_hash], &mut nonce);

    let mut system_confirmed = false;
    for round in 0..20u64 {
        let tip = chain.voter_tips[0].0;
        nonce += 1;
        chain.insert_block(voter_block(nonce, 0, tip, vec![]));

        // Stage-1 in isolation: must NEVER confirm anything at level 1,
        // however deep chain 0's dissenting vote gets -- this is the
        // liveness weakness itself, unaffected by stage-2 existing.
        assert_eq!(
            confirmed_leader_at_level(&chain, 1, K),
            None,
            "round {round}: stage-1 alone must never confirm while chain 0 dissents, no matter its depth"
        );

        // The system as a whole (stage-2 first, stage-1 fallback): should
        // confirm p1 once the honest chains' depth advantage is enough for
        // stage-2's confidence interval to declare it the unique leader.
        let confirmed = ledger.advance_with_params(&chain, K, &confirmation_params);
        if !confirmed.is_empty() {
            assert_eq!(confirmed.len(), 1);
            assert_eq!(confirmed[0].leader, p1_hash);
            system_confirmed = true;
            break;
        }
    }
    assert!(
        system_confirmed,
        "stage-2 should have confirmed p1 as the unique leader well within 20 rounds"
    );
}

#[test]
fn censored_transaction_is_still_included_via_a_later_honest_proposer() {
    let (_dir, mut chain, db) = setup(3);
    let genesis = chain.proposer_genesis;
    let mut nonce = 1000u64;

    let censored_tx = tx_block(1, genesis, vec![vec![0xCE, 0x50, 0x2E]]);
    let censored_hash = block_hash(&censored_tx.header);
    chain.insert_block(censored_tx);

    // Adversarial proposer at level 1 references *no* tx blocks at all --
    // censoring the honest transaction outright.
    let p1 = proposer_block(2, genesis, vec![], vec![]);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);
    for c in 0..3u16 {
        nonce += 1;
        chain.insert_block(voter_block(nonce, c, chain.voter_genesis[c as usize], vec![p1_hash]));
    }

    let mut ledger = PrismLedger::new(db);
    let level1 = ledger.advance(&chain, 0);
    assert_eq!(level1.len(), 1);
    assert_eq!(level1[0].txs, Vec::<Vec<u8>>::new(), "censored tx must not appear at level 1");

    // A later, honest proposer at level 2 does reference it.
    let p2 = proposer_block(3, p1_hash, vec![censored_hash], vec![]);
    let p2_hash = block_hash(&p2.header);
    chain.insert_block(p2);
    for c in 0..3u16 {
        let tip = chain.voter_tips[c as usize].0;
        nonce += 1;
        chain.insert_block(voter_block(nonce, c, tip, vec![p2_hash]));
    }

    let level2 = ledger.advance(&chain, 0);
    assert_eq!(level2.len(), 1);
    assert_eq!(level2[0].leader, p2_hash);
    assert_eq!(level2[0].txs, vec![vec![0xCE, 0x50, 0x2E]], "censored tx surfaces once an honest proposer includes it");
}
