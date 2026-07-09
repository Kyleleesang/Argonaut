//! Benchmarks for `pallet-provider-registry` (placeholder weights, spec D.1 rule 8).
use super::*;
use frame_benchmarking::v2::*;
use frame_support::traits::{Currency, Get};
use frame_system::RawOrigin;
use sp_runtime::traits::Saturating;

#[benchmarks]
mod benchmarks {
    use super::*;

    #[benchmark]
    fn register() {
        let caller: T::AccountId = whitelisted_caller();
        let stake = T::MinStake::get();
        T::Currency::make_free_balance_be(&caller, stake.saturating_add(stake));

        #[extrinsic_call]
        register(RawOrigin::Signed(caller.clone()), stake, [0u8; 32], Default::default(), stake);

        assert!(Providers::<T>::contains_key(&caller));
    }

    #[benchmark]
    fn deregister() {
        let caller: T::AccountId = whitelisted_caller();
        let stake = T::MinStake::get();
        T::Currency::make_free_balance_be(&caller, stake.saturating_add(stake));
        Pallet::<T>::register(
            RawOrigin::Signed(caller.clone()).into(),
            stake,
            [0u8; 32],
            Default::default(),
            stake,
        )
        .unwrap();

        #[extrinsic_call]
        deregister(RawOrigin::Signed(caller.clone()));

        assert!(!Providers::<T>::contains_key(&caller));
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
