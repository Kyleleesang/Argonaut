use crate as pallet_falcon_registry;
use frame_support::{derive_impl, traits::ConstU128};
use sp_core::crypto::AccountId32;
use sp_runtime::BuildStorage;

type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Balances: pallet_balances,
        FalconRegistry: pallet_falcon_registry,
    }
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
    // Bootstrap-mode registration checks `blake3(pk) == AccountId`, which only makes
    // sense for a 32-byte AccountId — must match `argonaut-runtime`'s real AccountId32,
    // not the mock-default `u64`.
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

impl pallet_falcon_registry::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type KeyDeposit = ConstU128<100>;
}

// Bootstrap-mode AccountIds are derived from `blake3(pk)`, so tests fund whichever
// account a given keypair happens to hash to, rather than a genesis-fixed list.
pub fn new_test_ext() -> sp_io::TestExternalities {
    frame_system::GenesisConfig::<Test>::default().build_storage().unwrap().into()
}
