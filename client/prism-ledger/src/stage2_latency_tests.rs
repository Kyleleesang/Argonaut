//! B-4: measure whether stage-2 confirms levels faster than stage-1 alone.
//!
//! Deliberately NOT a live-devnet measurement. A real 2026-07-10 devnet run
//! showed confirmation there is mining-dominated (blocks every ~40s-3min on
//! one CPU at m=100) -- stage-2's sub-second rule-latency edge is swamped by
//! minutes of mining noise, so a wall-clock devnet comparison would measure
//! CPU speed, not the confirmation rule. What B-4 actually asks is: *for the
//! same vote-arrival pattern*, does stage-2 confirm at an earlier point in
//! that pattern than stage-1 would? That's answerable deterministically and
//! reproducibly with direct `insert_block` calls (real timestamps, no PoW),
//! the same harness pattern `honest_split_tests.rs`/`adversarial_tests.rs`
//! already use -- no live devnet, no hardware noise.
//!
//! Scenario, at the real deployment scale (m=100, matching
//! `Params::testnet()`, not a toy m): a supermajority (92/100 chains) votes
//! for the level-1 proposer promptly; the remaining 8 chains never vote at
//! all in the test's window, modeling genuinely offline/lagging nodes -- a
//! realistic condition at m=100 that stage-1's unanimity requirement (B-15)
//! cannot tolerate no matter how long you wait, but stage-2 (B-4) is
//! specifically designed to route around once the majority's lead is
//! statistically decisive.
//!
//! Both rules are computed independently every round (not through
//! `advance`'s `stage2.or_else(stage1)` short-circuit, which would hide
//! stage-1's own status once stage-2 fires first) so the round each rule
//! *individually* first confirms is directly comparable.
use super::*;
use argonaut_primitives_prism::{block_hash, ChainId, MerkleProof, Params, PrismBlock, SuperHeader};
use sp_core::U256;

const M: u16 = 100; // real deployment scale, not a toy m
const VOTING_CHAINS: u16 = 92; // supermajority that votes promptly
const ROUNDS: u64 = 200;

fn params() -> Params {
    let mut p = Params::testnet();
    p.m_voter_chains = M;
    p
}

/// One simulated block-interval per round, matching `Params::testnet()`'s
/// own `target_interval_ms` -- so the round delta below is directly a
/// wall-clock-under-normal-operation delta, not an arbitrary step count.
const ROUND_MS: u64 = 1_000;

fn header(nonce: u64, timestamp_ms: u64) -> SuperHeader {
    SuperHeader {
        parents_root: H256::zero(),
        content_root: H256::zero(),
        timestamp_ms,
        nonce,
        difficulty: U256::max_value(),
    }
}

fn proposer_block(nonce: u64, parent: H256, timestamp_ms: u64) -> PrismBlock {
    PrismBlock {
        header: header(nonce, timestamp_ms),
        parent,
        content: BlockContent::Proposer { tx_refs: vec![], prop_refs: vec![] },
        parent_proof: MerkleProof::default(),
        content_proof: MerkleProof::default(),
    }
}

