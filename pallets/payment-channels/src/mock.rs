use crate as pallet_payment_channels;
use argonaut_primitives_falcon::FalconPublic;
use core::cell::RefCell;
use frame_support::{derive_impl, traits::ConstU128};
use pallet_falcon_registry::KeyProvider;
use sp_core::crypto::AccountId32;
use sp_runtime::BuildStorage;

type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Balances: pallet_balances,
        PaymentChannels: pallet_payment_channels,
    }
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
    // 32-byte AccountId so `blake3(pk) == from` holds for the voucher key check.
    type AccountId = AccountId32;
    type Lookup = sp_runtime::traits::IdentityLookup<AccountId32>;
    type AccountData = pallet_balances::AccountData<u128>;
}

#[derive_impl(pallet_balances::config_preludes::TestDefaultConfig)]
impl pallet_balances::Config for Test {
    type AccountStore = System;
    type Balance = u128;
    type ExistentialDeposit = ConstU128<1>;
}

thread_local! {
    static KEYS: RefCell<Vec<(AccountId32, FalconPublic)>> = RefCell::new(Vec::new());
}

/// Register a registry-mode key (for the `pk: None` settle path).
pub fn register_key(acc: AccountId32, pk: FalconPublic) {
    KEYS.with(|k| k.borrow_mut().push((acc, pk)));
}

pub struct MockKeys;
impl KeyProvider<AccountId32> for MockKeys {
    fn key_of(who: &AccountId32) -> Option<FalconPublic> {
        KEYS.with(|k| k.borrow().iter().find(|(a, _)| a == who).map(|(_, pk)| pk.clone()))
    }
}

impl pallet_payment_channels::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type KeyProvider = MockKeys;
    type MinDeposit = ConstU128<10>;
}

pub fn new_test_ext() -> sp_io::TestExternalities {
    let mut ext: sp_io::TestExternalities =
        frame_system::GenesisConfig::<Test>::default().build_storage().unwrap().into();
    ext.execute_with(|| System::set_block_number(1));
    ext
}
