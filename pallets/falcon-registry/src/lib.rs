//! Pallet: on-chain Falcon-512 public-key registry.
//!
//! # Bootstrap vs Registry mode
//! - Bootstrap: tx carries (signature + full public key); runtime checks
//!   `blake3(pk) == signer` then verifies. ~1.7 KB per tx.
//! - Registry: tx carries only (signature); this pallet provides the pk lookup.
//!   ~897 bytes saved per tx.
//!
//! Registration requires a storage deposit (`KeyDeposit`) to deter state spam.
//! Deregistration refunds the deposit.
//!
//! The `KeyProvider` trait is consumed by `CheckFalconSignature` TransactionExtension.
#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

#[cfg(feature = "runtime-benchmarks")]
mod benchmarking;
#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

use argonaut_primitives_falcon::FalconPublic;

/// Public-key lookup consumed by the `CheckFalconSignature` TransactionExtension
/// (runtime crate) to resolve registry-mode signatures, which carry no inline key.
pub trait KeyProvider<AccountId> {
    fn key_of(who: &AccountId) -> Option<FalconPublic>;
}

impl<T: pallet::Config> KeyProvider<T::AccountId> for pallet::Pallet<T> {
    fn key_of(who: &T::AccountId) -> Option<FalconPublic> {
        pallet::Pallet::<T>::key_of(who)
    }
}

#[frame_support::pallet]
pub mod pallet {
    use argonaut_primitives_falcon::{falcon_crypto, FalconPublic};
    use frame_support::{
        pallet_prelude::*,
        traits::{Currency, ReservableCurrency},
    };
    use frame_system::pallet_prelude::*;

    type BalanceOf<T> =
        <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config {
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
        type Currency: ReservableCurrency<Self::AccountId>;
        /// Storage deposit for registering a Falcon public key (~900 bytes of on-chain state).
        /// Refundable on deregistration.
        #[pallet::constant]
        type KeyDeposit: Get<BalanceOf<Self>>;
    }

    /// Maps AccountId → registered Falcon-512 public key.
    #[pallet::storage]
    #[pallet::getter(fn keys)]
    pub type Keys<T: Config> =
        StorageMap<_, Blake2_128Concat, T::AccountId, FalconPublic, OptionQuery>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        KeyRegistered { who: T::AccountId },
        KeyDeregistered { who: T::AccountId },
    }

    #[pallet::error]
    pub enum Error<T> {
        /// Provided pk's blake3 hash does not equal the caller's AccountId.
        KeyMismatch,
        /// Caller already has a registered key.
        AlreadyRegistered,
        /// Caller has no registered key to deregister.
        NotRegistered,
        /// Insufficient balance to cover the storage deposit.
        InsufficientBalance,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Register the caller's Falcon-512 public key.
        ///
        /// Origin must be signed in bootstrap mode (the enclosing tx signature already
        /// proved key ownership via `blake3(pk) == account_id` check).
        #[pallet::call_index(0)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn register_key(origin: OriginFor<T>, pk: FalconPublic) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(!Keys::<T>::contains_key(&who), Error::<T>::AlreadyRegistered);

            // Verify the pk's hash matches the signer's account id.
            let pk_hash = falcon_crypto::pk_hash(&pk.0);
            let account_bytes = who.encode();
            ensure!(pk_hash.as_slice() == account_bytes.as_slice(), Error::<T>::KeyMismatch);

            T::Currency::reserve(&who, T::KeyDeposit::get())
                .map_err(|_| Error::<T>::InsufficientBalance)?;

            Keys::<T>::insert(&who, pk);
            Self::deposit_event(Event::KeyRegistered { who });
            Ok(())
        }

        /// Deregister the caller's key and refund the storage deposit.
        #[pallet::call_index(1)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn deregister_key(origin: OriginFor<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;
            ensure!(Keys::<T>::contains_key(&who), Error::<T>::NotRegistered);

            Keys::<T>::remove(&who);
            T::Currency::unreserve(&who, T::KeyDeposit::get());
            Self::deposit_event(Event::KeyDeregistered { who });
            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        /// Look up a registered public key by account id.
        ///
        /// Called by `CheckFalconSignature` TransactionExtension to resolve
        /// registry-mode signatures (`ArgonautSignature::Falcon { sig }`).
        pub fn key_of(who: &T::AccountId) -> Option<FalconPublic> {
            Keys::<T>::get(who)
        }
    }
}
