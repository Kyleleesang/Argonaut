//! D2 lifecycle tests for `pallet-storage-deals` using REAL bao slices (D.12: no
//! proof mocking) on tiny 4 KiB shards.

use crate::{mock::*, DealState, Error, Event, ShardState};
use argonaut_primitives_storage::{
    activation_chunk,
    interfaces::{DealFaultHandler, DealInspector},
    DealParams, ShardSpec,
};
use bao_tree::{
    io::outboard::PreOrderMemOutboard, io::sync::encode_ranges_validated, BlockSize, ChunkNum,
    ChunkRanges,
};
use frame_support::{assert_noop, assert_ok, traits::Currency, BoundedVec};
use sp_core::ConstU32;

const CLIENT: u64 = 100;
const SHARD_SIZE: u64 = 4096; // 4 chunks

// ── bao helpers (native encode side) ─────────────────────────────────────────

fn shard_blob(i: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(SHARD_SIZE as usize);
    let mut x = 0x1234_5678u64 ^ (i as u64).wrapping_mul(0x9E37_79B9);
    for _ in 0..SHARD_SIZE {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.push((x & 0xff) as u8);
    }
    out
}

fn bao_root(data: &[u8]) -> [u8; 32] {
    *PreOrderMemOutboard::create(data, BlockSize::ZERO).root.as_bytes()
}

fn bao_slice(data: &[u8], chunk: u64) -> BoundedVec<u8, ConstU32<4096>> {
    let ob = PreOrderMemOutboard::create(data, BlockSize::ZERO);
    let ranges = ChunkRanges::from(ChunkNum(chunk)..ChunkNum(chunk + 1));
    let mut slice = Vec::new();
    encode_ranges_validated(data, &ob, ranges.as_ref(), &mut slice).unwrap();
    BoundedVec::try_from(slice).unwrap()
}

// ── deal helpers ─────────────────────────────────────────────────────────────

fn make_params(k: u8, n: u8, price: u128, duration: u32) -> DealParams<u128> {
    let shards: Vec<ShardSpec> =
        (0..n).map(|i| ShardSpec { root: bao_root(&shard_blob(i)), size: SHARD_SIZE }).collect();
    DealParams {
        manifest: [0u8; 32],
        shards: BoundedVec::try_from(shards).unwrap(),
        k,
        n,
        duration_epochs: duration,
        price_per_shard_epoch: price,
        challenge_rate_ppm: 500_000,
    }
}

fn register_provider(who: u64, stake: u128) {
    Balances::make_free_balance_be(&who, stake + 10_000);
    assert_ok!(ProviderRegistry::register(
        RuntimeOrigin::signed(who),
        stake,
        [who as u8; 32],
        BoundedVec::default(),
        1,
    ));
}

/// Activate shard `idx` held by `provider`, accepted at `accepted_epoch`, using a
/// real bao slice for its activation chunk.
fn activate(provider: u64, deal_id: u64, idx: u16, accepted_epoch: u32) {
    let beacon = mock_beacon(accepted_epoch + 1);
    let chunk = activation_chunk(&beacon, deal_id, idx, SHARD_SIZE);
    let slice = bao_slice(&shard_blob(idx as u8), chunk);
    assert_ok!(StorageDeals::activate_shard(RuntimeOrigin::signed(provider), deal_id, idx, slice));
}

// Re-expose the mock beacon (the pallet uses the same `MockEpochs::beacon`).
fn mock_beacon(epoch: u32) -> [u8; 32] {
    use argonaut_primitives_storage::interfaces::EpochProvider;
    MockEpochs::beacon(epoch).expect("beacon available for epoch >= 1")
}

// ── tests ────────────────────────────────────────────────────────────────────

