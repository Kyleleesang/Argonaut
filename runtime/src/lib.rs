#![cfg_attr(not(feature = "std"), no_std)]
#![recursion_limit = "256"]

#[cfg(feature = "std")]
include!(concat!(env!("OUT_DIR"), "/wasm_binary.rs"));

mod extensions;
pub use extensions::CheckFalconSignature;
#[cfg(test)]
mod tests;

use frame_support::{
    construct_runtime, parameter_types,
    PalletId,
    traits::{ConstU128, ConstU32, ConstU64, ConstU8},
    weights::{
        constants::{BlockExecutionWeight, ExtrinsicBaseWeight, RocksDbWeight, WEIGHT_REF_TIME_PER_SECOND},
        IdentityFee, Weight,
    },
};
use frame_system::limits::{BlockLength, BlockWeights};
use pallet_transaction_payment::FungibleAdapter;
use sp_api::impl_runtime_apis;
use sp_core::{crypto::KeyTypeId, OpaqueMetadata};
use sp_std::prelude::*;
use sp_runtime::{
    generic, impl_opaque_keys,
    traits::{AccountIdLookup, BlakeTwo256, Block as BlockT, IdentifyAccount, Verify},
    transaction_validity::{TransactionSource, TransactionValidity},
    ApplyExtrinsicResult,
};
use sp_version::Cow;
#[cfg(feature = "std")]
use sp_version::NativeVersion;
use sp_version::RuntimeVersion;

pub use frame_system::Call as SystemCall;
pub use pallet_balances::Call as BalancesCall;
pub use pallet_timestamp::Call as TimestampCall;
#[cfg(any(feature = "std", test))]
pub use sp_runtime::BuildStorage;
pub use sp_runtime::{Perbill, Permill};

// ── Core types ────────────────────────────────────────────────────────────────

pub type BlockNumber = u32;
pub type Signature = argonaut_primitives_falcon::ArgonautSignature;
pub type AccountId = <<Signature as Verify>::Signer as IdentifyAccount>::AccountId;
pub type Balance = u128;
pub type Nonce = u32;
pub type Hash = sp_core::H256;

// ── Opaque types (used by CLI / RPC without knowing runtime specifics) ────────

pub mod opaque {
    use super::*;
    pub use sp_runtime::OpaqueExtrinsic as UncheckedExtrinsic;
    pub type Header = generic::Header<BlockNumber, BlakeTwo256>;
    pub type Block = generic::Block<Header, UncheckedExtrinsic>;
    pub type BlockId = generic::BlockId<Block>;

    // Empty: no session-key-based authority set exists under PoW (B0) or Prism
    // (later). Kept only because `sc_service::spawn_tasks` structurally requires
    // the runtime to implement `sp_session::SessionKeys`, regardless of consensus.
    impl_opaque_keys! {
        pub struct SessionKeys {}
    }
}

// ── Runtime version ───────────────────────────────────────────────────────────

#[sp_version::runtime_version]
pub const VERSION: RuntimeVersion = RuntimeVersion {
    spec_name: Cow::Borrowed("argonaut"),
    impl_name: Cow::Borrowed("argonaut"),
    authoring_version: 1,
    spec_version: 100,
    impl_version: 1,
    apis: RUNTIME_API_VERSIONS,
    transaction_version: 1,
    system_version: 1,
};

#[cfg(feature = "std")]
pub fn native_version() -> NativeVersion {
    NativeVersion { runtime_version: VERSION, can_author_with: Default::default() }
}

// ── Block weight / length limits ──────────────────────────────────────────────

pub const MILLISECS_PER_BLOCK: u64 = 6_000;
pub const SLOT_DURATION: u64 = MILLISECS_PER_BLOCK;
pub const MINUTES: BlockNumber = 60_000 / (MILLISECS_PER_BLOCK as BlockNumber);
pub const HOURS: BlockNumber = MINUTES * 60;
pub const DAYS: BlockNumber = HOURS * 24;

