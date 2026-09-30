//! B-9 root-cause: "two honest miners never confirm" repro + explanation.
//!
//! Earlier live 2-miner devnet testing (see `docs/IMPLEMENTATION_AB_FIXPLAN.md`)
//! observed 100+ Prism blocks accepted without stage-1 ever confirming a
//! single level, at a scale where a single miner confirmed within ~60
//! blocks -- suspected, but not proven, to be the same mechanism as the
//! adversarial harness's deliberate balancing attack. It is not: no attacker
//! or reorg is needed here at all. Two honest miners can each independently
//! mine the first proposer block at a level before hearing about the other's;
//! since "vote for whichever proposer you saw first" is the paper's own
//! honest strategy and a chain's vote for a level is never revisited absent a
//! competing *reorg* (which a purely honest voter chain never initiates on
//! its own), the DAG's `m` voter chains permanently partition across the two
//! siblings the moment each chain casts its single vote. What happens next
//! depends entirely on the resulting split, proven below rather than
//! guessed:
//!
//! - An **exact tie** (e.g. m=4 split 2-2) stalls both stage-1 (obviously --
//!   unanimity is impossible) *and* stage-2, forever, by construction: every
//!   quantity `confirm_unique_leader` computes for one side has an exact
//!   mirror image on the other (same vote count, and -- because both sides'
//!   chains keep mining at the same rate -- the same depth too), so neither
//!   proposer's lower bound can ever strictly exceed the other's upper bound.
//!   More mining doesn't help; it grows both sides in lockstep.
//! - **Any real asymmetry**, even with zero malice and zero reorgs -- a 3-1
//!   honest split from pure bad luck in propagation timing -- resolves via
//!   stage-2 given enough depth, exactly like the adversarial balancing tests
//!   already demonstrate once an attacker "gives up".
//!
//! Practical takeaway (see `IMPLEMENTATION_AB_FIXPLAN.md` item 8): the live
//! stall was very likely an artifact of testing at small `m` (a handful of
//! voter chains), where an exact or near-exact split between two miners is
//! quite probable. At the paper's intended scale (`m` on the order of 100+),
//! the probability of a sustained exact tie across every one of `m`
//! independent chains is vanishingly small, and stage-2 (B-4) resolves any
//! real skew -- so this isn't a protocol fix so much as a scaling caveat:
//! don't run small-`m` multi-miner devnets and expect fast confirmation, and
//! don't be surprised if a *perfectly* symmetric 2-miner setup at tiny `m`
//! never confirms at all, by design of the confirmation math, not a bug in it.
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

fn setup(m: u16) -> (tempfile::TempDir, PrismChain, Arc<PrismDb>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Arc::new(PrismDb::open(dir.path()).expect("open PrismDb"));
    let chain = PrismChain::with_db(params(m), db.clone());
    (dir, chain, db)
}

/// Every chain in `chain_idxs` casts one vote for `proposer` (from genesis --
/// this is each chain's very first block) and is never touched again by this
/// call; further symmetric growth happens separately so both groups extend at
/// the same simulated rate, modeling two honest miner populations that never
/// reorg away from their first-seen proposer.
fn honest_group_votes_once(chain: &mut PrismChain, chain_idxs: &[ChainId], proposer: H256, nonce: &mut u64) {
    for &c in chain_idxs {
        let genesis = chain.voter_genesis[c as usize];
        *nonce += 1;
        chain.insert_block(voter_block(*nonce, c, genesis, vec![proposer]));
    }
}

/// Every chain in `chain_idxs` extends its own tip by one more voteless
/// block -- simulates continued honest mining with no new votes at this
/// level, growing each group's existing vote deeper.
fn extend_group(chain: &mut PrismChain, chain_idxs: &[ChainId], nonce: &mut u64) {
    for &c in chain_idxs {
        let tip = chain.voter_tips[c as usize].0;
        *nonce += 1;
        chain.insert_block(voter_block(*nonce, c, tip, vec![]));
    }
}

