//! Benchmarks for `pallet-storage-deals` (placeholder weights, spec D.1 rule 8).
//! Only `create_deal` is benchmarked as a stub; the full lifecycle needs real
//! providers and bao slices and is covered by the unit tests.
use super::*;
use argonaut_primitives_storage::{DealParams, ShardSpec};
use frame_benchmarking::v2::*;
use frame_support::{traits::Currency, BoundedVec};
use frame_system::RawOrigin;
use sp_std::vec;

#[benchmarks]
mod benchmarks {
    use super::*;

    #[benchmark]
    fn create_deal() {
        let caller: T::AccountId = whitelisted_caller();
        T::Currency::make_free_balance_be(&caller, 1_000_000u32.into());
        let shards = BoundedVec::try_from(vec![
            ShardSpec { root: [0u8; 32], size: 1024 },
            ShardSpec { root: [1u8; 32], size: 1024 },
        ])
        .unwrap();
        let params = DealParams {
            manifest: [0u8; 32],
            shards,
            k: 1,
            n: 2,
            duration_epochs: 1,
            price_per_shard_epoch: 1u32.into(),
            challenge_rate_ppm: 0,
        };

        #[extrinsic_call]
        create_deal(RawOrigin::Signed(caller.clone()), params);

        assert!(Deals::<T>::contains_key(0));
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