const NORMAL_DISPATCH_RATIO: Perbill = Perbill::from_percent(75);
const MAXIMUM_BLOCK_WEIGHT: Weight =
    Weight::from_parts(2u64 * WEIGHT_REF_TIME_PER_SECOND, u64::MAX);

parameter_types! {
    pub const BlockHashCount: BlockNumber = 2400;
    pub const Version: RuntimeVersion = VERSION;
    pub RuntimeBlockWeights: BlockWeights = BlockWeights::builder()
        .base_block(BlockExecutionWeight::get())
        .for_class(frame_support::dispatch::DispatchClass::all(), |weights| {
            weights.base_extrinsic = ExtrinsicBaseWeight::get();
        })
        .for_class(frame_support::dispatch::DispatchClass::Normal, |weights| {
            weights.max_total = Some(NORMAL_DISPATCH_RATIO * MAXIMUM_BLOCK_WEIGHT);
        })
        .for_class(frame_support::dispatch::DispatchClass::Operational, |weights| {
            weights.max_total = Some(MAXIMUM_BLOCK_WEIGHT);
            weights.reserved = Some(MAXIMUM_BLOCK_WEIGHT - NORMAL_DISPATCH_RATIO * MAXIMUM_BLOCK_WEIGHT);
        })
        .avg_block_initialization(Perbill::from_percent(5))
        .build_or_panic();
    pub RuntimeBlockLength: BlockLength =
        BlockLength::max_with_normal_ratio(5 * 1024 * 1024, NORMAL_DISPATCH_RATIO);
    pub const SS58Prefix: u8 = 42;
}

// ── Pallet configs ────────────────────────────────────────────────────────────

impl frame_system::Config for Runtime {
    type BaseCallFilter = frame_support::traits::Everything;
    type BlockWeights = RuntimeBlockWeights;
    type BlockLength = RuntimeBlockLength;
    type AccountId = AccountId;
    type RuntimeCall = RuntimeCall;
    type Lookup = AccountIdLookup<AccountId, ()>;
    type Nonce = Nonce;
    type Block = Block;
    type Hash = Hash;
    type Hashing = BlakeTwo256;
    type RuntimeEvent = RuntimeEvent;
    type RuntimeOrigin = RuntimeOrigin;
    type BlockHashCount = BlockHashCount;
    type DbWeight = RocksDbWeight;
    type Version = Version;
    type PalletInfo = PalletInfo;
    type OnNewAccount = ();
    type OnKilledAccount = ();
    type AccountData = pallet_balances::AccountData<Balance>;
    type SystemWeightInfo = ();
    type SS58Prefix = SS58Prefix;
    type OnSetCode = ();
    type MaxConsumers = ConstU32<16>;
    type RuntimeTask = ();
    type ExtensionsWeightInfo = ();
    type SingleBlockMigrations = ();
    type MultiBlockMigrator = ();
    type PreInherents = ();
    type PostInherents = ();
    type PostTransactions = ();
}

impl pallet_insecure_randomness_collective_flip::Config for Runtime {}

impl pallet_timestamp::Config for Runtime {
    type Moment = u64;
    // No block-authoring pallet to notify (B0: sc-consensus-pow drives authoring
    // entirely client-side; see node/src/pow.rs).
    type OnTimestampSet = ();
    type MinimumPeriod = ConstU64<{ SLOT_DURATION / 2 }>;
    type WeightInfo = ();
}

pub const EXISTENTIAL_DEPOSIT: u128 = 500;

impl pallet_balances::Config for Runtime {
    type MaxLocks = ConstU32<50>;
    type MaxReserves = ();
    type ReserveIdentifier = [u8; 8];
    type Balance = Balance;
    type RuntimeEvent = RuntimeEvent;
    type DustRemoval = ();
    type ExistentialDeposit = ConstU128<EXISTENTIAL_DEPOSIT>;
    type AccountStore = System;
    type WeightInfo = pallet_balances::weights::SubstrateWeight<Runtime>;
    type FreezeIdentifier = RuntimeFreezeReason;
    type MaxFreezes = ();
    type RuntimeHoldReason = RuntimeHoldReason;
    type RuntimeFreezeReason = RuntimeFreezeReason;
    type DoneSlashHandler = ();
}