#[test]
fn create_deal_reserves_escrow() {
    new_test_ext().execute_with(|| {
        Balances::make_free_balance_be(&CLIENT, 10_000);
        let params = make_params(2, 3, 100, 5); // per = 500, total = 1500
        assert_ok!(StorageDeals::create_deal(RuntimeOrigin::signed(CLIENT), params));
        assert_eq!(Balances::reserved_balance(CLIENT), 1_500);
        let deal = StorageDeals::deals(0).unwrap();
        assert_eq!(deal.state, DealState::Listed);
        assert_eq!(deal.escrow_remaining, 1_500);
        System::assert_has_event(Event::DealCreated { deal_id: 0, client: CLIENT }.into());
    });
}

#[test]
fn create_deal_rejects_bad_params() {
    new_test_ext().execute_with(|| {
        Balances::make_free_balance_be(&CLIENT, 10_000);
        // k == n is invalid.
        let mut params = make_params(3, 3, 100, 5);
        params.k = 3;
        assert_noop!(
            StorageDeals::create_deal(RuntimeOrigin::signed(CLIENT), params),
            Error::<Test>::BadParams
        );
    });
}

#[test]
fn full_lifecycle_activate_claim_close() {
    new_test_ext().execute_with(|| {
        Balances::make_free_balance_be(&CLIENT, 100_000);
        for p in 1..=3u64 {
            register_provider(p, 1_000);
        }
        let params = make_params(2, 3, 100, 5); // per=500, total=1500, end=start+5
        assert_ok!(StorageDeals::create_deal(RuntimeOrigin::signed(CLIENT), params));

        // Epoch 0: accept all three shards.
        for (p, idx) in [(1u64, 0u16), (2, 1), (3, 2)] {
            assert_ok!(StorageDeals::accept_shard(RuntimeOrigin::signed(p), 0, idx));
        }
        assert_eq!(StorageDeals::deals(0).unwrap().state, DealState::Assigning);

        // Epoch 1: activate all three → Active.
        go_to_epoch(1);
        activate(1, 0, 0, 0);
        activate(2, 0, 1, 0);
        assert_eq!(StorageDeals::deals(0).unwrap().state, DealState::Assigning);
        activate(3, 0, 2, 0);
        let deal = StorageDeals::deals(0).unwrap();
        assert_eq!(deal.state, DealState::Active);
        assert_eq!(deal.start_epoch, 1);
        assert_eq!(deal.end_epoch, 6);
        System::assert_has_event(Event::DealActive { deal_id: 0 }.into());

        // Epoch 4: provider 1 claims epochs (1,3] = 2 epochs * 100 = 200.
        go_to_epoch(4);
        let before = Balances::free_balance(1);
        assert_ok!(StorageDeals::claim_earnings(RuntimeOrigin::signed(1), 0, 0));
        assert_eq!(Balances::free_balance(1), before + 200);
        let a = StorageDeals::assignment(0, 0).unwrap();
        assert_eq!(a.claimed_through, 3);
        assert_eq!(a.state, ShardState::Activated);

        // Claiming again immediately yields nothing.
        assert_noop!(
            StorageDeals::claim_earnings(RuntimeOrigin::signed(1), 0, 0),
            Error::<Test>::NothingToClaim
        );

        // Close after the grace window (end 6 + grace 8 = 14 → epoch 15).
        go_to_epoch(15);
        assert_ok!(StorageDeals::close_deal(RuntimeOrigin::signed(CLIENT), 0));
        assert_eq!(StorageDeals::deals(0).unwrap().state, DealState::Expired);
        // Escrow fully released (client got its unspent escrow back).
        assert_eq!(Balances::reserved_balance(CLIENT), 0);
    });
}

#[test]
fn accept_rejects_duplicate_provider() {
    new_test_ext().execute_with(|| {
        Balances::make_free_balance_be(&CLIENT, 100_000);
        register_provider(1, 5_000);
        let params = make_params(2, 3, 100, 5);
        assert_ok!(StorageDeals::create_deal(RuntimeOrigin::signed(CLIENT), params));
        assert_ok!(StorageDeals::accept_shard(RuntimeOrigin::signed(1), 0, 0));
        assert_noop!(
            StorageDeals::accept_shard(RuntimeOrigin::signed(1), 0, 1),
            Error::<Test>::AlreadyProviding
        );
    });
}

