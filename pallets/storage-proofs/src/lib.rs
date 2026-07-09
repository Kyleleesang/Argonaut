//! Pallet: proof-of-possession challenge game (Implementation D, spec §D.6).
//!
//! This pallet owns epoch math and the per-epoch **beacon** (§D.6.1): at each
//! epoch boundary `on_initialize` snapshots a delayed `frame_system` block hash.
//! That block hash commits to the Prism leader digest (the digest is in the
//! header and the chain never reorgs), so it is an unpredictable, un-grindable
//! challenge seed — and it works identically on an Aura devnet, needing no
//! bridge changes (deviation D.13.1 from §9.5's post-digest beacon, which the
//! runtime cannot read).
//!
//! Providers `submit_proof` for challenged shard-epochs; anyone may
//! `report_missed` a lapsed challenge, slashing the provider (50/50 to reporter
//! and client) and, after `MaxConsecutiveMisses`, escalating the shard to
//! `Faulted` via the `DealFaultHandler` callback into `storage-deals`.
//!
//! All work is O(1): the beacon snapshot is the only `on_initialize` write and
//! it touches a constant number of storage items (D.1 rule 2).
#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

pub mod parameters;

#[frame_support::pallet]
pub mod pallet {
    use argonaut_primitives_storage::{
        bao_proof, challenged_chunk,
        interfaces::{
            DealFaultHandler, DealInspector, EpochProvider, ProofPruner, ProviderInterface,
        },
        is_selected, DealId, EpochIndex, ShardIndex, MAX_PROOF_BYTES,
    };
    use frame_support::{pallet_prelude::*, traits::Currency};
    use frame_system::pallet_prelude::*;
    use sp_runtime::traits::{Saturating, UniqueSaturatedInto, Zero};

    pub type BalanceOf<T> =
        <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

    /// Proof payload. `Aggregated` is the v2 STARK-bundle hook (§9.5), rejected
    /// for now (`Unsupported`) — additive later without a storage migration.
    #[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, PartialEq, Eq, Debug)]
    pub enum ProofPayload {
        Single(BoundedVec<u8, ConstU32<MAX_PROOF_BYTES>>),
        Aggregated(sp_core::H256),
    }

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config {
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
        type Currency: Currency<Self::AccountId>;
        /// Deal read side + fault callback (`pallet-storage-deals`).
        type Deals: DealInspector<Self::AccountId>
            + DealFaultHandler<Self::AccountId, BalanceOf<Self>>;
        /// Provider slashing/fault accounting (`pallet-provider-registry`).
        type Providers: ProviderInterface<Self::AccountId, BalanceOf<Self>>;
        #[pallet::constant]
        type EpochDuration: Get<BlockNumberFor<Self>>;
        #[pallet::constant]
        type BeaconDelay: Get<BlockNumberFor<Self>>;
        #[pallet::constant]
        type BeaconRetention: Get<EpochIndex>;
        #[pallet::constant]
        type ReportWindowEpochs: Get<EpochIndex>;
        #[pallet::constant]
        type MissedProofSlash: Get<BalanceOf<Self>>;
        #[pallet::constant]
        type FaultSlash: Get<BalanceOf<Self>>;
        #[pallet::constant]
        type MaxConsecutiveMisses: Get<u8>;
    }

    /// Epoch-snapshotted challenge beacons (D.6.1). Pruned to `BeaconRetention`.
    #[pallet::storage]
    #[pallet::getter(fn beacon_of)]
    pub type Beacons<T: Config> = StorageMap<_, Twox64Concat, EpochIndex, [u8; 32], OptionQuery>;

    /// Proofs accepted, per `(deal, shard)` per epoch. A scalar last-proven-epoch
    /// would mask a gap (prove `e+1`, miss `e`), so we record each epoch.
    #[pallet::storage]
    pub type ProofRecord<T: Config> = StorageDoubleMap<
        _,
        Blake2_128Concat,
        (DealId, ShardIndex),
        Twox64Concat,
        EpochIndex,
        (),
        OptionQuery,
    >;

    /// Successful miss reports (prevents double-slashing the same miss).
    #[pallet::storage]
    pub type MissRecord<T: Config> = StorageDoubleMap<
        _,
        Blake2_128Concat,
        (DealId, ShardIndex),
        Twox64Concat,
        EpochIndex,
        (),
        OptionQuery,
    >;