impl pallet_transaction_payment::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type OnChargeTransaction = FungibleAdapter<Balances, ()>;
    type OperationalFeeMultiplier = ConstU8<5>;
    type WeightToFee = IdentityFee<Balance>;
    type LengthToFee = IdentityFee<Balance>;
    type FeeMultiplierUpdate = ();
    type WeightInfo = ();
}

impl pallet_sudo::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type RuntimeCall = RuntimeCall;
    type WeightInfo = pallet_sudo::weights::SubstrateWeight<Runtime>;
}

/// Storage deposit for registering a Falcon-512 public key (~900 bytes of on-chain
/// state); refunded on deregistration. See architecture guide §2.3.
pub const FALCON_KEY_DEPOSIT: Balance = 1 * EXISTENTIAL_DEPOSIT;

impl pallet_falcon_registry::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type KeyDeposit = ConstU128<FALCON_KEY_DEPOSIT>;
}

// ── Implementation D: decentralized storage platform (spec §D.8) ──────────────
//
// Four pallets wired via the interface traits in `argonaut-primitives-storage`
// (no crate cycle): ProviderRegistry ⇄ StorageDeals ⇄ StorageProofs. Constants
// come from each pallet's `parameters` module; balance-denominated ones are
// scaled by the existential deposit here. The `BaoProof` host function is
// registered in `node/src/service.rs` (done in D1).

parameter_types! {
    pub const StorageMinStake: Balance =
        pallet_provider_registry::parameters::MIN_STAKE_ED_MULTIPLE * EXISTENTIAL_DEPOSIT;
    /// Sub-account custodying slashed repair bounties (`repair_pot`).
    pub const StorageDealsPalletId: PalletId = PalletId(*b"argnstor");
    pub const AssignTimeoutEpochs: u32 =
        pallet_storage_deals::parameters::ASSIGN_TIMEOUT_EPOCHS;
    pub const ActivationTimeoutEpochs: u32 =
        pallet_storage_deals::parameters::ACTIVATION_TIMEOUT_EPOCHS;
    pub const CloseGraceEpochs: u32 = pallet_storage_deals::parameters::CLOSE_GRACE_EPOCHS;
    pub const CancelFeeEpochs: u32 = pallet_storage_deals::parameters::CANCEL_FEE_EPOCHS;
    pub const StorageEpochDuration: BlockNumber =
        pallet_storage_proofs::parameters::EPOCH_DURATION_BLOCKS;
    pub const StorageBeaconDelay: BlockNumber =
        pallet_storage_proofs::parameters::BEACON_DELAY_BLOCKS;
    pub const StorageBeaconRetention: u32 =
        pallet_storage_proofs::parameters::BEACON_RETENTION_EPOCHS;
    pub const StorageReportWindow: u32 =
        pallet_storage_proofs::parameters::REPORT_WINDOW_EPOCHS;
    pub const StorageMissedProofSlash: Balance =
        pallet_storage_proofs::parameters::MISSED_PROOF_SLASH_ED_MULTIPLE * EXISTENTIAL_DEPOSIT;
    pub const StorageFaultSlash: Balance =
        pallet_storage_proofs::parameters::FAULT_SLASH_ED_MULTIPLE * EXISTENTIAL_DEPOSIT;
    pub const StorageMaxConsecutiveMisses: u8 =
        pallet_storage_proofs::parameters::MAX_CONSECUTIVE_MISSES;
    pub const ChannelMinDeposit: Balance =
        pallet_payment_channels::parameters::MIN_DEPOSIT_ED_MULTIPLE * EXISTENTIAL_DEPOSIT;
}

impl pallet_provider_registry::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type MinStake = StorageMinStake;
}

