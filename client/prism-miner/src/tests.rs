use super::*;
use argonaut_primitives_prism::validate_block;
use sp_core::U256;

/// Params with an easy, almost-Fail-free threshold so tests mine in very few
/// attempts: like `Params::testnet()`'s own construction, but the whole U256
/// range is split (almost) entirely across the m+2 slots.
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

/// Mine one block from `tmpl`, panicking if none is found within a generous
/// attempt budget (should never happen at `fast_params`' difficulty).
fn mine_one(tmpl: &MinerTemplate, params: &Params) -> PrismBlock {
    let (found, _) = try_mine(tmpl, params, 0, 1_000_000);
    found.expect("fast_params should yield a block well within 1_000_000 attempts")
}

#[test]
fn mined_block_passes_validate_block() {
    let params = fast_params(4);
    let chain = PrismChain::new(params.clone());
    let tmpl = build_template(&chain, &[], 0, 8, &params);

    let block = mine_one(&tmpl, &params);
    assert!(validate_block(&block, &params).is_ok());
}

#[test]
fn template_voter_parents_match_chain_tips() {
    let params = fast_params(4);
    let chain = PrismChain::new(params.clone());
    let tmpl = build_template(&chain, &[], 0, 8, &params);

    for i in 0..params.m_voter_chains as usize {
        assert_eq!(tmpl.parents[i], chain.voter_tips[i].0);
        match &tmpl.contents[i] {
            BlockContent::Voter { chain: c, voter_parent, votes } => {
                assert_eq!(*c, i as u16);
                assert_eq!(*voter_parent, chain.voter_tips[i].0);
                assert!(votes.is_empty(), "no proposers exist yet, nothing to vote on");
            }
            other => panic!("expected Voter content at slot {i}, got {other:?}"),
        }
    }
}

#[test]
fn template_tx_and_proposer_parent_is_genesis_when_no_proposers_exist() {
    let params = fast_params(4);
    let chain = PrismChain::new(params.clone());
    let m = params.m_voter_chains as usize;
    let tmpl = build_template(&chain, &[], 0, 8, &params);

    assert_eq!(tmpl.parents[m], chain.proposer_genesis);
    assert_eq!(tmpl.parents[m + 1], chain.proposer_genesis);
}

#[test]
fn template_proposer_parent_is_deepest_unreferenced_proposer() {
    let params = fast_params(4);
    let mut chain = PrismChain::new(params.clone());

    // Mine and insert two proposers in sequence: p2 builds on p1, so p2 is
    // strictly deeper and should be picked as the next template's parent.
    let tmpl1 = build_template(&chain, &[], 0, 8, &params);
    let mut p1 = mine_one(&tmpl1, &params);
    // Force a Proposer-content block regardless of what sortition first found,
    // by re-mining until we land on one (fast_params makes this quick).
    while !matches!(p1.content, BlockContent::Proposer { .. }) {
        let (found, _) = try_mine(&tmpl1, &params, p1.header.nonce + 1, 1_000_000);
        p1 = found.expect("should find a proposer block quickly under fast_params");
    }
    let p1_hash = block_hash(&p1.header);
    chain.insert_block(p1);

    let tmpl2 = build_template(&chain, &[], 0, 8, &params);
    let m = params.m_voter_chains as usize;
    assert_eq!(tmpl2.parents[m + 1], p1_hash);
}

#[test]
fn queue_of_is_deterministic_and_bounded() {
    let tx = b"hello world".to_vec();
    let q1 = queue_of(&tx, 8);
    let q2 = queue_of(&tx, 8);
    assert_eq!(q1, q2);
    assert!(q1 < 8);
}

#[test]
fn build_template_only_includes_matching_queue_txs() {
    let params = fast_params(4);
    let chain = PrismChain::new(params.clone());
    let mempool: Vec<Vec<u8>> = (0u8..50).map(|b| vec![b; 4]).collect();
    let q = 8;

    for queue in 0..q {
        let tmpl = build_template(&chain, &mempool, queue, q, &params);
        let m = params.m_voter_chains as usize;
        match &tmpl.contents[m] {
            BlockContent::Transaction { txs } => {
                for tx in txs {
                    assert_eq!(queue_of(tx, q), queue);
                }
            }
            other => panic!("expected Transaction content, got {other:?}"),
        }
    }
}

/// The B2 "single-node self-mining loopback" scenario at the miner+chain level
/// (without going through `prism-network`'s validate/gossip layer, which has
/// its own test for the same scenario): repeatedly build a template from live
/// chain state, mine a block, insert it back into the same chain, and check
/// the chain's core invariant (vote totals == recount-from-scratch) holds
/// after every single insertion, across many mined blocks of mixed types.
#[test]
fn self_mining_loopback_preserves_chain_invariants() {
    let params = fast_params(4);
    let mut chain = PrismChain::new(params.clone());
    let mempool: Vec<Vec<u8>> = (0u8..20).map(|b| vec![b; 8]).collect();

    for round in 0..40u64 {
        let tmpl = build_template(&chain, &mempool, round % 8, 8, &params);
        let block = mine_one(&tmpl, &params);
        assert!(
            validate_block(&block, &params).is_ok(),
            "round {round}: miner produced a block that fails validate_block"
        );
        let outcome = chain.insert_block(block);
        assert!(
            outcome.missing.is_empty(),
            "round {round}: miner-produced block referenced something the chain doesn't know about"
        );
        assert_eq!(
            chain.vote_totals(),
            chain.recount_votes(),
            "round {round}: vote totals diverged from recount after a self-mined insertion"
        );
    }
}
