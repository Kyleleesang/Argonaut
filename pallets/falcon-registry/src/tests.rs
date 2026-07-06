use crate::{mock::*, Error, Event, KeyProvider};
use argonaut_primitives_falcon::{falcon_crypto, FalconPublic};
use frame_support::{assert_noop, assert_ok, traits::Currency, BoundedVec};
use sp_core::crypto::AccountId32;

/// A structurally-valid-length Falcon public key filled with `byte`. `register_key`
/// never checks Falcon key validity itself (only that its hash matches the caller's
/// claimed AccountId), so an arbitrary fill is fine for pallet-level tests.
fn falcon_pk(byte: u8) -> FalconPublic {
    FalconPublic(BoundedVec::try_from(vec![byte; 897]).unwrap())
}

fn account_for(pk: &FalconPublic) -> AccountId32 {
    AccountId32::from(falcon_crypto::pk_hash(&pk.0))
}

#[test]
fn register_key_succeeds_with_matching_pk() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        let pk = falcon_pk(1);
        let who = account_for(&pk);
        Balances::make_free_balance_be(&who, 1_000);

        assert_ok!(FalconRegistry::register_key(RuntimeOrigin::signed(who.clone()), pk.clone()));

        assert_eq!(FalconRegistry::keys(&who), Some(pk));
        System::assert_last_event(Event::KeyRegistered { who }.into());
    });
}

#[test]
fn register_key_reserves_the_deposit() {
    new_test_ext().execute_with(|| {
        let pk = falcon_pk(2);
        let who = account_for(&pk);
        Balances::make_free_balance_be(&who, 1_000);

        assert_ok!(FalconRegistry::register_key(RuntimeOrigin::signed(who.clone()), pk));

        assert_eq!(Balances::reserved_balance(&who), 100);
        assert_eq!(Balances::free_balance(&who), 900);
    });
}

#[test]
fn register_key_fails_with_mismatched_pk() {
    new_test_ext().execute_with(|| {
        let pk = falcon_pk(3);
        let wrong_account = account_for(&falcon_pk(4));
        Balances::make_free_balance_be(&wrong_account, 1_000);

        assert_noop!(
            FalconRegistry::register_key(RuntimeOrigin::signed(wrong_account), pk),
            Error::<Test>::KeyMismatch
        );
    });
}

#[test]
fn register_key_fails_if_already_registered() {
    new_test_ext().execute_with(|| {
        let pk = falcon_pk(5);
        let who = account_for(&pk);
        Balances::make_free_balance_be(&who, 1_000);

        assert_ok!(FalconRegistry::register_key(RuntimeOrigin::signed(who.clone()), pk.clone()));
        assert_noop!(
            FalconRegistry::register_key(RuntimeOrigin::signed(who), pk),
            Error::<Test>::AlreadyRegistered
        );
    });
}

#[test]
fn register_key_fails_with_insufficient_balance() {
    new_test_ext().execute_with(|| {
        let pk = falcon_pk(6);
        let who = account_for(&pk);
        // No genesis balance and nothing minted: can't cover the KeyDeposit.

        assert_noop!(
            FalconRegistry::register_key(RuntimeOrigin::signed(who), pk),
            Error::<Test>::InsufficientBalance
        );
    });
}

#[test]
fn deregister_key_removes_entry_and_refunds() {
    new_test_ext().execute_with(|| {
        System::set_block_number(1);
        let pk = falcon_pk(7);
        let who = account_for(&pk);
        Balances::make_free_balance_be(&who, 1_000);
        assert_ok!(FalconRegistry::register_key(RuntimeOrigin::signed(who.clone()), pk));

        assert_ok!(FalconRegistry::deregister_key(RuntimeOrigin::signed(who.clone())));

        assert_eq!(FalconRegistry::keys(&who), None);
        assert_eq!(Balances::reserved_balance(&who), 0);
        assert_eq!(Balances::free_balance(&who), 1_000);
        System::assert_last_event(Event::KeyDeregistered { who }.into());
    });
}

#[test]
fn deregister_key_fails_if_not_registered() {
    new_test_ext().execute_with(|| {
        let who = account_for(&falcon_pk(8));

        assert_noop!(
            FalconRegistry::deregister_key(RuntimeOrigin::signed(who)),
            Error::<Test>::NotRegistered
        );
    });
}

#[test]
fn key_provider_resolves_registered_key() {
    new_test_ext().execute_with(|| {
        let pk = falcon_pk(9);
        let who = account_for(&pk);
        Balances::make_free_balance_be(&who, 1_000);
        assert_ok!(FalconRegistry::register_key(RuntimeOrigin::signed(who.clone()), pk.clone()));

        assert_eq!(<FalconRegistry as KeyProvider<AccountId32>>::key_of(&who), Some(pk));
    });
}

#[test]
fn key_provider_returns_none_for_unregistered_account() {
    new_test_ext().execute_with(|| {
        let who = account_for(&falcon_pk(10));
        assert_eq!(<FalconRegistry as KeyProvider<AccountId32>>::key_of(&who), None);
    });
}
