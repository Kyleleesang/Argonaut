//! Pallet: storage-provider registry (Implementation D, spec §D.4).
//!
//! Providers stake `ReservableCurrency` to advertise capacity. Stake backs the
//! §9.3 2× rule: a provider may hold an assignment only while
//! `stake ≥ 2 · Σ per-shard escrow` across its active assignments. That running
//! sum is tracked in [`StakeLocked`] and enforced by
//! [`ProviderInterface::note_assignment_started`].
//!
//! This pallet owns no deal/proof logic; it exposes the [`ProviderInterface`]
//! trait (defined in `argonaut-primitives-storage`) that `storage-deals` and
//! `storage-proofs` consume via their `Config` associated types. Keeping the
//! trait in the neutral primitives crate is what lets those two pallets refer to
//! each other's contracts without a cyclic crate dependency.
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
    use argonaut_primitives_storage::interfaces::ProviderInterface;
    use frame_support::{
        pallet_prelude::*,
        traits::{BalanceStatus, Currency, ReservableCurrency},
    };
    use frame_system::pallet_prelude::*;
    use sp_runtime::traits::{CheckedAdd, Saturating, Zero};

    pub type BalanceOf<T> =
        <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config {
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
        type Currency: ReservableCurrency<Self::AccountId>;
        /// Minimum stake to register (spec D.8 devnet default: `1_000 · ED`).
        #[pallet::constant]
        type MinStake: Get<BalanceOf<Self>>;
    }

    /// Provider lifecycle. `Deregistering` is reserved for a future unbonding
    /// path; v1 deregisters synchronously once `active_assignments == 0` (D.13.7).
    #[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, PartialEq, Eq, Debug)]
    pub enum ProviderStatus {
        Active,
        Deregistering,
    }

    #[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, PartialEq, Eq, Debug)]
    pub struct ProviderInfo<Balance> {
        /// Currently reserved stake (kept in sync with slashing).
        pub stake: Balance,
        pub iroh_node_id: [u8; 32],
        /// Opaque connection hint; the daemon interprets it.
        pub relay_hint: BoundedVec<u8, ConstU32<256>>,
        /// Advertised price; the real per-deal price is set in `DealParams`.
        pub price_per_shard_epoch: Balance,
        pub active_assignments: u32,
        /// Lifetime informational fault counter.
        pub faults: u32,
        pub status: ProviderStatus,
    }

    #[pallet::storage]
    #[pallet::getter(fn providers)]
    pub type Providers<T: Config> =
        StorageMap<_, Blake2_128Concat, T::AccountId, ProviderInfo<BalanceOf<T>>, OptionQuery>;

    /// Σ per-shard escrow across a provider's active assignments — the quantity
    /// the §9.3 2× rule is checked against. `ValueQuery` (default 0).
    #[pallet::storage]
    #[pallet::getter(fn stake_locked)]
    pub type StakeLocked<T: Config> =
        StorageMap<_, Blake2_128Concat, T::AccountId, BalanceOf<T>, ValueQuery>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        Registered { who: T::AccountId },
        StakeToppedUp { who: T::AccountId, amount: BalanceOf<T> },
        TermsUpdated { who: T::AccountId },
        Deregistered { who: T::AccountId },
        Slashed { who: T::AccountId, amount: BalanceOf<T>, beneficiary: T::AccountId },
    }

    #[pallet::error]
    pub enum Error<T> {
        AlreadyRegistered,
        NotRegistered,
        StakeTooLow,
        HasActiveAssignments,
        /// Reserve failed — free balance below the requested stake.
        InsufficientBalance,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Register as a storage provider, reserving `stake` (`≥ MinStake`).
        #[pallet::call_index(0)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn register(
            origin: OriginFor<T>,
            stake: BalanceOf<T>,
            iroh_node_id: [u8; 32],
            relay_hint: BoundedVec<u8, ConstU32<256>>,
            price_per_shard_epoch: BalanceOf<T>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(!Providers::<T>::contains_key(&who), Error::<T>::AlreadyRegistered);
            ensure!(stake >= T::MinStake::get(), Error::<T>::StakeTooLow);

            T::Currency::reserve(&who, stake).map_err(|_| Error::<T>::InsufficientBalance)?;

            Providers::<T>::insert(
                &who,
                ProviderInfo {
                    stake,
                    iroh_node_id,
                    relay_hint,
                    price_per_shard_epoch,
                    active_assignments: 0,
                    faults: 0,
                    status: ProviderStatus::Active,
                },
            );
            Self::deposit_event(Event::Registered { who });
            Ok(())
        }

        /// Reserve additional stake.
        #[pallet::call_index(1)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn top_up_stake(origin: OriginFor<T>, amount: BalanceOf<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;
            Providers::<T>::try_mutate(&who, |maybe| -> DispatchResult {
                let p = maybe.as_mut().ok_or(Error::<T>::NotRegistered)?;
                T::Currency::reserve(&who, amount)
                    .map_err(|_| Error::<T>::InsufficientBalance)?;
                p.stake = p.stake.saturating_add(amount);
                Ok(())
            })?;
            Self::deposit_event(Event::StakeToppedUp { who, amount });
            Ok(())
        }

        /// Update advertised connection info and price.
        #[pallet::call_index(2)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn update_terms(
            origin: OriginFor<T>,
            iroh_node_id: [u8; 32],
            relay_hint: BoundedVec<u8, ConstU32<256>>,
            price_per_shard_epoch: BalanceOf<T>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            Providers::<T>::try_mutate(&who, |maybe| -> DispatchResult {
                let p = maybe.as_mut().ok_or(Error::<T>::NotRegistered)?;
                p.iroh_node_id = iroh_node_id;
                p.relay_hint = relay_hint;
                p.price_per_shard_epoch = price_per_shard_epoch;
                Ok(())
            })?;
            Self::deposit_event(Event::TermsUpdated { who });
            Ok(())
        }

        /// Deregister and unreserve the full remaining stake. Requires no active
        /// assignments (no unbonding period in v1 — D.13.7).
        #[pallet::call_index(3)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn deregister(origin: OriginFor<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let p = Providers::<T>::get(&who).ok_or(Error::<T>::NotRegistered)?;
            ensure!(p.active_assignments == 0, Error::<T>::HasActiveAssignments);

            T::Currency::unreserve(&who, p.stake);
            Providers::<T>::remove(&who);
            StakeLocked::<T>::remove(&who);
            Self::deposit_event(Event::Deregistered { who });
            Ok(())
        }
    }

    // ── ProviderInterface (consumed by storage-deals / storage-proofs) ──────
    impl<T: Config> ProviderInterface<T::AccountId, BalanceOf<T>> for Pallet<T> {
        fn is_active(who: &T::AccountId) -> bool {
            matches!(Providers::<T>::get(who), Some(p) if p.status == ProviderStatus::Active)
        }

        fn free_stake(who: &T::AccountId) -> BalanceOf<T> {
            match Providers::<T>::get(who) {
                Some(p) => p.stake.saturating_sub(StakeLocked::<T>::get(who)),
                None => Zero::zero(),
            }
        }

        fn slash(
            who: &T::AccountId,
            amount: BalanceOf<T>,
            beneficiary: &T::AccountId,
        ) -> BalanceOf<T> {
            if !Providers::<T>::contains_key(who) {
                return Zero::zero();
            }
            // `repatriate_reserved` returns the portion it could NOT move.
            let remainder = T::Currency::repatriate_reserved(
                who,
                beneficiary,
                amount,
                BalanceStatus::Free,
            )
            .unwrap_or(amount);
            let moved = amount.saturating_sub(remainder);
            if !moved.is_zero() {
                Providers::<T>::mutate(who, |maybe| {
                    if let Some(p) = maybe {
                        p.stake = p.stake.saturating_sub(moved);
                    }
                });
                Self::deposit_event(Event::Slashed {
                    who: who.clone(),
                    amount: moved,
                    beneficiary: beneficiary.clone(),
                });
            }
            moved
        }

        fn note_assignment_started(
            who: &T::AccountId,
            escrow_at_risk: BalanceOf<T>,
        ) -> Result<(), ()> {
            Providers::<T>::try_mutate(who, |maybe| {
                let p = maybe.as_mut().ok_or(())?;
                if p.status != ProviderStatus::Active {
                    return Err(());
                }
                let new_locked =
                    StakeLocked::<T>::get(who).checked_add(&escrow_at_risk).ok_or(())?;
                // §9.3 2× rule: stake must cover twice the locked escrow.
                let two_locked = new_locked.checked_add(&new_locked).ok_or(())?;
                if p.stake < two_locked {
                    return Err(());
                }
                StakeLocked::<T>::insert(who, new_locked);
                p.active_assignments = p.active_assignments.saturating_add(1);
                Ok(())
            })
        }

        fn note_assignment_ended(who: &T::AccountId, escrow_at_risk: BalanceOf<T>) {
            Providers::<T>::mutate(who, |maybe| {
                if let Some(p) = maybe {
                    p.active_assignments = p.active_assignments.saturating_sub(1);
                }
            });
            StakeLocked::<T>::mutate(who, |l| *l = l.saturating_sub(escrow_at_risk));
        }

        fn note_fault(who: &T::AccountId) {
            Providers::<T>::mutate(who, |maybe| {
                if let Some(p) = maybe {
                    p.faults = p.faults.saturating_add(1);
                }
            });
        }
    }
}