fn voter_block(nonce: u64, chain: ChainId, voter_parent: H256, votes: Vec<H256>, timestamp_ms: u64) -> PrismBlock {
    PrismBlock {
        header: header(nonce, timestamp_ms),
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

#[test]
fn stage2_confirms_a_92_of_100_majority_far_sooner_than_stage1_ever_could() {
    let (_dir, mut chain, _db) = setup();
    let genesis = chain.proposer_genesis;
    let mut nonce = 1_000u64;

    let p1 = proposer_block(1, genesis, 0);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);

    // Round 0: the 92-chain supermajority votes for p1 immediately (their
    // very first block). Chains 92..100 never vote in this test at all --
    // modeling nodes that are offline/lagging for the whole window, which
    // stage-1's unanimity requirement cannot tolerate at any depth.
    for c in 0..VOTING_CHAINS {
        nonce += 1;
        let genesis_c = chain.voter_genesis[c as usize];
        chain.insert_block(voter_block(nonce, c, genesis_c, vec![p1_hash], 0));
    }

    let stage2_params =
        confirmation::ConfirmationParams { stage2_enabled: true, ..Default::default() };

    let mut stage1_confirmed_at: Option<u64> = None;
    let mut stage2_confirmed_at: Option<u64> = None;

    for round in 1..=ROUNDS {
        let ts = round * ROUND_MS;
        // The supermajority keeps mining (depth grows); the 8 silent chains
        // never produce a block at all this test -- genuinely offline, not
        // just slow.
        for c in 0..VOTING_CHAINS {
            let tip = chain.voter_tips[c as usize].0;
            nonce += 1;
            chain.insert_block(voter_block(nonce, c, tip, vec![], ts));
        }

        if stage1_confirmed_at.is_none()
            && confirmed_leader_at_level(&chain, 1, K_DEEP_CONFIRMATION).is_some()
        {
            stage1_confirmed_at = Some(round);
        }
        if stage2_confirmed_at.is_none()
            && confirmation::confirm_unique_leader(&chain, 1, &stage2_params).is_some()
        {
            stage2_confirmed_at = Some(round);
        }
        if stage1_confirmed_at.is_some() {
            break; // both rules have had their chance; stop early
        }
    }

    println!(
        "B-4 measurement (m={M}, {VOTING_CHAINS}/{M} voting, {} silent, {ROUND_MS}ms/round): \
         stage-2 confirmed at round {:?} ({:?} ms); stage-1 confirmed at round {:?} (within {ROUNDS} rounds)",
        M - VOTING_CHAINS,
        stage2_confirmed_at,
        stage2_confirmed_at.map(|r| r * ROUND_MS),
        stage1_confirmed_at,
    );

    assert!(
        stage2_confirmed_at.is_some(),
        "stage-2 should confirm the 92/100 majority within {ROUNDS} rounds"
    );
    assert_eq!(
        stage1_confirmed_at, None,
        "stage-1 must NEVER confirm while 8 chains stay silent -- this is the unanimity \
         weakness (B-15) stage-2 exists to route around, not a bug in this test"
    );
}

/// A second data point: what if the 8 stragglers are merely *slow*, not
/// permanently offline -- they all vote at round 30. Stage-1 can then
/// eventually confirm (once K_DEEP_CONFIRMATION-deep past round 30), so this
/// measures a genuine head-start, not just "stage-1 never finishes".
#[test]
fn stage2_confirms_before_stage1_when_stragglers_are_merely_slow() {
    let (_dir, mut chain, _db) = setup();
    let genesis = chain.proposer_genesis;
    let mut nonce = 1_000u64;
    const STRAGGLER_ROUND: u64 = 30;

    let p1 = proposer_block(1, genesis, 0);
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);

    for c in 0..VOTING_CHAINS {
        nonce += 1;
        let genesis_c = chain.voter_genesis[c as usize];
        chain.insert_block(voter_block(nonce, c, genesis_c, vec![p1_hash], 0));
    }

    let stage2_params =
        confirmation::ConfirmationParams { stage2_enabled: true, ..Default::default() };
    let mut stage1_confirmed_at: Option<u64> = None;
    let mut stage2_confirmed_at: Option<u64> = None;
    let mut stragglers_voted = false;

    for round in 1..=ROUNDS {
        let ts = round * ROUND_MS;
        for c in 0..VOTING_CHAINS {
            let tip = chain.voter_tips[c as usize].0;
            nonce += 1;
            chain.insert_block(voter_block(nonce, c, tip, vec![], ts));
        }
        if round == STRAGGLER_ROUND && !stragglers_voted {
            for c in VOTING_CHAINS..M {
                nonce += 1;
                let genesis_c = chain.voter_genesis[c as usize];
                chain.insert_block(voter_block(nonce, c, genesis_c, vec![p1_hash], ts));
            }
            stragglers_voted = true;
        } else if stragglers_voted {
            // Keep extending the stragglers' chains too, once they've voted --
            // otherwise their vote can never reach k-deep and stage-1 would
            // never confirm regardless of the confirmation math (a bug in the
            // simulation, not a protocol property).
            for c in VOTING_CHAINS..M {
                let tip = chain.voter_tips[c as usize].0;
                nonce += 1;
                chain.insert_block(voter_block(nonce, c, tip, vec![], ts));
            }
        }

        if stage1_confirmed_at.is_none()
            && confirmed_leader_at_level(&chain, 1, K_DEEP_CONFIRMATION).is_some()
        {
            stage1_confirmed_at = Some(round);
        }
        if stage2_confirmed_at.is_none()
            && confirmation::confirm_unique_leader(&chain, 1, &stage2_params).is_some()
        {
            stage2_confirmed_at = Some(round);
        }
        if stage1_confirmed_at.is_some() && stage2_confirmed_at.is_some() {
            break;
        }
    }

    let s1 = stage1_confirmed_at.expect("stage-1 must eventually confirm once stragglers vote + go k-deep");
    let s2 = stage2_confirmed_at.expect("stage-2 must confirm the majority well before that");
    println!(
        "B-4 measurement (slow stragglers at round {STRAGGLER_ROUND}): stage-2 confirmed at \
         round {s2} ({} ms); stage-1 confirmed at round {s1} ({} ms); stage-2 head start = {} rounds ({} ms)",
        s2 * ROUND_MS,
        s1 * ROUND_MS,
        s1 - s2,
        (s1 - s2) * ROUND_MS,
    );
    assert!(s2 < s1, "stage-2 (round {s2}) should confirm strictly before stage-1 (round {s1})");
}
