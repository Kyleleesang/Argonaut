//! D2 tests for `pallet-storage-proofs`: the challenge game with REAL bao slices
//! (D.12: no proof mocking) and the beacon snapshot.

use crate::{mock::*, Error, Event, ProofPayload};
use argonaut_primitives_storage::{challenged_chunk, interfaces::ProofContext};
use bao_tree::{
    io::outboard::PreOrderMemOutboard, io::sync::encode_ranges_validated, BlockSize, ChunkNum,
    ChunkRanges,
};
use frame_support::{assert_noop, assert_ok, traits::OnInitialize, BoundedVec};
use sp_core::{ConstU32, H256};

const PROVIDER: u64 = 1;
const CLIENT: u64 = 2;
const REPORTER: u64 = 3;
const DEAL: u64 = 7;
const SHARD: u16 = 0;
const SHARD_SIZE: u64 = 4096;

fn shard_blob() -> Vec<u8> {
    let mut out = Vec::with_capacity(SHARD_SIZE as usize);
    let mut x = 0xDEAD_BEEFu64;
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

fn beacon(tag: u8) -> [u8; 32] {
    let mut b = [0u8; 32];
    b[0] = tag;
    b[1] = 0xCD;
    b
}

fn ctx(rate_ppm: u32) -> ProofContext<u64> {
    ProofContext {
        provider: PROVIDER,
        client: CLIENT,
        shard_root: bao_root(&shard_blob()),
        shard_size: SHARD_SIZE,
        challenge_rate_ppm: rate_ppm,
        end_epoch: 1_000,
    }
}

fn proof_for(b: &[u8; 32]) -> ProofPayload {
    let chunk = challenged_chunk(b, DEAL, SHARD, SHARD_SIZE);
    ProofPayload::Single(bao_slice(&shard_blob(), chunk))
}

#[test]
fn submit_proof_accepts_valid() {
    new_test_ext().execute_with(|| {
        set_ctx(ctx(1_000_000)); // always selected
        go_to_epoch(1);
        let b = beacon(1);
        crate::Beacons::<Test>::insert(1, b);
        assert_ok!(StorageProofs::submit_proof(
            RuntimeOrigin::signed(PROVIDER),
            DEAL,
            SHARD,
            1,
            proof_for(&b)
        ));
        assert!(crate::ProofRecord::<Test>::contains_key((DEAL, SHARD), 1));
        System::assert_has_event(
            Event::ProofAccepted { deal_id: DEAL, shard_idx: SHARD, epoch: 1 }.into(),
        );
    });
}

#[test]
fn submit_proof_rejects_not_selected() {
    new_test_ext().execute_with(|| {
        set_ctx(ctx(0)); // never selected
        go_to_epoch(1);
        let b = beacon(1);
        crate::Beacons::<Test>::insert(1, b);
        assert_noop!(
            StorageProofs::submit_proof(RuntimeOrigin::signed(PROVIDER), DEAL, SHARD, 1, proof_for(&b)),
            Error::<Test>::NotSelected
        );
    });
}

#[test]
fn submit_proof_rejects_bad_proof() {
    new_test_ext().execute_with(|| {
        set_ctx(ctx(1_000_000));
        go_to_epoch(1);
        let b = beacon(1);
        crate::Beacons::<Test>::insert(1, b);
        // A slice for all-zero data won't verify against the real shard root.
        let chunk = challenged_chunk(&b, DEAL, SHARD, SHARD_SIZE);
        let wrong = ProofPayload::Single(bao_slice(&vec![0u8; SHARD_SIZE as usize], chunk));
        assert_noop!(
            StorageProofs::submit_proof(RuntimeOrigin::signed(PROVIDER), DEAL, SHARD, 1, wrong),
            Error::<Test>::BadProof
        );
    });
}

#[test]
fn submit_proof_rejects_already_proven() {
    new_test_ext().execute_with(|| {
        set_ctx(ctx(1_000_000));
        go_to_epoch(1);
        let b = beacon(1);
        crate::Beacons::<Test>::insert(1, b);
        assert_ok!(StorageProofs::submit_proof(
            RuntimeOrigin::signed(PROVIDER),
            DEAL,
            SHARD,
            1,
            proof_for(&b)
        ));
        assert_noop!(
            StorageProofs::submit_proof(RuntimeOrigin::signed(PROVIDER), DEAL, SHARD, 1, proof_for(&b)),
            Error::<Test>::AlreadyProven
        );
    });
}

#[test]
fn submit_proof_rejects_epoch_out_of_window() {
    new_test_ext().execute_with(|| {
        set_ctx(ctx(1_000_000));
        go_to_epoch(5); // current = 5
        let b = beacon(1);
        crate::Beacons::<Test>::insert(1, b);
        assert_noop!(
            StorageProofs::submit_proof(RuntimeOrigin::signed(PROVIDER), DEAL, SHARD, 1, proof_for(&b)),
            Error::<Test>::EpochOutOfWindow
        );
    });
}

#[test]
fn submit_proof_rejects_aggregated_payload() {
    new_test_ext().execute_with(|| {
        set_ctx(ctx(1_000_000));
        go_to_epoch(1);
        crate::Beacons::<Test>::insert(1, beacon(1));
        assert_noop!(
            StorageProofs::submit_proof(
                RuntimeOrigin::signed(PROVIDER),
                DEAL,
                SHARD,
                1,
                ProofPayload::Aggregated(H256::zero())
            ),
            Error::<Test>::Unsupported
        );
    });
}

#[test]
fn report_missed_slashes_50_50() {
    new_test_ext().execute_with(|| {
        set_ctx(ctx(1_000_000));
        let e = 2u32;
        crate::Beacons::<Test>::insert(e, beacon(e as u8));
        go_to_epoch(4); // current 4 >= e+2 = 4
        assert_ok!(StorageProofs::report_missed(RuntimeOrigin::signed(REPORTER), DEAL, SHARD, e));

        assert_eq!(Balances::free_balance(&REPORTER), 50);
        assert_eq!(Balances::free_balance(&CLIENT), 50);
        assert_eq!(StorageProofs::misses((DEAL, SHARD)), 1);
        assert_eq!(slashes().len(), 2);
        System::assert_has_event(
            Event::MissReported { deal_id: DEAL, shard_idx: SHARD, epoch: e }.into(),
        );
    });
}

#[test]
fn report_missed_before_deadline_fails() {
    new_test_ext().execute_with(|| {
        set_ctx(ctx(1_000_000));
        let e = 3u32;
        crate::Beacons::<Test>::insert(e, beacon(e as u8));
        go_to_epoch(4); // current 4 < e+2 = 5
        assert_noop!(
            StorageProofs::report_missed(RuntimeOrigin::signed(REPORTER), DEAL, SHARD, e),
            Error::<Test>::TooEarly
        );
    });
}

#[test]
fn report_missed_escalates_after_max_misses() {
    new_test_ext().execute_with(|| {
        set_ctx(ctx(1_000_000));
        for e in 2u32..=4 {
            crate::Beacons::<Test>::insert(e, beacon(e as u8));
        }
        go_to_epoch(6); // reportable epochs: 2,3,4
        assert_ok!(StorageProofs::report_missed(RuntimeOrigin::signed(REPORTER), DEAL, SHARD, 2));
        assert_ok!(StorageProofs::report_missed(RuntimeOrigin::signed(REPORTER), DEAL, SHARD, 3));
        assert_ok!(StorageProofs::report_missed(RuntimeOrigin::signed(REPORTER), DEAL, SHARD, 4));

        let f = faults();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0], (DEAL, SHARD, PROVIDER, 1_000)); // FaultSlash bounty
        System::assert_has_event(
            Event::ShardFaultEscalated { deal_id: DEAL, shard_idx: SHARD }.into(),
        );
        // ctx cleared by the fault callback → further reports see no assignment.
        assert_noop!(
            StorageProofs::report_missed(RuntimeOrigin::signed(REPORTER), DEAL, SHARD, 2),
            Error::<Test>::NotActivated
        );
    });
}

#[test]
fn on_initialize_snapshots_beacon() {
    new_test_ext().execute_with(|| {
        // At block 20 (epoch 2), beacon = block_hash(20 - BeaconDelay(2)) = hash(18).
        frame_system::BlockHash::<Test>::insert(18u64, H256::from([9u8; 32]));
        StorageProofs::on_initialize(20u64);
        assert_eq!(StorageProofs::beacon_of(2), Some([9u8; 32]));
    });
}

#[test]
fn on_deal_closed_prunes_records() {
    use argonaut_primitives_storage::interfaces::ProofPruner;
    new_test_ext().execute_with(|| {
        crate::ProofRecord::<Test>::insert((DEAL, SHARD), 1u32, ());
        crate::MissRecord::<Test>::insert((DEAL, SHARD), 2u32, ());
        crate::Misses::<Test>::insert((DEAL, SHARD), 2u8);
        <StorageProofs as ProofPruner>::on_deal_closed(DEAL, 1);
        assert!(!crate::ProofRecord::<Test>::contains_key((DEAL, SHARD), 1));
        assert!(!crate::MissRecord::<Test>::contains_key((DEAL, SHARD), 2));
        assert_eq!(StorageProofs::misses((DEAL, SHARD)), 0);
    });
}
