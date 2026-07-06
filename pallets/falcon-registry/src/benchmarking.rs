//! Benchmarks for `pallet-falcon-registry` (architecture guide §2.3).
use super::*;
use argonaut_primitives_falcon::falcon_crypto;
use frame_benchmarking::v2::*;
use frame_support::traits::{Currency, Get};
use frame_system::RawOrigin;
use sp_core::{bounded::BoundedVec, ConstU32};
use sp_std::vec;

/// A structurally-valid-length Falcon public key filled with `byte`. Benchmarks don't
/// need a cryptographically real key: `register_key` only checks `blake3(pk)` against
/// the caller's claimed AccountId, never the key's Falcon validity.
fn falcon_pk(byte: u8) -> FalconPublic {
    FalconPublic(BoundedVec::<u8, ConstU32<897>>::try_from(vec![byte; 897]).unwrap())
}

#[benchmarks(where T::AccountId: From<[u8; 32]>)]
mod benchmarks {
    use super::*;

    #[benchmark]
    fn register_key() {
        let pk = falcon_pk(1);
        let caller: T::AccountId = falcon_crypto::pk_hash(&pk.0).into();
        T::Currency::make_free_balance_be(&caller, T::KeyDeposit::get() * 2u32.into());

        #[extrinsic_call]
        register_key(RawOrigin::Signed(caller.clone()), pk.clone());

        assert_eq!(Keys::<T>::get(&caller), Some(pk));
    }

    #[benchmark]
    fn deregister_key() {
        let pk = falcon_pk(2);
        let caller: T::AccountId = falcon_crypto::pk_hash(&pk.0).into();
        T::Currency::make_free_balance_be(&caller, T::KeyDeposit::get() * 2u32.into());
        Pallet::<T>::register_key(RawOrigin::Signed(caller.clone()).into(), pk).unwrap();

        #[extrinsic_call]
        deregister_key(RawOrigin::Signed(caller.clone()));

        assert_eq!(Keys::<T>::get(&caller), None);
    }

    impl_benchmark_test_suite!(
        Pallet,
        crate::mock::new_test_ext(),
        crate::mock::Test,
    );
}