impl pallet_storage_deals::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type Providers = ProviderRegistry;
    type Epochs = StorageProofs;
    type Proofs = StorageProofs;
    type PalletId = StorageDealsPalletId;
    type AssignTimeoutEpochs = AssignTimeoutEpochs;
    type ActivationTimeoutEpochs = ActivationTimeoutEpochs;
    type CloseGraceEpochs = CloseGraceEpochs;
    type CancelFeeEpochs = CancelFeeEpochs;
}

impl pallet_storage_proofs::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type Deals = StorageDeals;
    type Providers = ProviderRegistry;
    type EpochDuration = StorageEpochDuration;
    type BeaconDelay = StorageBeaconDelay;
    type BeaconRetention = StorageBeaconRetention;
    type ReportWindowEpochs = StorageReportWindow;
    type MissedProofSlash = StorageMissedProofSlash;
    type FaultSlash = StorageFaultSlash;
    type MaxConsecutiveMisses = StorageMaxConsecutiveMisses;
}

impl pallet_payment_channels::Config for Runtime {
    type RuntimeEvent = RuntimeEvent;
    type Currency = Balances;
    type KeyProvider = FalconRegistry;
    type MinDeposit = ChannelMinDeposit;
}

// ── Construct runtime ─────────────────────────────────────────────────────────

construct_runtime!(
    pub enum Runtime {
        System: frame_system,
        RandomnessCollectiveFlip: pallet_insecure_randomness_collective_flip,
        Timestamp: pallet_timestamp,
        Balances: pallet_balances,
        FalconRegistry: pallet_falcon_registry,
        TransactionPayment: pallet_transaction_payment,
        Sudo: pallet_sudo,
        // Implementation D: decentralized storage platform (§D.8).
        ProviderRegistry: pallet_provider_registry,
        StorageDeals: pallet_storage_deals,
        StorageProofs: pallet_storage_proofs,
        PaymentChannels: pallet_payment_channels,
    }
);

// ── Type aliases ──────────────────────────────────────────────────────────────

pub type Address = sp_runtime::MultiAddress<AccountId, ()>;
pub type Header = generic::Header<BlockNumber, BlakeTwo256>;
pub type Block = generic::Block<Header, UncheckedExtrinsic>;
pub type SignedExtra = (
    // Must run first: for `General`-preamble (registry-mode) extrinsics it authorizes
    // the origin itself, and everything after it assumes an already-authorized signer.
    CheckFalconSignature,
    frame_system::CheckNonZeroSender<Runtime>,
    frame_system::CheckSpecVersion<Runtime>,
    frame_system::CheckTxVersion<Runtime>,
    frame_system::CheckGenesis<Runtime>,
    frame_system::CheckEra<Runtime>,
    frame_system::CheckNonce<Runtime>,
    frame_system::CheckWeight<Runtime>,
    pallet_transaction_payment::ChargeTransactionPayment<Runtime>,
);
pub type UncheckedExtrinsic =
    generic::UncheckedExtrinsic<Address, RuntimeCall, Signature, SignedExtra>;
pub type SignedPayload = generic::SignedPayload<RuntimeCall, SignedExtra>;
pub type Executive = frame_executive::Executive<
    Runtime,
    Block,
    frame_system::ChainContext<Runtime>,
    Runtime,
    AllPalletsWithSystem,
>;

// ── Benchmarking ──────────────────────────────────────────────────────────────

#[cfg(feature = "runtime-benchmarks")]
mod benches {
    frame_benchmarking::define_benchmarks!(
        [frame_benchmarking, BaselineBench::<Runtime>]
        [frame_system, SystemBench::<Runtime>]
        [pallet_balances, Balances]
        [pallet_timestamp, Timestamp]
        [pallet_falcon_registry, FalconRegistry]
    );
}

// ── Runtime APIs ──────────────────────────────────────────────────────────────

