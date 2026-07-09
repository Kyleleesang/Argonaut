use crate::{mock::*, Error, Event, ProviderStatus};
use argonaut_primitives_storage::interfaces::ProviderInterface;
use frame_support::{assert_noop, assert_ok, traits::Currency, BoundedVec};

const PROVIDER: u64 = 1;
const BENEFICIARY: u64 = 2;

fn hint() -> BoundedVec<u8, frame_support::traits::ConstU32<256>> {
    BoundedVec::try_from(b"relay".to_vec()).unwrap()
}

fn register(who: u64, stake: u128) {
    Balances::make_free_balance_be(&who, stake + 1_000);
    assert_ok!(ProviderRegistry::register(
        RuntimeOrigin::signed(who),
        stake,
        [7u8; 32],
        hint(),
        5,
    ));
}

#[test]
fn register_reserves_and_stores() {
    new_test_ext().execute_with(|| {
        register(PROVIDER, 1_000);
        let p = ProviderRegistry::providers(PROVIDER).unwrap();
        assert_eq!(p.stake, 1_000);
        assert_eq!(p.status, ProviderStatus::Active);
        assert_eq!(Balances::reserved_balance(PROVIDER), 1_000);
        System::assert_last_event(Event::Registered { who: PROVIDER }.into());
    });
}

#[test]
fn register_rejects_low_stake() {
    new_test_ext().execute_with(|| {
        Balances::make_free_balance_be(&PROVIDER, 10_000);
        assert_noop!(
            ProviderRegistry::register(RuntimeOrigin::signed(PROVIDER), 999, [0u8; 32], hint(), 1),
            Error::<Test>::StakeTooLow
        );
    });
}

#[test]
fn register_rejects_duplicate() {
    new_test_ext().execute_with(|| {
        register(PROVIDER, 1_000);
        assert_noop!(
            ProviderRegistry::register(RuntimeOrigin::signed(PROVIDER), 1_000, [0u8; 32], hint(), 1),
            Error::<Test>::AlreadyRegistered
        );
    });
}

#[test]
fn top_up_increases_stake() {
    new_test_ext().execute_with(|| {
        register(PROVIDER, 1_000);
        assert_ok!(ProviderRegistry::top_up_stake(RuntimeOrigin::signed(PROVIDER), 500));
        assert_eq!(ProviderRegistry::providers(PROVIDER).unwrap().stake, 1_500);
        assert_eq!(Balances::reserved_balance(PROVIDER), 1_500);
    });
}

#[test]
fn deregister_refunds_when_idle() {
    new_test_ext().execute_with(|| {
        register(PROVIDER, 1_000);
        assert_ok!(ProviderRegistry::deregister(RuntimeOrigin::signed(PROVIDER)));
        assert!(ProviderRegistry::providers(PROVIDER).is_none());
        assert_eq!(Balances::reserved_balance(PROVIDER), 0);
    });
}

#[test]
fn deregister_blocked_with_active_assignment() {
    new_test_ext().execute_with(|| {
        register(PROVIDER, 1_000);
        assert_ok!(<ProviderRegistry as ProviderInterface<u64, u128>>::note_assignment_started(
            &PROVIDER, 100
        ));
        assert_noop!(
            ProviderRegistry::deregister(RuntimeOrigin::signed(PROVIDER)),
            Error::<Test>::HasActiveAssignments
        );
    });
}

#[test]
fn note_assignment_started_enforces_2x_rule() {
    new_test_ext().execute_with(|| {
        register(PROVIDER, 1_000);
        // 2 · 400 = 800 ≤ 1_000 → ok.
        assert_ok!(<ProviderRegistry as ProviderInterface<u64, u128>>::note_assignment_started(
            &PROVIDER, 400
        ));
        assert_eq!(ProviderRegistry::stake_locked(PROVIDER), 400);
        assert_eq!(ProviderRegistry::providers(PROVIDER).unwrap().active_assignments, 1);
        // 2 · (400 + 200) = 1_200 > 1_000 → rejected, state unchanged.
        assert!(<ProviderRegistry as ProviderInterface<u64, u128>>::note_assignment_started(
            &PROVIDER, 200
        )
        .is_err());
        assert_eq!(ProviderRegistry::stake_locked(PROVIDER), 400);
        assert_eq!(ProviderRegistry::providers(PROVIDER).unwrap().active_assignments, 1);
    });
}

#[test]
fn note_assignment_ended_releases_lock() {
    new_test_ext().execute_with(|| {
        register(PROVIDER, 1_000);
        assert_ok!(<ProviderRegistry as ProviderInterface<u64, u128>>::note_assignment_started(
            &PROVIDER, 400
        ));
        <ProviderRegistry as ProviderInterface<u64, u128>>::note_assignment_ended(&PROVIDER, 400);
        assert_eq!(ProviderRegistry::stake_locked(PROVIDER), 0);
        assert_eq!(ProviderRegistry::providers(PROVIDER).unwrap().active_assignments, 0);
    });
}

#[test]
fn free_stake_subtracts_locked() {
    new_test_ext().execute_with(|| {
        register(PROVIDER, 1_000);
        assert_ok!(<ProviderRegistry as ProviderInterface<u64, u128>>::note_assignment_started(
            &PROVIDER, 300
        ));
        assert_eq!(
            <ProviderRegistry as ProviderInterface<u64, u128>>::free_stake(&PROVIDER),
            700
        );
    });
}

#[test]
fn slash_moves_reserved_to_beneficiary_and_decrements_stake() {
    new_test_ext().execute_with(|| {
        register(PROVIDER, 1_000);
        Balances::make_free_balance_be(&BENEFICIARY, 10);

        let moved = <ProviderRegistry as ProviderInterface<u64, u128>>::slash(
            &PROVIDER,
            300,
            &BENEFICIARY,
        );

        assert_eq!(moved, 300);
        assert_eq!(ProviderRegistry::providers(PROVIDER).unwrap().stake, 700);
        assert_eq!(Balances::reserved_balance(PROVIDER), 700);
        assert_eq!(Balances::free_balance(BENEFICIARY), 310);
        System::assert_last_event(
            Event::Slashed { who: PROVIDER, amount: 300, beneficiary: BENEFICIARY }.into(),
        );
    });
}

#[test]
fn note_fault_increments_counter() {
    new_test_ext().execute_with(|| {
        register(PROVIDER, 1_000);
        <ProviderRegistry as ProviderInterface<u64, u128>>::note_fault(&PROVIDER);
        <ProviderRegistry as ProviderInterface<u64, u128>>::note_fault(&PROVIDER);
        assert_eq!(ProviderRegistry::providers(PROVIDER).unwrap().faults, 2);
    });
}