#[test]
fn activate_rejects_bad_proof() {
    new_test_ext().execute_with(|| {
        Balances::make_free_balance_be(&CLIENT, 100_000);
        register_provider(1, 1_000);
        let params = make_params(2, 3, 100, 5);
        assert_ok!(StorageDeals::create_deal(RuntimeOrigin::signed(CLIENT), params));
        assert_ok!(StorageDeals::accept_shard(RuntimeOrigin::signed(1), 0, 0));
        go_to_epoch(1);
        // A slice for the wrong shard's data won't verify against shard 0's root.
        let beacon = mock_beacon(1);
        let chunk = activation_chunk(&beacon, 0, 0, SHARD_SIZE);
        let wrong = bao_slice(&shard_blob(9), chunk);
        assert_noop!(
            StorageDeals::activate_shard(RuntimeOrigin::signed(1), 0, 0, wrong),
            Error::<Test>::BadProof
        );
    });
}

#[test]
fn activate_before_window_fails() {
    new_test_ext().execute_with(|| {
        Balances::make_free_balance_be(&CLIENT, 100_000);
        register_provider(1, 1_000);
        let params = make_params(2, 3, 100, 5);
        assert_ok!(StorageDeals::create_deal(RuntimeOrigin::signed(CLIENT), params));
        assert_ok!(StorageDeals::accept_shard(RuntimeOrigin::signed(1), 0, 0));
        // Still epoch 0 (== accepted_epoch): window not open.
        let beacon = mock_beacon(1);
        let chunk = activation_chunk(&beacon, 0, 0, SHARD_SIZE);
        let slice = bao_slice(&shard_blob(0), chunk);
        assert_noop!(
            StorageDeals::activate_shard(RuntimeOrigin::signed(1), 0, 0, slice),
            Error::<Test>::ActivationWindowNotOpen
        );
    });
}

#[test]
fn cancel_deal_refunds_and_releases() {
    new_test_ext().execute_with(|| {
        Balances::make_free_balance_be(&CLIENT, 100_000);
        register_provider(1, 1_000);
        let params = make_params(2, 3, 100, 5);
        assert_ok!(StorageDeals::create_deal(RuntimeOrigin::signed(CLIENT), params));
        assert_ok!(StorageDeals::accept_shard(RuntimeOrigin::signed(1), 0, 0));
        assert_eq!(ProviderRegistry::stake_locked(1), 500);

        assert_ok!(StorageDeals::cancel_deal(RuntimeOrigin::signed(CLIENT), 0));
        assert_eq!(StorageDeals::deals(0).unwrap().state, DealState::Cancelled);
        assert_eq!(Balances::reserved_balance(CLIENT), 0);
        assert_eq!(ProviderRegistry::stake_locked(1), 0); // assignment released
    });
}

#[test]
fn expire_assignment_reclaims_slot() {
    new_test_ext().execute_with(|| {
        Balances::make_free_balance_be(&CLIENT, 100_000);
        register_provider(1, 1_000);
        let params = make_params(2, 3, 100, 5);
        assert_ok!(StorageDeals::create_deal(RuntimeOrigin::signed(CLIENT), params));
        assert_ok!(StorageDeals::accept_shard(RuntimeOrigin::signed(1), 0, 0));

        // Activation window = accepted(0)+1+2 = 3; expire allowed once current > 3.
        go_to_epoch(4);
        assert_ok!(StorageDeals::expire_assignment(RuntimeOrigin::signed(CLIENT), 0, 0));
        assert!(StorageDeals::assignment(0, 0).is_none());
        assert_eq!(ProviderRegistry::stake_locked(1), 0);
        System::assert_has_event(
            Event::AssignmentExpired { deal_id: 0, shard_idx: 0, provider: 1 }.into(),
        );
    });
}

