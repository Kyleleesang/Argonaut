//! Pallet: storage-deal lifecycle (Implementation D, spec §D.5).
//!
//! A deal escrows `n · duration · price` from the client, then providers accept
//! and *activate* individual shards by proving possession of an activation-chunk
//! (a bao slice, verified by the `BaoProof` host function). Once all `n` shards
//! activate the deal goes `Active` and vesting begins for every shard together.
//!
//! # Design notes (spec §D.5.2 / §D.13)
//! - **Vesting is time-based, independent of proofs** (D.13.6): an `Activated`
//!   shard earns `price` per completed epoch regardless of proof submissions;
//!   misbehaviour is punished by *slashing* in `storage-proofs`, not by
//!   withholding payment. `claim_earnings` is a pull.
//! - **All state transitions are lazy** (D.1 rule 2): every timeout is checked
//!   inside a user call; there is no `on_initialize` iteration over deals.
//! - Cross-pallet coupling is via traits in `argonaut-primitives-storage`
//!   (`ProviderInterface`, `EpochProvider`, `ProofPruner`) so this pallet and
//!   `storage-proofs` reference each other's contracts without a crate cycle.
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
        activation_chunk, bao_proof,
        interfaces::{
            DealFaultHandler, DealInspector, EpochProvider, ProofContext, ProofPruner,
            ProviderInterface,
        },
        DealId, DealParams, EpochIndex, ShardIndex, CHUNK_SIZE, MAX_PROOF_BYTES,
    };
    use frame_support::{
        pallet_prelude::*,
        traits::{BalanceStatus, Currency, ExistenceRequirement, ReservableCurrency},
        PalletId,
    };
    use frame_system::pallet_prelude::*;
    use sp_runtime::traits::{AccountIdConversion, CheckedMul, Saturating, Zero};
    use sp_std::vec::Vec;

    pub type BalanceOf<T> =
        <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config {
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
        type Currency: ReservableCurrency<Self::AccountId>;
        /// Provider stake/slashing/assignment accounting (`pallet-provider-registry`).
        type Providers: ProviderInterface<Self::AccountId, BalanceOf<Self>>;
        /// Epoch math + beacon lookups (`pallet-storage-proofs`).
        type Epochs: EpochProvider;
        /// Proof-record pruning on close (`pallet-storage-proofs`).
        type Proofs: ProofPruner;
        /// Sub-account custodying slashed repair bounties (`repair_pot`).
        #[pallet::constant]
        type PalletId: Get<PalletId>;
        #[pallet::constant]
        type AssignTimeoutEpochs: Get<EpochIndex>;
        #[pallet::constant]
        type ActivationTimeoutEpochs: Get<EpochIndex>;
        #[pallet::constant]
        type CloseGraceEpochs: Get<EpochIndex>;
        #[pallet::constant]
        type CancelFeeEpochs: Get<EpochIndex>;
    }

    #[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, PartialEq, Eq, Debug)]
    pub enum DealState {
        Listed,
        Assigning,
        Active,
        Degraded,
        Expired,
        Cancelled,
        Lost,
    }

    #[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, PartialEq, Eq, Debug)]
    pub enum ShardState {
        Accepted,
        Activated,
        Faulted,
    }

    #[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, PartialEq, Eq, Debug)]
    pub struct Deal<AccountId, Balance> {
        pub client: AccountId,
        pub params: DealParams<Balance>,
        pub state: DealState,
        pub created_epoch: EpochIndex,
        /// Set when the deal first becomes `Active` (all `n` activated).
        pub start_epoch: EpochIndex,
        pub end_epoch: EpochIndex,
        /// Reserved on the client; decremented on payouts/refunds.
        pub escrow_remaining: Balance,
        /// `FaultSlash` bounty pool (custodied in the pallet sub-account).
        pub repair_pot: Balance,
        pub activated_count: u16,
        pub faulted_count: u16,
    }

    #[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, PartialEq, Eq, Debug)]
    pub struct Assignment<AccountId> {
        pub provider: AccountId,
        pub state: ShardState,
        pub accepted_epoch: EpochIndex,
        pub activated_epoch: EpochIndex,
        /// Last epoch already paid out (init = activated_epoch on full activation).
        pub claimed_through: EpochIndex,
        /// Epoch the shard faulted; `0` = none. Caps a faulted provider's vesting.
        pub fault_epoch: EpochIndex,
    }

    #[pallet::storage]
    #[pallet::getter(fn deals)]
    pub type Deals<T: Config> =
        StorageMap<_, Blake2_128Concat, DealId, Deal<T::AccountId, BalanceOf<T>>, OptionQuery>;

    #[pallet::storage]
    #[pallet::getter(fn assignment)]
    pub type ShardAssignments<T: Config> = StorageDoubleMap<
        _,
        Blake2_128Concat,
        DealId,
        Twox64Concat,
        ShardIndex,
        Assignment<T::AccountId>,
        OptionQuery,
    >;

    /// Enforces one shard per provider per deal.
    #[pallet::storage]
    pub type DealProviders<T: Config> = StorageDoubleMap<
        _,
        Blake2_128Concat,
        DealId,
        Blake2_128Concat,
        T::AccountId,
        (),
        OptionQuery,
    >;

    #[pallet::storage]
    #[pallet::getter(fn next_deal_id)]
    pub type NextDealId<T: Config> = StorageValue<_, DealId, ValueQuery>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        DealCreated { deal_id: DealId, client: T::AccountId },
        ShardAccepted { deal_id: DealId, shard_idx: ShardIndex, provider: T::AccountId },
        ShardActivated { deal_id: DealId, shard_idx: ShardIndex, provider: T::AccountId },
        DealActive { deal_id: DealId },
        AssignmentExpired { deal_id: DealId, shard_idx: ShardIndex, provider: T::AccountId },
        EarningsClaimed {
            deal_id: DealId,
            shard_idx: ShardIndex,
            provider: T::AccountId,
            amount: BalanceOf<T>,
        },
        DealCancelled { deal_id: DealId },
        DealTerminated { deal_id: DealId },
        DealClosed { deal_id: DealId },
        ShardFaulted { deal_id: DealId, shard_idx: ShardIndex, provider: T::AccountId },
        DealDegraded { deal_id: DealId },
        DealRepaired { deal_id: DealId, shard_idx: ShardIndex, provider: T::AccountId },
        DealLost { deal_id: DealId },
    }

    #[pallet::error]
    pub enum Error<T> {
        DealNotFound,
        BadParams,
        Overflow,
        AssignWindowClosed,
        SlotOccupied,
        AlreadyProviding,
        NotProvider,
        WrongState,
        ActivationWindowClosed,
        ActivationWindowNotOpen,
        BadProof,
        NothingToClaim,
        NotClient,
        TooEarly,
        InsufficientBalance,
        InsufficientStake,
        ProviderNotActive,
        BeaconUnavailable,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Create a deal and reserve `n · duration · price` escrow on the client.
        #[pallet::call_index(0)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn create_deal(origin: OriginFor<T>, params: DealParams<BalanceOf<T>>) -> DispatchResult {
            let client = ensure_signed(origin)?;
            let n = params.n;
            ensure!(params.k >= 1 && params.k < n, Error::<T>::BadParams);
            ensure!(params.shards.len() == n as usize, Error::<T>::BadParams);
            ensure!(params.duration_epochs > 0, Error::<T>::BadParams);
            ensure!(params.challenge_rate_ppm <= 1_000_000, Error::<T>::BadParams);
            for s in params.shards.iter() {
                ensure!(s.size > 0 && s.size % CHUNK_SIZE as u64 == 0, Error::<T>::BadParams);
            }

            let per = Self::per_shard_escrow(&params)?;
            let n_bal: BalanceOf<T> = (n as u32).into();
            let total = per.checked_mul(&n_bal).ok_or(Error::<T>::Overflow)?;
            T::Currency::reserve(&client, total).map_err(|_| Error::<T>::InsufficientBalance)?;

            let deal_id = NextDealId::<T>::get();
            NextDealId::<T>::put(deal_id.saturating_add(1));
            let created = T::Epochs::current_epoch();
            Deals::<T>::insert(
                deal_id,
                Deal {
                    client: client.clone(),
                    params,
                    state: DealState::Listed,
                    created_epoch: created,
                    start_epoch: 0,
                    end_epoch: 0,
                    escrow_remaining: total,
                    repair_pot: Zero::zero(),
                    activated_count: 0,
                    faulted_count: 0,
                },
            );
            Self::deposit_event(Event::DealCreated { deal_id, client });
            Ok(())
        }

        /// Accept a shard slot (initial fill, or repair of a `Faulted` slot).
        #[pallet::call_index(1)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn accept_shard(
            origin: OriginFor<T>,
            deal_id: DealId,
            shard_idx: ShardIndex,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let mut deal = Deals::<T>::get(deal_id).ok_or(Error::<T>::DealNotFound)?;
            ensure!(shard_idx < deal.params.n as u16, Error::<T>::BadParams);
            let current = T::Epochs::current_epoch();
            match deal.state {
                DealState::Listed | DealState::Assigning => ensure!(
                    current <= deal.created_epoch.saturating_add(T::AssignTimeoutEpochs::get()),
                    Error::<T>::AssignWindowClosed
                ),
                DealState::Active | DealState::Degraded => {} // repair path
                _ => return Err(Error::<T>::WrongState.into()),
            }
            match ShardAssignments::<T>::get(deal_id, shard_idx) {
                None => {}
                Some(a) if a.state == ShardState::Faulted => {}
                Some(_) => return Err(Error::<T>::SlotOccupied.into()),
            }
            ensure!(
                !DealProviders::<T>::contains_key(deal_id, &who),
                Error::<T>::AlreadyProviding
            );
            ensure!(T::Providers::is_active(&who), Error::<T>::ProviderNotActive);

            let per = Self::per_shard_escrow(&deal.params)?;
            T::Providers::note_assignment_started(&who, per)
                .map_err(|_| Error::<T>::InsufficientStake)?;

            ShardAssignments::<T>::insert(
                deal_id,
                shard_idx,
                Assignment {
                    provider: who.clone(),
                    state: ShardState::Accepted,
                    accepted_epoch: current,
                    activated_epoch: 0,
                    claimed_through: 0,
                    fault_epoch: 0,
                },
            );
            DealProviders::<T>::insert(deal_id, &who, ());
            if deal.state == DealState::Listed {
                deal.state = DealState::Assigning;
                Deals::<T>::insert(deal_id, deal);
            }
            Self::deposit_event(Event::ShardAccepted { deal_id, shard_idx, provider: who });
            Ok(())
        }

        /// Activate an accepted shard by proving possession of its activation chunk.
        #[pallet::call_index(2)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn activate_shard(
            origin: OriginFor<T>,
            deal_id: DealId,
            shard_idx: ShardIndex,
            slice: BoundedVec<u8, ConstU32<MAX_PROOF_BYTES>>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let mut deal = Deals::<T>::get(deal_id).ok_or(Error::<T>::DealNotFound)?;
            let mut a =
                ShardAssignments::<T>::get(deal_id, shard_idx).ok_or(Error::<T>::NotProvider)?;
            ensure!(a.provider == who, Error::<T>::NotProvider);
            ensure!(a.state == ShardState::Accepted, Error::<T>::WrongState);

            let current = T::Epochs::current_epoch();
            // Beacon of accepted_epoch+1 is unpredictable at accept time.
            ensure!(current > a.accepted_epoch, Error::<T>::ActivationWindowNotOpen);
            ensure!(
                current
                    <= a.accepted_epoch
                        .saturating_add(1)
                        .saturating_add(T::ActivationTimeoutEpochs::get()),
                Error::<T>::ActivationWindowClosed
            );
            let beacon = T::Epochs::beacon(a.accepted_epoch.saturating_add(1))
                .ok_or(Error::<T>::BeaconUnavailable)?;
            let shard =
                deal.params.shards.get(shard_idx as usize).ok_or(Error::<T>::BadParams)?.clone();
            let chunk = activation_chunk(&beacon, deal_id, shard_idx, shard.size);
            ensure!(
                bao_proof::verify_slice(&shard.root, shard.size, chunk, 1, slice.as_ref()),
                Error::<T>::BadProof
            );

            a.state = ShardState::Activated;
            a.activated_epoch = current;
            a.claimed_through = current;
            let n = deal.params.n as u16;

            match deal.state {
                DealState::Assigning => {
                    deal.activated_count = deal.activated_count.saturating_add(1);
                    ShardAssignments::<T>::insert(deal_id, shard_idx, a);
                    if deal.activated_count == n {
                        deal.state = DealState::Active;
                        deal.start_epoch = current;
                        deal.end_epoch = current.saturating_add(deal.params.duration_epochs);
                        // Vesting starts together: rebase every shard to start_epoch.
                        for i in 0..n {
                            ShardAssignments::<T>::mutate(deal_id, i, |slot| {
                                if let Some(s) = slot {
                                    s.activated_epoch = current;
                                    s.claimed_through = current;
                                }
                            });
                        }
                        Deals::<T>::insert(deal_id, deal);
                        Self::deposit_event(Event::DealActive { deal_id });
                    } else {
                        Deals::<T>::insert(deal_id, deal);
                    }
                    Self::deposit_event(Event::ShardActivated { deal_id, shard_idx, provider: who });
                }
                DealState::Degraded | DealState::Active => {
                    // Repair path: shard returns to service.
                    deal.faulted_count = deal.faulted_count.saturating_sub(1);
                    deal.activated_count = deal.activated_count.saturating_add(1);
                    ShardAssignments::<T>::insert(deal_id, shard_idx, a);
                    let bounty = deal.repair_pot;
                    if !bounty.is_zero() {
                        deal.repair_pot = Zero::zero();
                        let _ = T::Currency::transfer(
                            &Self::account_id(),
                            &who,
                            bounty,
                            ExistenceRequirement::AllowDeath,
                        );
                    }
                    if deal.faulted_count == 0 && deal.state == DealState::Degraded {
                        deal.state = DealState::Active;
                    }
                    Deals::<T>::insert(deal_id, deal);
                    Self::deposit_event(Event::DealRepaired { deal_id, shard_idx, provider: who });
                }
                _ => return Err(Error::<T>::WrongState.into()),
            }
            Ok(())
        }

        /// Pull vested earnings for one shard (time-based; D.5.2).
        #[pallet::call_index(3)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn claim_earnings(
            origin: OriginFor<T>,
            deal_id: DealId,
            shard_idx: ShardIndex,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let mut deal = Deals::<T>::get(deal_id).ok_or(Error::<T>::DealNotFound)?;
            let mut a =
                ShardAssignments::<T>::get(deal_id, shard_idx).ok_or(Error::<T>::NotProvider)?;
            ensure!(a.provider == who, Error::<T>::NotProvider);
            ensure!(
                matches!(a.state, ShardState::Activated | ShardState::Faulted),
                Error::<T>::WrongState
            );

            let current = T::Epochs::current_epoch();
            let upper = Self::vesting_upper(&deal, &a, current);
            ensure!(upper > a.claimed_through, Error::<T>::NothingToClaim);

            let epochs = upper.saturating_sub(a.claimed_through);
            let owed = deal.params.price_per_shard_epoch.saturating_mul(epochs.into());
            let owed = owed.min(deal.escrow_remaining);
            ensure!(!owed.is_zero(), Error::<T>::NothingToClaim);

            let remainder =
                T::Currency::repatriate_reserved(&deal.client, &who, owed, BalanceStatus::Free)
                    .unwrap_or(owed);
            let paid = owed.saturating_sub(remainder);
            a.claimed_through = upper;
            deal.escrow_remaining = deal.escrow_remaining.saturating_sub(paid);
            ShardAssignments::<T>::insert(deal_id, shard_idx, a);
            Deals::<T>::insert(deal_id, deal);
            Self::deposit_event(Event::EarningsClaimed {
                deal_id,
                shard_idx,
                provider: who,
                amount: paid,
            });
            Ok(())
        }

        /// Permissionlessly reclaim a shard slot whose activation window lapsed.
        #[pallet::call_index(4)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn expire_assignment(
            origin: OriginFor<T>,
            deal_id: DealId,
            shard_idx: ShardIndex,
        ) -> DispatchResult {
            ensure_signed(origin)?;
            let deal = Deals::<T>::get(deal_id).ok_or(Error::<T>::DealNotFound)?;
            let a =
                ShardAssignments::<T>::get(deal_id, shard_idx).ok_or(Error::<T>::NotProvider)?;
            ensure!(a.state == ShardState::Accepted, Error::<T>::WrongState);
            let current = T::Epochs::current_epoch();
            ensure!(
                current
                    > a.accepted_epoch
                        .saturating_add(1)
                        .saturating_add(T::ActivationTimeoutEpochs::get()),
                Error::<T>::TooEarly
            );
            let per = Self::per_shard_escrow(&deal.params)?;
            T::Providers::note_assignment_ended(&a.provider, per);
            ShardAssignments::<T>::remove(deal_id, shard_idx);
            DealProviders::<T>::remove(deal_id, &a.provider);
            Self::deposit_event(Event::AssignmentExpired {
                deal_id,
                shard_idx,
                provider: a.provider,
            });
            Ok(())
        }

        /// Cancel a not-yet-active deal; refunds all escrow to the client.
        #[pallet::call_index(5)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn cancel_deal(origin: OriginFor<T>, deal_id: DealId) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let mut deal = Deals::<T>::get(deal_id).ok_or(Error::<T>::DealNotFound)?;
            ensure!(deal.client == who, Error::<T>::NotClient);
            ensure!(
                matches!(deal.state, DealState::Listed | DealState::Assigning),
                Error::<T>::WrongState
            );
            let per = Self::per_shard_escrow(&deal.params)?;
            Self::release_assignments(deal_id, per);
            T::Currency::unreserve(&deal.client, deal.escrow_remaining);
            deal.escrow_remaining = Zero::zero();
            deal.state = DealState::Cancelled;
            Deals::<T>::insert(deal_id, deal);
            Self::deposit_event(Event::DealCancelled { deal_id });
            Ok(())
        }

        /// Terminate an active deal early: pay vested earnings + a cancellation fee
        /// per Activated shard, refund the remainder.
        #[pallet::call_index(6)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn terminate(origin: OriginFor<T>, deal_id: DealId) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let mut deal = Deals::<T>::get(deal_id).ok_or(Error::<T>::DealNotFound)?;
            ensure!(deal.client == who, Error::<T>::NotClient);
            ensure!(
                matches!(deal.state, DealState::Active | DealState::Degraded),
                Error::<T>::WrongState
            );
            let current = T::Epochs::current_epoch();
            let per = Self::per_shard_escrow(&deal.params)?;
            let price = deal.params.price_per_shard_epoch;
            let cancel_fee_epochs: BalanceOf<T> = T::CancelFeeEpochs::get().into();
            let cancel_fee = price.saturating_mul(cancel_fee_epochs);

            let assignments: Vec<(ShardIndex, Assignment<T::AccountId>)> =
                ShardAssignments::<T>::drain_prefix(deal_id).collect();
            for (_idx, a) in assignments {
                if matches!(a.state, ShardState::Activated | ShardState::Faulted) {
                    let upper = Self::vesting_upper(&deal, &a, current);
                    let epochs = upper.saturating_sub(a.claimed_through);
                    let mut payout = price.saturating_mul(epochs.into());
                    if a.state == ShardState::Activated {
                        payout = payout.saturating_add(cancel_fee);
                    }
                    let payout = payout.min(deal.escrow_remaining);
                    if !payout.is_zero() {
                        let remainder = T::Currency::repatriate_reserved(
                            &deal.client,
                            &a.provider,
                            payout,
                            BalanceStatus::Free,
                        )
                        .unwrap_or(payout);
                        deal.escrow_remaining =
                            deal.escrow_remaining.saturating_sub(payout.saturating_sub(remainder));
                    }
                }
                T::Providers::note_assignment_ended(&a.provider, per);
            }
            let _ = DealProviders::<T>::clear_prefix(deal_id, u32::MAX, None);
            T::Currency::unreserve(&deal.client, deal.escrow_remaining);
            deal.escrow_remaining = Zero::zero();
            deal.state = DealState::Cancelled;
            Deals::<T>::insert(deal_id, deal);
            Self::deposit_event(Event::DealTerminated { deal_id });
            Ok(())
        }

        /// Permissionlessly close a deal past its grace window; refunds the
        /// remaining escrow to the client (unclaimed earnings are forfeited).
        #[pallet::call_index(7)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn close_deal(origin: OriginFor<T>, deal_id: DealId) -> DispatchResult {
            ensure_signed(origin)?;
            let mut deal = Deals::<T>::get(deal_id).ok_or(Error::<T>::DealNotFound)?;
            ensure!(
                matches!(deal.state, DealState::Active | DealState::Degraded | DealState::Lost),
                Error::<T>::WrongState
            );
            let current = T::Epochs::current_epoch();
            ensure!(
                current > deal.end_epoch.saturating_add(T::CloseGraceEpochs::get()),
                Error::<T>::TooEarly
            );
            let per = Self::per_shard_escrow(&deal.params)?;
            let n = deal.params.n as u16;
            Self::release_assignments(deal_id, per);
            T::Currency::unreserve(&deal.client, deal.escrow_remaining);
            deal.escrow_remaining = Zero::zero();
            deal.state = DealState::Expired;
            Deals::<T>::insert(deal_id, deal);
            T::Proofs::on_deal_closed(deal_id, n);
            Self::deposit_event(Event::DealClosed { deal_id });
            Ok(())
        }
    }

    // ── Internal helpers ────────────────────────────────────────────────────
    impl<T: Config> Pallet<T> {
        /// The pallet sub-account custodying `repair_pot` funds.
        pub fn account_id() -> T::AccountId {
            T::PalletId::get().into_account_truncating()
        }

        fn per_shard_escrow(
            params: &DealParams<BalanceOf<T>>,
        ) -> Result<BalanceOf<T>, Error<T>> {
            let count: BalanceOf<T> = params.duration_epochs.into();
            params.price_per_shard_epoch.checked_mul(&count).ok_or(Error::<T>::Overflow)
        }

        /// Highest epoch (inclusive) a shard may be paid through, given the deal
        /// end and any fault cutoff.
        fn vesting_upper(
            deal: &Deal<T::AccountId, BalanceOf<T>>,
            a: &Assignment<T::AccountId>,
            current: EpochIndex,
        ) -> EpochIndex {
            let mut upper = current.saturating_sub(1).min(deal.end_epoch);
            if a.state == ShardState::Faulted {
                upper = upper.min(a.fault_epoch.saturating_sub(1));
            }
            upper
        }

        /// Drain every assignment for a deal (bounded by `n ≤ 64`), releasing the
        /// provider's locked escrow, and clear the `DealProviders` index.
        fn release_assignments(deal_id: DealId, per: BalanceOf<T>) {
            let assignments: Vec<(ShardIndex, Assignment<T::AccountId>)> =
                ShardAssignments::<T>::drain_prefix(deal_id).collect();
            for (_idx, a) in assignments {
                T::Providers::note_assignment_ended(&a.provider, per);
            }
            let _ = DealProviders::<T>::clear_prefix(deal_id, u32::MAX, None);
        }
    }

    // ── DealInspector (read side, consumed by storage-proofs) ───────────────
    impl<T: Config> DealInspector<T::AccountId> for Pallet<T> {
        fn proof_context(deal_id: DealId, shard: ShardIndex) -> Option<ProofContext<T::AccountId>> {
            let deal = Deals::<T>::get(deal_id)?;
            if !matches!(deal.state, DealState::Active | DealState::Degraded) {
                return None;
            }
            let a = ShardAssignments::<T>::get(deal_id, shard)?;
            if a.state != ShardState::Activated {
                return None;
            }
            let s = deal.params.shards.get(shard as usize)?.clone();
            Some(ProofContext {
                provider: a.provider,
                client: deal.client,
                shard_root: s.root,
                shard_size: s.size,
                challenge_rate_ppm: deal.params.challenge_rate_ppm,
                end_epoch: deal.end_epoch,
            })
        }
    }

    // ── DealFaultHandler (fault side, consumed by storage-proofs) ───────────
    impl<T: Config> DealFaultHandler<T::AccountId, BalanceOf<T>> for Pallet<T> {
        fn deals_account() -> T::AccountId {
            Self::account_id()
        }

        fn on_shard_faulted(
            deal_id: DealId,
            shard: ShardIndex,
            provider: &T::AccountId,
            repair_bounty: BalanceOf<T>,
        ) {
            let current = T::Epochs::current_epoch();
            let per = match Deals::<T>::get(deal_id) {
                Some(d) => Self::per_shard_escrow(&d.params).unwrap_or_else(|_| Zero::zero()),
                None => return,
            };
            let transition = Deals::<T>::mutate(deal_id, |maybe| -> Option<Event<T>> {
                let deal = maybe.as_mut()?;
                let was = deal.state.clone();
                deal.faulted_count = deal.faulted_count.saturating_add(1);
                deal.activated_count = deal.activated_count.saturating_sub(1);
                deal.repair_pot = deal.repair_pot.saturating_add(repair_bounty);
                ShardAssignments::<T>::mutate(deal_id, shard, |slot| {
                    if let Some(s) = slot {
                        s.state = ShardState::Faulted;
                        s.fault_epoch = current;
                    }
                });
                let survivors = (deal.params.n as u16).saturating_sub(deal.faulted_count);
                if survivors < deal.params.k as u16 {
                    deal.state = DealState::Lost;
                    Some(Event::DealLost { deal_id })
                } else if was == DealState::Active {
                    deal.state = DealState::Degraded;
                    Some(Event::DealDegraded { deal_id })
                } else {
                    None
                }
            });
            DealProviders::<T>::remove(deal_id, provider);
            T::Providers::note_assignment_ended(provider, per);
            T::Providers::note_fault(provider);
            Self::deposit_event(Event::ShardFaulted {
                deal_id,
                shard_idx: shard,
                provider: provider.clone(),
            });
            if let Some(ev) = transition {
                Self::deposit_event(ev);
            }
        }
    }
}