#[test]
fn exact_even_split_between_two_honest_proposers_never_confirms() {
    let (_dir, mut chain, db) = setup(4);
    let genesis = chain.proposer_genesis;
    let mut nonce = 1000u64;

    // Two honest miners each mine the level-1 proposer first, independently.
    let p1 = proposer_block(1, genesis);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);
    let p2 = proposer_block(2, genesis);
    let p2_hash = block_hash(&p2.header);
    chain.insert_block(p2);

    // m=4 voter chains split exactly 2-2 by which sibling they heard about
    // first -- no attacker, no reorg, just honest first-seen voting.
    honest_group_votes_once(&mut chain, &[0, 1], p1_hash, &mut nonce);
    honest_group_votes_once(&mut chain, &[2, 3], p2_hash, &mut nonce);

    let mut ledger = PrismLedger::new(db);
    for round in 0..50u64 {
        // Both groups keep mining at the same simulated rate -- depth grows
        // in lockstep on both sides, preserving the symmetry.
        extend_group(&mut chain, &[0, 1], &mut nonce);
        extend_group(&mut chain, &[2, 3], &mut nonce);

        assert_eq!(
            confirmed_leader_at_level(&chain, 1, 0),
            None,
            "round {round}: stage-1 can never reach unanimity across a permanent 2-2 split"
        );
        assert_eq!(
            ledger.advance(&chain, 0),
            vec![],
            "round {round}: stage-2 has no basis to favor either side of an exact, ever-growing tie"
        );
    }
}

#[test]
fn any_real_skew_in_an_honest_split_still_resolves_via_stage_2() {
    // Same setup as the tie above, except the split is 7-1 instead of 2-2 --
    // still purely honest (no attacker, no reorg, no vote ever changes once
    // cast), just a less even accident of propagation timing. Contrasts with
    // the tie test: a genuine asymmetry is enough for stage-2 to eventually
    // confirm.
    //
    // `m=8`, not the `m=4`/3-1 this test used before the eq. (35) private-
    // block bound was added to `confirm_unique_leader`: at m=4 a 3-vote
    // majority's lower bound tops out at `floor(3 - margin) = 2`, which is
    // exactly `v_bar_private = 4 - 2`, so a *withheld* proposer could still
    // tie it -- correctly unconfirmable at any depth, and the reason this
    // test passed only while that bound was missing. The claim under test is
    // about honest skew resolving via stage-2, not about the smallest `m`
    // where it does, so `m=8` (lower bound 6 vs `v_bar_private` 2) exercises
    // it with the margin the rule actually requires.
    let (_dir, mut chain, db) = setup(8);
    let genesis = chain.proposer_genesis;
    let mut nonce = 1000u64;

    let p1 = proposer_block(1, genesis);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);
    let p2 = proposer_block(2, genesis);
    let p2_hash = block_hash(&p2.header);
    chain.insert_block(p2);

    honest_group_votes_once(&mut chain, &[0, 1, 2, 3, 4, 5, 6], p1_hash, &mut nonce);
    honest_group_votes_once(&mut chain, &[7], p2_hash, &mut nonce);

    let mut ledger = PrismLedger::new(db);
    // B-15: post-B-15, stage2_enabled is on by default, but the real
    // (SingleCheck) margin is vacuous at the shallow depths (<50 rounds) this
    // test runs at -- pin the pre-B-15 placeholder explicitly so this test
    // still exercises stage-2 confirming a genuine (even 3-1) skew that
    // stage-1 alone never would (chain 3 dissents forever).
    let confirmation_params = confirmation::ConfirmationParams {
        stage2_enabled: true,
        margin: confirmation::MarginPolicy::Custom(confirmation::placeholder_delta_d),
        ..Default::default()
    };
    let mut confirmed_leader = None;
    for _ in 0..50u64 {
        extend_group(&mut chain, &[0, 1, 2, 3, 4, 5, 6], &mut nonce);
        extend_group(&mut chain, &[7], &mut nonce);

        let confirmed = ledger.advance_with_params(&chain, 0, &confirmation_params);
        if !confirmed.is_empty() {
            assert_eq!(confirmed.len(), 1);
            confirmed_leader = Some(confirmed[0].leader);
            break;
        }
    }
    assert_eq!(
        confirmed_leader,
        Some(p1_hash),
        "stage-2 should confirm the 7-vote majority within 50 rounds of purely honest mining"
    );
}
