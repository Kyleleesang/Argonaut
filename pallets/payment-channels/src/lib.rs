//! Pallet: unidirectional Falcon-signed micropayment channels (spec §D.7).
//!
//! A client `open`s a channel to a provider, reserving a deposit. Off-chain the
//! client streams monotone-cumulative [`Voucher`]s (each a Falcon signature over
//! `blake3(DS_VOUCHER ++ scale(Voucher))`) as it consumes retrieval bandwidth.
//! Only the freshest voucher ever needs to hit the chain: the provider calls
//! `settle` to sweep `cumulative − claimed` from the client's reserve. Because
//! vouchers are monotone, no challenge game is needed (deviation-free vs §9.7).
//!
//! The ~666-byte Falcon voucher signatures live entirely off-chain; a single
//! on-chain `settle` finalizes the total.
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
    use argonaut_primitives_falcon::{falcon_crypto, FalconPublic, FalconSignature};
    use argonaut_primitives_storage::{voucher_signing_payload, ChannelId, Voucher};
    use frame_support::{
        pallet_prelude::*,
        traits::{BalanceStatus, Currency, ReservableCurrency},
    };
    use frame_system::pallet_prelude::*;
    use pallet_falcon_registry::KeyProvider;
    use sp_runtime::traits::{Saturating, Zero};

    pub type BalanceOf<T> =
        <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config {
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
        type Currency: ReservableCurrency<Self::AccountId>;
        /// Registry-mode public-key lookup (`pallet-falcon-registry`).
        type KeyProvider: KeyProvider<Self::AccountId>;
        #[pallet::constant]
        type MinDeposit: Get<BalanceOf<Self>>;
    }

    #[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, PartialEq, Eq, Debug)]
    pub struct Channel<AccountId, Balance, BlockNumber> {
        pub from: AccountId, // client (payer)
        pub to: AccountId,   // provider (payee)
        pub deposit: Balance,
        pub claimed: Balance, // cumulative already settled
        pub expiry: BlockNumber,
    }

    #[pallet::storage]
    #[pallet::getter(fn channels)]
    pub type Channels<T: Config> = StorageMap<
        _,
        Blake2_128Concat,
        ChannelId,
        Channel<T::AccountId, BalanceOf<T>, BlockNumberFor<T>>,
        OptionQuery,
    >;

    #[pallet::storage]
    #[pallet::getter(fn next_channel_id)]
    pub type NextChannelId<T: Config> = StorageValue<_, ChannelId, ValueQuery>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        ChannelOpened { channel_id: ChannelId, from: T::AccountId, to: T::AccountId },
        ChannelSettled { channel_id: ChannelId, paid: BalanceOf<T> },
        ChannelClosed { channel_id: ChannelId },
    }

    #[pallet::error]
    pub enum Error<T> {
        DepositTooLow,
        InsufficientBalance,
        ChannelNotFound,
        /// Caller is not the channel payee (`to`).
        NotPayee,
        /// Caller is neither party to the channel.
        NotParty,
        /// `blake3(pk) != channel.from`.
        KeyMismatch,
        /// No inline key and none registered for the payer.
        NoKey,
        BadSignature,
        /// `cumulative` not strictly greater than `claimed`, or exceeds deposit.
        BadVoucher,
        /// Voucher's `channel_id` does not match.
        WrongChannel,
        Overflow,
        /// `from` tried to close before expiry.
        NotExpired,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Open a channel to `to`, reserving `deposit` (`≥ MinDeposit`).
        #[pallet::call_index(0)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn open(
            origin: OriginFor<T>,
            to: T::AccountId,
            deposit: BalanceOf<T>,
            ttl_blocks: BlockNumberFor<T>,
        ) -> DispatchResult {
            let from = ensure_signed(origin)?;
            ensure!(deposit >= T::MinDeposit::get(), Error::<T>::DepositTooLow);
            T::Currency::reserve(&from, deposit).map_err(|_| Error::<T>::InsufficientBalance)?;

            let channel_id = NextChannelId::<T>::get();
            NextChannelId::<T>::put(channel_id.saturating_add(1));
            let expiry =
                frame_system::Pallet::<T>::block_number().saturating_add(ttl_blocks);
            Channels::<T>::insert(
                channel_id,
                Channel {
                    from: from.clone(),
                    to: to.clone(),
                    deposit,
                    claimed: Zero::zero(),
                    expiry,
                },
            );
            Self::deposit_event(Event::ChannelOpened { channel_id, from, to });
            Ok(())
        }

        /// Settle a voucher: sweep `cumulative − claimed` from the client's reserve
        /// to the provider. Callable repeatedly with increasing `cumulative`.
        #[pallet::call_index(1)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn settle(
            origin: OriginFor<T>,
            channel_id: ChannelId,
            voucher: Voucher,
            sig: FalconSignature,
            pk: Option<FalconPublic>,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let mut channel =
                Channels::<T>::get(channel_id).ok_or(Error::<T>::ChannelNotFound)?;
            ensure!(who == channel.to, Error::<T>::NotPayee);
            ensure!(voucher.channel_id == channel_id, Error::<T>::WrongChannel);

            // Resolve the client's Falcon key (inline in bootstrap mode, else registry).
            let pk = match pk {
                Some(pk) => {
                    let pk_hash = falcon_crypto::pk_hash(&pk.0);
                    ensure!(
                        pk_hash.as_slice() == channel.from.encode().as_slice(),
                        Error::<T>::KeyMismatch
                    );
                    pk
                }
                None => T::KeyProvider::key_of(&channel.from).ok_or(Error::<T>::NoKey)?,
            };
            let payload = voucher_signing_payload(&voucher);
            ensure!(
                falcon_crypto::verify(&sig.0, &payload, &pk.0),
                Error::<T>::BadSignature
            );

            let cumulative: BalanceOf<T> =
                voucher.cumulative.try_into().map_err(|_| Error::<T>::Overflow)?;
            ensure!(
                cumulative > channel.claimed && cumulative <= channel.deposit,
                Error::<T>::BadVoucher
            );
            let owed = cumulative.saturating_sub(channel.claimed);
            let _ = T::Currency::repatriate_reserved(
                &channel.from,
                &channel.to,
                owed,
                BalanceStatus::Free,
            );
            channel.claimed = cumulative;
            Channels::<T>::insert(channel_id, channel);
            Self::deposit_event(Event::ChannelSettled { channel_id, paid: owed });
            Ok(())
        }

        /// Close a channel: refund `deposit − claimed` to the client. The provider
        /// (`to`) may close any time; the client (`from`) only after `expiry`.
        #[pallet::call_index(2)]
        #[pallet::weight(Weight::from_parts(10_000, 0))]
        pub fn close(origin: OriginFor<T>, channel_id: ChannelId) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let channel = Channels::<T>::get(channel_id).ok_or(Error::<T>::ChannelNotFound)?;
            if who == channel.to {
                // ok any time
            } else if who == channel.from {
                ensure!(
                    frame_system::Pallet::<T>::block_number() >= channel.expiry,
                    Error::<T>::NotExpired
                );
            } else {
                return Err(Error::<T>::NotParty.into());
            }
            let refund = channel.deposit.saturating_sub(channel.claimed);
            T::Currency::unreserve(&channel.from, refund);
            Channels::<T>::remove(channel_id);
            Self::deposit_event(Event::ChannelClosed { channel_id });
            Ok(())
        }
    }
}
