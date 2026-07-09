//! Benchmarks for `pallet-payment-channels` (placeholder weights, spec D.1 rule 8).
//! `settle` needs a real Falcon voucher (covered by unit tests); `open`/`close`
//! are benchmarked here.
use super::*;
use frame_benchmarking::v2::*;
use frame_support::traits::{Currency, Get};
use frame_system::RawOrigin;
use sp_runtime::traits::{Saturating, Zero};

#[benchmarks]
mod benchmarks {
    use super::*;

    #[benchmark]
    fn open() {
        let caller: T::AccountId = whitelisted_caller();
        let to: T::AccountId = account("provider", 0, 0);
        let dep = T::MinDeposit::get();
        T::Currency::make_free_balance_be(&caller, dep.saturating_add(dep));

        #[extrinsic_call]
        open(RawOrigin::Signed(caller.clone()), to, dep, Zero::zero());

        assert!(Channels::<T>::contains_key(0));
    }

    #[benchmark]
    fn close() {
        let caller: T::AccountId = whitelisted_caller();
        let to: T::AccountId = account("provider", 0, 0);
        let dep = T::MinDeposit::get();
        T::Currency::make_free_balance_be(&caller, dep.saturating_add(dep));
        Pallet::<T>::open(RawOrigin::Signed(caller.clone()).into(), to.clone(), dep, Zero::zero())
            .unwrap();

        // `to` may close any time.
        #[extrinsic_call]
        close(RawOrigin::Signed(to.clone()), 0);

        assert!(!Channels::<T>::contains_key(0));
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