    /// Consecutive-miss counter per `(deal, shard)`; reset by a successful proof.
    #[pallet::storage]
    #[pallet::getter(fn misses)]
    pub type Misses<T: Config> =
        StorageMap<_, Blake2_128Concat, (DealId, ShardIndex), u8, ValueQuery>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        ProofAccepted { deal_id: DealId, shard_idx: ShardIndex, epoch: EpochIndex },
        MissReported { deal_id: DealId, shard_idx: ShardIndex, epoch: EpochIndex },
        ShardFaultEscalated { deal_id: DealId, shard_idx: ShardIndex },
    }

    #[pallet::error]
    pub enum Error<T> {
        /// No `Activated` assignment / deal not chargeable.
        NotActivated,
        NotProvider,
        /// Proof epoch not in `{current-1, current}`.
        EpochOutOfWindow,
        /// Challenge epoch is past the deal's end.
        AfterEnd,
        BeaconUnavailable,
        /// This shard-epoch was not challenged.
        NotSelected,
        AlreadyProven,
        BadProof,
        /// `Aggregated` payloads are not yet supported.
        Unsupported,
        /// Report before the miss deadline (`current < epoch + 2`).
        TooEarly,
        /// Report past the retained report window.
        ReportWindowClosed,
        /// The shard-epoch was actually proven — nothing to report.
        WasProven,
        AlreadyReported,
    }

    #[pallet::hooks]
    impl<T: Config> Hooks<BlockNumberFor<T>> for Pallet<T> {
        /// Epoch-boundary beacon snapshot (D.6.1). O(1): one insert + one prune.
        fn on_initialize(n: BlockNumberFor<T>) -> Weight {
            let dur = T::EpochDuration::get();
            if dur.is_zero() || !(n % dur).is_zero() {
                return T::DbWeight::get().reads(1);
            }
            let e: EpochIndex = (n / dur).unique_saturated_into();
            let target = n.saturating_sub(T::BeaconDelay::get());
            let hash = <frame_system::Pallet<T>>::block_hash(target);
            let mut beacon = [0u8; 32];
            let bytes = hash.as_ref();
            if bytes.len() == 32 {
                beacon.copy_from_slice(bytes);
            }
            Beacons::<T>::insert(e, beacon);
            Beacons::<T>::remove(e.saturating_sub(T::BeaconRetention::get()));
            T::DbWeight::get().reads_writes(2, 2)
        }
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Submit a bao proof for a challenged shard-epoch (D.6.3). Check order is
        /// exactly the spec's; the `BoundedVec` payload is `≤ MAX_PROOF_BYTES` by
        /// construction.
        #[pallet::call_index(0)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn submit_proof(
            origin: OriginFor<T>,
            deal_id: DealId,
            shard_idx: ShardIndex,
            epoch: EpochIndex,
            payload: ProofPayload,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let slice = match payload {
                ProofPayload::Single(s) => s,
                ProofPayload::Aggregated(_) => return Err(Error::<T>::Unsupported.into()),
            };
            let ctx = T::Deals::proof_context(deal_id, shard_idx)
                .ok_or(Error::<T>::NotActivated)?;
            ensure!(ctx.provider == who, Error::<T>::NotProvider);
            let current = Self::epoch_now();
            ensure!(
                epoch <= current && epoch >= current.saturating_sub(1),
                Error::<T>::EpochOutOfWindow
            );
            ensure!(epoch <= ctx.end_epoch, Error::<T>::AfterEnd);
            let beacon = Beacons::<T>::get(epoch).ok_or(Error::<T>::BeaconUnavailable)?;
            ensure!(
                is_selected(&beacon, deal_id, shard_idx, ctx.challenge_rate_ppm),
                Error::<T>::NotSelected
            );
            ensure!(
                !ProofRecord::<T>::contains_key((deal_id, shard_idx), epoch),
                Error::<T>::AlreadyProven
            );
            let chunk = challenged_chunk(&beacon, deal_id, shard_idx, ctx.shard_size);
            ensure!(
                bao_proof::verify_slice(&ctx.shard_root, ctx.shard_size, chunk, 1, slice.as_ref()),
                Error::<T>::BadProof
            );

            ProofRecord::<T>::insert((deal_id, shard_idx), epoch, ());
            Misses::<T>::remove((deal_id, shard_idx)); // reset consecutive misses
            Self::deposit_event(Event::ProofAccepted { deal_id, shard_idx, epoch });
            Ok(())
        }

        /// Report a missed challenge (D.6.4). Permissionless, no bond; an invalid
        /// report fails synchronously and costs only fees (deviation D.13.4).
        #[pallet::call_index(1)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn report_missed(
            origin: OriginFor<T>,
            deal_id: DealId,
            shard_idx: ShardIndex,
            epoch: EpochIndex,
        ) -> DispatchResult {
            let reporter = ensure_signed(origin)?;
            let ctx = T::Deals::proof_context(deal_id, shard_idx)
                .ok_or(Error::<T>::NotActivated)?;
            let current = Self::epoch_now();
            ensure!(epoch <= ctx.end_epoch, Error::<T>::AfterEnd);
            ensure!(current >= epoch.saturating_add(2), Error::<T>::TooEarly);
            ensure!(
                epoch >= current.saturating_sub(T::ReportWindowEpochs::get()),
                Error::<T>::ReportWindowClosed
            );
            let beacon = Beacons::<T>::get(epoch).ok_or(Error::<T>::BeaconUnavailable)?;
            ensure!(
                is_selected(&beacon, deal_id, shard_idx, ctx.challenge_rate_ppm),
                Error::<T>::NotSelected
            );
            ensure!(
                !ProofRecord::<T>::contains_key((deal_id, shard_idx), epoch),
                Error::<T>::WasProven
            );
            ensure!(
                !MissRecord::<T>::contains_key((deal_id, shard_idx), epoch),
                Error::<T>::AlreadyReported
            );

            MissRecord::<T>::insert((deal_id, shard_idx), epoch, ());
            let provider = ctx.provider.clone();

            // Split MissedProofSlash 50/50 reporter/client (direct compensation).
            let slash = T::MissedProofSlash::get();
            let two: BalanceOf<T> = 2u32.into();
            let half = slash / two;
            let other = slash.saturating_sub(half);
            let _ = T::Providers::slash(&provider, half, &reporter);
            let _ = T::Providers::slash(&provider, other, &ctx.client);
            T::Providers::note_fault(&provider);

            let misses = Misses::<T>::mutate((deal_id, shard_idx), |m| {
                *m = m.saturating_add(1);
                *m
            });
            Self::deposit_event(Event::MissReported { deal_id, shard_idx, epoch });

            if misses >= T::MaxConsecutiveMisses::get() {
                let bounty =
                    T::Providers::slash(&provider, T::FaultSlash::get(), &T::Deals::deals_account());
                T::Deals::on_shard_faulted(deal_id, shard_idx, &provider, bounty);
                Misses::<T>::remove((deal_id, shard_idx)); // reset for the repair provider
                Self::deposit_event(Event::ShardFaultEscalated { deal_id, shard_idx });
            }
            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        /// `block_number / EpochDuration`, saturating into `EpochIndex`.
        pub fn epoch_now() -> EpochIndex {
            let dur = T::EpochDuration::get();
            if dur.is_zero() {
                return 0;
            }
            (<frame_system::Pallet<T>>::block_number() / dur).unique_saturated_into()
        }
    }

    // ── EpochProvider (consumed by storage-deals) ───────────────────────────
    impl<T: Config> EpochProvider for Pallet<T> {
        fn current_epoch() -> EpochIndex {
            Self::epoch_now()
        }
        fn beacon(epoch: EpochIndex) -> Option<[u8; 32]> {
            Beacons::<T>::get(epoch)
        }
    }

    // ── ProofPruner (consumed by storage-deals::close_deal) ─────────────────
    impl<T: Config> ProofPruner for Pallet<T> {
        fn on_deal_closed(deal: DealId, n: ShardIndex) {
            for idx in 0..n {
                let _ = ProofRecord::<T>::clear_prefix((deal, idx), u32::MAX, None);
                let _ = MissRecord::<T>::clear_prefix((deal, idx), u32::MAX, None);
                Misses::<T>::remove((deal, idx));
            }
        }
    }
}
