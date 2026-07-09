use crate as pallet_storage_proofs;
use argonaut_primitives_storage::{
    interfaces::{DealFaultHandler, DealInspector, ProofContext, ProviderInterface},
    DealId, EpochIndex, ShardIndex,
};
use core::cell::RefCell;
use frame_support::{derive_impl, parameter_types, traits::Currency};
use sp_runtime::BuildStorage;

type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Balances: pallet_balances,
        StorageProofs: pallet_storage_proofs,
    }
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
    type Block = Block;
    type AccountData = pallet_balances::AccountData<u128>;
}

#[derive_impl(pallet_balances::config_preludes::TestDefaultConfig)]
impl pallet_balances::Config for Test {
    type AccountStore = System;
    type Balance = u128;
    type ExistentialDeposit = frame_support::traits::ConstU128<1>;
}

// ── Lightweight mocks of the deals + registry contracts ─────────────────────
//
// The proofs pallet only reads the deal via `DealInspector` and slashes via
// `ProviderInterface`, so we mock both with thread-locals rather than wiring the
// real pallets. `slash` actually credits the beneficiary (via `deposit_creating`)
// so tests can assert the 50/50 payout.

thread_local! {
    static CTX: RefCell<Option<ProofContext<u64>>> = RefCell::new(None);
    static SLASHES: RefCell<Vec<(u64, u128, u64)>> = RefCell::new(Vec::new());
    static FAULTS: RefCell<Vec<(DealId, ShardIndex, u64, u128)>> = RefCell::new(Vec::new());
}

pub const DEALS_ACCOUNT: u64 = 999;

pub fn set_ctx(ctx: ProofContext<u64>) {
    CTX.with(|c| *c.borrow_mut() = Some(ctx));
}
pub fn slashes() -> Vec<(u64, u128, u64)> {
    SLASHES.with(|s| s.borrow().clone())
}
pub fn faults() -> Vec<(DealId, ShardIndex, u64, u128)> {
    FAULTS.with(|f| f.borrow().clone())
}

pub struct MockDeals;
impl DealInspector<u64> for MockDeals {
    fn proof_context(_deal: DealId, _shard: ShardIndex) -> Option<ProofContext<u64>> {
        CTX.with(|c| c.borrow().clone())
    }
}
impl DealFaultHandler<u64, u128> for MockDeals {
    fn deals_account() -> u64 {
        DEALS_ACCOUNT
    }
    fn on_shard_faulted(deal: DealId, shard: ShardIndex, provider: &u64, bounty: u128) {
        FAULTS.with(|f| f.borrow_mut().push((deal, shard, *provider, bounty)));
        // Emulate the real pallet: once faulted the shard is no longer Activated.
        CTX.with(|c| *c.borrow_mut() = None);
    }
}

pub struct MockProviders;
impl ProviderInterface<u64, u128> for MockProviders {
    fn is_active(_who: &u64) -> bool {
        true
    }
    fn free_stake(_who: &u64) -> u128 {
        u128::MAX
    }
    fn slash(who: &u64, amount: u128, beneficiary: &u64) -> u128 {
        SLASHES.with(|s| s.borrow_mut().push((*who, amount, *beneficiary)));
        // Credit the beneficiary so payout assertions work (provider debit elided).
        let _ = <Balances as Currency<u64>>::deposit_creating(beneficiary, amount);
        amount
    }
    fn note_assignment_started(_who: &u64, _escrow: u128) -> Result<(), ()> {
        Ok(())
    }
    fn note_assignment_ended(_who: &u64, _escrow: u128) {}
    fn note_fault(_who: &u64) {}
}

parameter_types! {
    pub const EpochDuration: u64 = 10; // mock BlockNumber is u64
    pub const BeaconDelay: u64 = 2;
    pub const BeaconRetention: EpochIndex = 8;
    pub const ReportWindow: EpochIndex = 4;
    pub const MissedSlash: u128 = 100;
    pub const FaultSlashAmt: u128 = 1_000;
    pub const MaxMisses: u8 = 3;
}

impl pallet_storage_proofs::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type Deals = MockDeals;
    type Providers = MockProviders;
    type EpochDuration = EpochDuration;
    type BeaconDelay = BeaconDelay;
    type BeaconRetention = BeaconRetention;
    type ReportWindowEpochs = ReportWindow;
    type MissedProofSlash = MissedSlash;
    type FaultSlash = FaultSlashAmt;
    type MaxConsecutiveMisses = MaxMisses;
}

/// Set the block number so `epoch_now() == e` (EpochDuration = 10 blocks).
pub fn go_to_epoch(e: EpochIndex) {
    System::set_block_number(e as u64 * 10);
}

pub fn new_test_ext() -> sp_io::TestExternalities {
    let mut ext: sp_io::TestExternalities =
        frame_system::GenesisConfig::<Test>::default().build_storage().unwrap().into();
    ext.execute_with(|| System::set_block_number(1));
    ext
}
