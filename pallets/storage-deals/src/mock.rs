use crate as pallet_storage_deals;
use argonaut_primitives_storage::{
    interfaces::{EpochProvider, ProofPruner},
    DealId, EpochIndex, ShardIndex,
};
use frame_support::{derive_impl, parameter_types, traits::ConstU128, PalletId};
use sp_runtime::BuildStorage;

type Block = frame_system::mocking::MockBlock<Test>;

frame_support::construct_runtime!(
    pub enum Test {
        System: frame_system,
        Balances: pallet_balances,
        ProviderRegistry: pallet_provider_registry,
        StorageDeals: pallet_storage_deals,
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
    type ExistentialDeposit = ConstU128<1>;
}

impl pallet_provider_registry::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type MinStake = ConstU128<1_000>;
}

/// Epoch length (in mock blocks). Tests move epochs via [`go_to_epoch`].
pub const EPOCH_DURATION: u64 = 10;

/// Mock `EpochProvider`: epoch = block / EPOCH_DURATION; the beacon is a
/// deterministic per-epoch value available for every epoch ≥ 1 (epoch 0 → None,
/// mirroring the real pre-genesis behaviour). Tests recompute the *same* beacon
/// via `MockEpochs::beacon` when building activation/proof slices.
pub struct MockEpochs;
impl EpochProvider for MockEpochs {
    fn current_epoch() -> EpochIndex {
        (System::block_number() / EPOCH_DURATION) as EpochIndex
    }
    fn beacon(epoch: EpochIndex) -> Option<[u8; 32]> {
        if epoch == 0 {
            return None;
        }
        let mut b = [0u8; 32];
        b[..4].copy_from_slice(&epoch.to_le_bytes());
        b[4] = 0xAB; // avoid an all-selection-friendly zero pattern
        Some(b)
    }
}

/// No-op proof pruner (the real one lives in `pallet-storage-proofs`).
pub struct MockProofs;
impl ProofPruner for MockProofs {
    fn on_deal_closed(_deal: DealId, _n: ShardIndex) {}
}

parameter_types! {
    pub const DealsPalletId: PalletId = PalletId(*b"argnstor");
    pub const AssignTimeout: EpochIndex = 4;
    pub const ActivationTimeout: EpochIndex = 2;
    pub const CloseGrace: EpochIndex = 8;
    pub const CancelFee: EpochIndex = 2;
}

impl pallet_storage_deals::Config for Test {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type Providers = ProviderRegistry;
    type Epochs = MockEpochs;
    type Proofs = MockProofs;
    type PalletId = DealsPalletId;
    type AssignTimeoutEpochs = AssignTimeout;
    type ActivationTimeoutEpochs = ActivationTimeout;
    type CloseGraceEpochs = CloseGrace;
    type CancelFeeEpochs = CancelFee;
}

/// Jump to the first block of epoch `e`.
pub fn go_to_epoch(e: EpochIndex) {
    System::set_block_number(e as u64 * EPOCH_DURATION);
}

pub fn new_test_ext() -> sp_io::TestExternalities {
    let mut ext: sp_io::TestExternalities =
        frame_system::GenesisConfig::<Test>::default().build_storage().unwrap().into();
    ext.execute_with(|| System::set_block_number(1)); // epoch 0
    ext
}