#[test]
fn fault_then_repair_returns_active() {
    new_test_ext().execute_with(|| {
        Balances::make_free_balance_be(&CLIENT, 100_000);
        for p in 1..=4u64 {
            register_provider(p, 2_000);
        }
        let params = make_params(2, 3, 100, 5);
        assert_ok!(StorageDeals::create_deal(RuntimeOrigin::signed(CLIENT), params));
        for (p, idx) in [(1u64, 0u16), (2, 1), (3, 2)] {
            assert_ok!(StorageDeals::accept_shard(RuntimeOrigin::signed(p), 0, idx));
        }
        go_to_epoch(1);
        activate(1, 0, 0, 0);
        activate(2, 0, 1, 0);
        activate(3, 0, 2, 0);
        assert_eq!(StorageDeals::deals(0).unwrap().state, DealState::Active);

        // Simulate a confirmed fault of shard 0 (what storage-proofs would call),
        // pre-funding the pallet account with the repair bounty.
        let bounty = 500u128;
        Balances::make_free_balance_be(&StorageDeals::account_id(), bounty + 1);
        <StorageDeals as DealFaultHandler<u64, u128>>::on_shard_faulted(0, 0, &1, bounty);
        let deal = StorageDeals::deals(0).unwrap();
        assert_eq!(deal.state, DealState::Degraded);
        assert_eq!(deal.faulted_count, 1);
        assert_eq!(deal.repair_pot, bounty);
        assert_eq!(StorageDeals::assignment(0, 0).unwrap().state, ShardState::Faulted);

        // Provider 4 repairs shard 0 (accept on the faulted slot, then activate).
        assert_ok!(StorageDeals::accept_shard(RuntimeOrigin::signed(4), 0, 0));
        let before = Balances::free_balance(4);
        // accepted at epoch 1 → activate at epoch 2.
        go_to_epoch(2);
        activate(4, 0, 0, 1);
        let deal = StorageDeals::deals(0).unwrap();
        assert_eq!(deal.state, DealState::Active);
        assert_eq!(deal.faulted_count, 0);
        assert_eq!(deal.repair_pot, 0);
        assert_eq!(Balances::free_balance(4), before + bounty); // collected the bounty
    });
}

#[test]
fn fault_to_lost_when_survivors_below_k() {
    new_test_ext().execute_with(|| {
        Balances::make_free_balance_be(&CLIENT, 100_000);
        for p in 1..=3u64 {
            register_provider(p, 2_000);
        }
        let params = make_params(2, 3, 100, 5); // k=2, n=3 → survivors<2 means Lost after 2 faults
        assert_ok!(StorageDeals::create_deal(RuntimeOrigin::signed(CLIENT), params));
        for (p, idx) in [(1u64, 0u16), (2, 1), (3, 2)] {
            assert_ok!(StorageDeals::accept_shard(RuntimeOrigin::signed(p), 0, idx));
        }
        go_to_epoch(1);
        activate(1, 0, 0, 0);
        activate(2, 0, 1, 0);
        activate(3, 0, 2, 0);

        <StorageDeals as DealFaultHandler<u64, u128>>::on_shard_faulted(0, 0, &1, 0);
        assert_eq!(StorageDeals::deals(0).unwrap().state, DealState::Degraded);
        <StorageDeals as DealFaultHandler<u64, u128>>::on_shard_faulted(0, 1, &2, 0);
        assert_eq!(StorageDeals::deals(0).unwrap().state, DealState::Lost);
        System::assert_has_event(Event::DealLost { deal_id: 0 }.into());
    });
}

#[test]
fn proof_context_reflects_state() {
    new_test_ext().execute_with(|| {
        Balances::make_free_balance_be(&CLIENT, 100_000);
        register_provider(1, 5_000);
        let params = make_params(2, 3, 100, 5);
        assert_ok!(StorageDeals::create_deal(RuntimeOrigin::signed(CLIENT), params));
        assert_ok!(StorageDeals::accept_shard(RuntimeOrigin::signed(1), 0, 0));
        // Not yet activated / deal not Active → no context.
        assert!(<StorageDeals as DealInspector<u64>>::proof_context(0, 0).is_none());
    });
}