impl_runtime_apis! {
    impl sp_api::Core<Block> for Runtime {
        fn version() -> RuntimeVersion { VERSION }
        fn execute_block(block: Block) { Executive::execute_block(block); }
        fn initialize_block(header: &<Block as BlockT>::Header) -> sp_runtime::ExtrinsicInclusionMode {
            Executive::initialize_block(header)
        }
    }

    impl sp_api::Metadata<Block> for Runtime {
        fn metadata() -> OpaqueMetadata {
            OpaqueMetadata::new(Runtime::metadata().into())
        }
        fn metadata_at_version(version: u32) -> Option<OpaqueMetadata> {
            Runtime::metadata_at_version(version)
        }
        fn metadata_versions() -> sp_std::vec::Vec<u32> {
            Runtime::metadata_versions()
        }
    }

    impl sp_block_builder::BlockBuilder<Block> for Runtime {
        fn apply_extrinsic(extrinsic: <Block as BlockT>::Extrinsic) -> ApplyExtrinsicResult {
            Executive::apply_extrinsic(extrinsic)
        }
        fn finalize_block() -> <Block as BlockT>::Header {
            Executive::finalize_block()
        }
        fn inherent_extrinsics(data: sp_inherents::InherentData) -> Vec<<Block as BlockT>::Extrinsic> {
            data.create_extrinsics()
        }
        fn check_inherents(block: Block, data: sp_inherents::InherentData) -> sp_inherents::CheckInherentsResult {
            data.check_extrinsics(&block)
        }
    }

    impl sp_transaction_pool::runtime_api::TaggedTransactionQueue<Block> for Runtime {
        fn validate_transaction(
            source: TransactionSource,
            tx: <Block as BlockT>::Extrinsic,
            block_hash: <Block as BlockT>::Hash,
        ) -> TransactionValidity {
            Executive::validate_transaction(source, tx, block_hash)
        }
    }

    impl sp_offchain::OffchainWorkerApi<Block> for Runtime {
        fn offchain_worker(header: &<Block as BlockT>::Header) {
            Executive::offchain_worker(header)
        }
    }

    impl sp_session::SessionKeys<Block> for Runtime {
        fn generate_session_keys(seed: Option<Vec<u8>>) -> Vec<u8> {
            opaque::SessionKeys::generate(seed)
        }
        fn decode_session_keys(encoded: Vec<u8>) -> Option<Vec<(Vec<u8>, KeyTypeId)>> {
            opaque::SessionKeys::decode_into_raw_public_keys(&encoded)
        }
    }

    impl frame_system_rpc_runtime_api::AccountNonceApi<Block, AccountId, Nonce> for Runtime {
        fn account_nonce(account: AccountId) -> Nonce {
            System::account_nonce(account)
        }
    }

    impl pallet_transaction_payment_rpc_runtime_api::TransactionPaymentApi<Block, Balance> for Runtime {
        fn query_info(
            uxt: <Block as BlockT>::Extrinsic,
            len: u32,
        ) -> pallet_transaction_payment_rpc_runtime_api::RuntimeDispatchInfo<Balance> {
            TransactionPayment::query_info(uxt, len)
        }
        fn query_fee_details(
            uxt: <Block as BlockT>::Extrinsic,
            len: u32,
        ) -> pallet_transaction_payment::FeeDetails<Balance> {
            TransactionPayment::query_fee_details(uxt, len)
        }
        fn query_weight_to_fee(weight: Weight) -> Balance {
            TransactionPayment::weight_to_fee(weight)
        }
        fn query_length_to_fee(length: u32) -> Balance {
            TransactionPayment::length_to_fee(length)
        }
    }

    impl pallet_transaction_payment_rpc_runtime_api::TransactionPaymentCallApi<Block, Balance, RuntimeCall>
        for Runtime
    {
        fn query_call_info(call: RuntimeCall, len: u32) -> pallet_transaction_payment_rpc_runtime_api::RuntimeDispatchInfo<Balance> {
            TransactionPayment::query_call_info(call, len)
        }
        fn query_call_fee_details(call: RuntimeCall, len: u32) -> pallet_transaction_payment::FeeDetails<Balance> {
            TransactionPayment::query_call_fee_details(call, len)
        }
        fn query_weight_to_fee(weight: Weight) -> Balance {
            TransactionPayment::weight_to_fee(weight)
        }
        fn query_length_to_fee(length: u32) -> Balance {
            TransactionPayment::length_to_fee(length)
        }
    }

    impl sp_genesis_builder::GenesisBuilder<Block> for Runtime {
        fn build_state(config: Vec<u8>) -> sp_genesis_builder::Result {
            frame_support::genesis_builder_helper::build_state::<RuntimeGenesisConfig>(config)
        }
        fn get_preset(id: &Option<sp_genesis_builder::PresetId>) -> Option<Vec<u8>> {
            frame_support::genesis_builder_helper::get_preset::<RuntimeGenesisConfig>(id, |_| None)
        }
        fn preset_names() -> Vec<sp_genesis_builder::PresetId> {
            vec![]
        }
    }

    #[cfg(feature = "runtime-benchmarks")]
    impl frame_benchmarking::Benchmark<Block> for Runtime {
        fn benchmark_metadata(extra: bool) -> (Vec<frame_benchmarking::BenchmarkList>, Vec<frame_support::traits::StorageInfo>) {
            use frame_benchmarking::{baseline, Benchmarking, BenchmarkList};
            use frame_support::traits::StorageInfoTrait;
            use frame_system_benchmarking::Pallet as SystemBench;
            use baseline::Pallet as BaselineBench;

            let mut list = Vec::<BenchmarkList>::new();
            list_benchmarks!(list, extra);
            let storage_info = AllPalletsWithSystem::storage_info();
            (list, storage_info)
        }

        fn dispatch_benchmark(config: frame_benchmarking::BenchmarkConfig) -> Result<Vec<frame_benchmarking::BenchmarkBatch>, sp_runtime::RuntimeString> {
            use frame_benchmarking::{baseline, Benchmarking, BenchmarkBatch};
            use frame_system_benchmarking::Pallet as SystemBench;
            use baseline::Pallet as BaselineBench;
            use frame_support::traits::TrackedStorageKey;

            impl frame_system_benchmarking::Config for Runtime {}
            impl baseline::Config for Runtime {}

            let whitelist: Vec<TrackedStorageKey> = vec![
                hex_literal::hex!("26aa394eea5630e07c48ae0c9558cef702a5c1b19ab7a04f536c519aca4983ac").to_vec().into(),
                hex_literal::hex!("c2261276cc9d1f8598ea4b6a74b15c2f57c875e4cff74148e4628f264b974c80").to_vec().into(),
                hex_literal::hex!("26aa394eea5630e07c48ae0c9558cef7ff553b5a9862a516939d82b3d3d8661a").to_vec().into(),
                hex_literal::hex!("26aa394eea5630e07c48ae0c9558cef70a98fdbe9ce6c55837576c60c7af3850").to_vec().into(),
                hex_literal::hex!("26aa394eea5630e07c48ae0c9558cef780d41e5e16056765bc8461851072c9d7").to_vec().into(),
            ];
            let mut batches = Vec::<BenchmarkBatch>::new();
            let params = (&config, &whitelist);
            add_benchmarks!(params, batches);
            Ok(batches)
        }
    }

    #[cfg(feature = "try-runtime")]
    impl frame_try_runtime::TryRuntime<Block> for Runtime {
        fn on_runtime_upgrade(checks: frame_try_runtime::UpgradeCheckSelect) -> (Weight, Weight) {
            let weight = Executive::try_runtime_upgrade(checks).unwrap();
            (weight, RuntimeBlockWeights::get().max_block)
        }
        fn execute_block(
            block: Block,
            state_root_check: bool,
            signature_check: bool,
            select: frame_try_runtime::TryStateSelect,
        ) -> Weight {
            Executive::try_execute_block(block, state_root_check, signature_check, select).unwrap()
        }
    }
}
