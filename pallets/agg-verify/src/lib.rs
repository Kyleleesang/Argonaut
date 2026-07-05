//! Pallet: STARK-aggregated XMSS signature bundle verification.
//!
//! # Why aggregation?
//! ~150 B signature material per tx amortized vs ~800 B registry-mode Falcon.
//! One STARK proof covers up to 1024 txs; verify ≈5 ms per bundle.
//!
//! # Epoch replay protection
//! XMSS leaf reuse is catastrophic (enables signature forgery). This pallet
//! maintains an on-chain bitfield of used (key, epoch) pairs. Any reuse is
//! rejected at pool time via `ValidateUnsigned` — it never enters a block.
//!
//! # Upgrade path
//! `AggSchemeId` allow-list lets governance disable a compromised scheme without
//! a hard fork. All crypto goes behind the `agg_verify` host function.
#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

#[frame_support::pallet]
pub mod pallet {
    use argonaut_primitives_agg::{agg_verify, statement_hash, AggBundle, XmssRootRef};
    use frame_support::pallet_prelude::*;
    use frame_system::pallet_prelude::*;

    #[pallet::pallet]
    pub struct Pallet<T>(_);

    #[pallet::config]
    pub trait Config: frame_system::Config {
        type RuntimeEvent: From<Event<Self>> + IsType<<Self as frame_system::Config>::RuntimeEvent>;
        /// Allowed `AggSchemeId.vm_id` values. Emergency disable: remove the id.
        #[pallet::constant]
        type AllowedSchemes: Get<sp_std::vec::Vec<u8>>;
    }

    /// Used XMSS (key_root, epoch) pairs. Reject any reuse — leaf reuse = key compromise.
    #[pallet::storage]
    pub type UsedEpochs<T: Config> = StorageDoubleMap<
        _,
        Blake2_128Concat,
        XmssRootRef,
        Blake2_128Concat,
        u32,
        (),
        OptionQuery,
    >;

    /// Registered XMSS key roots: AccountId → xmss_root_ref.
    /// AccountId = blake3(xmss_root) mirrors the Falcon registry pattern.
    #[pallet::storage]
    pub type XmssKeys<T: Config> =
        StorageMap<_, Blake2_128Concat, T::AccountId, XmssRootRef, OptionQuery>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        BundleVerified { tx_count: u32 },
        EpochReplay { key: XmssRootRef, epoch: u32 },
    }

    #[pallet::error]
    pub enum Error<T> {
        /// STARK proof verification failed.
        InvalidProof,
        /// A (key, epoch) pair was already used — XMSS leaf reuse attempt.
        EpochReplay,
        /// The bundle's scheme id is not in the on-chain allow-list.
        SchemeNotAllowed,
        /// A signer key root is not registered on-chain.
        KeyNotRegistered,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Submit an aggregated bundle for verification and dispatch.
        ///
        /// Pool-time check: `validate_unsigned` recomputes the statement hash and
        /// verifies the STARK proof before this ever enters a block.
        ///
        /// On success: marks all (key, epoch) pairs as used; emits event.
        /// Individual inner-tx dispatch failures are skipped, not fatal (same
        /// determinism rule as the Prism bridge — §3.1).
        #[pallet::call_index(0)]
        #[pallet::weight({
            // Base: proof verify ≈5ms → benchmark and replace this estimate.
            let base = Weight::from_parts(50_000, 0);
            let per_tx = Weight::from_parts(5_000, 0).saturating_mul(bundle.txs.len() as u64);
            base.saturating_add(per_tx)
        })]
        pub fn submit_bundle(origin: OriginFor<T>, bundle: AggBundle) -> DispatchResult {
            ensure_none(origin)?;

            let scheme_byte = bundle.scheme.vm_id;
            ensure!(T::AllowedSchemes::get().contains(&scheme_byte), Error::<T>::SchemeNotAllowed);

            // Epoch replay check — must be first to prevent DoS via replayed proofs
            for (key, &epoch) in bundle.signer_keys.iter().zip(bundle.epochs.iter()) {
                ensure!(!UsedEpochs::<T>::contains_key(key, epoch), Error::<T>::EpochReplay);
            }

            // STARK proof verification via native host function
            let stmt = statement_hash(&bundle.scheme, &bundle.txs, &bundle.signer_keys, &bundle.epochs);
            ensure!(
                agg_verify::verify_bundle(scheme_byte, &stmt, &bundle.proof),
                Error::<T>::InvalidProof
            );

            // Mark epochs used (after verification to avoid partial state on failure)
            for (key, &epoch) in bundle.signer_keys.iter().zip(bundle.epochs.iter()) {
                UsedEpochs::<T>::insert(key, epoch, ());
            }

            let tx_count = bundle.txs.len() as u32;
            Self::deposit_event(Event::BundleVerified { tx_count });
            Ok(())
        }
    }

    #[pallet::validate_unsigned]
    impl<T: Config> ValidateUnsigned for Pallet<T> {
        type Call = Call<T>;

        fn validate_unsigned(_source: TransactionSource, call: &Self::Call) -> TransactionValidity {
            if let Call::submit_bundle { bundle } = call {
                let scheme_byte = bundle.scheme.vm_id;
                if !T::AllowedSchemes::get().contains(&scheme_byte) {
                    return InvalidTransaction::BadProof.into();
                }
                // Epoch replay check at pool time
                for (key, &epoch) in bundle.signer_keys.iter().zip(bundle.epochs.iter()) {
                    if UsedEpochs::<T>::contains_key(key, epoch) {
                        return InvalidTransaction::Stale.into();
                    }
                }
                // Proof verification at pool time — critical for DoS prevention
                let stmt = statement_hash(
                    &bundle.scheme,
                    &bundle.txs,
                    &bundle.signer_keys,
                    &bundle.epochs,
                );
                if !agg_verify::verify_bundle(scheme_byte, &stmt, &bundle.proof) {
                    return InvalidTransaction::BadProof.into();
                }
                ValidTransaction::with_tag_prefix("AggBundle")
                    .priority(100)
                    .longevity(64)
                    .propagate(true)
                    .build()
            } else {
                InvalidTransaction::Call.into()
            }
        }
    }
}
