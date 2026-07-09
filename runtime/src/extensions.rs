//! `CheckFalconSignature` — registry-mode & bootstrap-mode signature checking
//! (architecture guide §2.2 / §2.3).
//!
//! Three modes, all resolved before the rest of the extension pipeline runs:
//!
//! - **`RegistryMode`** — `General`-preamble tx carrying only a signature + the
//!   claimed 32-byte account; the public key is looked up in
//!   `pallet-falcon-registry` (~897 bytes saved per tx).
//! - **`Bootstrap`** — `General`-preamble tx carrying an *inline* public key +
//!   signature; verified directly (no registry lookup) and, when the
//!   [`AUTO_REGISTER_ON_BOOTSTRAP`] config gate is on, the key is registered in
//!   `prepare` so the first transaction doubles as registration (§2.3). This is
//!   the extension-side counterpart to the classic `Signed`-preamble
//!   `ArgonautSignature::FalconWithKey` path, which stays available and is left
//!   untouched (that path uses [`CheckFalconSignature::Disabled`]).
//! - **`Disabled`** — no-op, for `Signed`-preamble (classic bootstrap) extrinsics
//!   whose signature was already checked via `Verify::verify`.
//!
//! Registry- and bootstrap-mode carry no `Verify`-checked preamble signature, so
//! the signature and claimed account live *inside the extension itself*. This
//! mirrors polkadot-sdk's own `pallet-verify-signature`
//! (`substrate/frame/verify-signature/src/extension.rs`).
//!
//! Must run first in the runtime's extension tuple: for a `General` extrinsic the
//! origin starts as `RawOrigin::None`, and every extension after this one
//! (`CheckNonce`, `CheckNonZeroSender`, …) expects an already-authorized signer.

use crate::{AccountId, Runtime, RuntimeCall, RuntimeOrigin};
use argonaut_primitives_falcon::{blake3_hash, falcon_crypto, FalconPublic, FalconSignature};
use frame_support::{traits::OriginTrait, weights::Weight};
use parity_scale_codec::{Decode, Encode};
use pallet_falcon_registry::KeyProvider;
use scale_info::TypeInfo;
use sp_runtime::{
    traits::{
        AsTransactionAuthorizedOrigin, DispatchInfoOf, Implication, TransactionExtension,
        ValidateResult,
    },
    transaction_validity::{InvalidTransaction, TransactionSource, TransactionValidityError, ValidTransaction},
};

/// Config gate for §2.3's optional auto-registration. When `true`, a
/// [`CheckFalconSignature::Bootstrap`] transaction's inline key is registered
/// (best-effort) in `prepare`, so a user's very first transaction also registers
/// their key and every subsequent one can use the cheaper registry mode. Flip to
/// `false` to make `Bootstrap` verify-and-authorize only, leaving registration to
/// an explicit `register_key` call. Enabled on this devnet; a production chain
/// sets it per policy. (A compile-time const is the whole config surface — a
/// boolean policy needs no pallet plumbing.)
pub const AUTO_REGISTER_ON_BOOTSTRAP: bool = true;

/// Registry-/bootstrap-mode signature check for `General` (unsigned-with-extension)
/// extrinsics.
#[derive(Encode, Decode, Clone, Eq, PartialEq, TypeInfo, Debug)]
pub enum CheckFalconSignature {
    /// Registry-mode: signature + claimed signer, key resolved from
    /// `pallet-falcon-registry` storage (no inline public key on the wire).
    RegistryMode { sig: FalconSignature, account: AccountId },
    /// No-op: used for `Signed`-preamble (classic bootstrap) extrinsics, whose
    /// signature was already checked via `Verify::verify`.
    Disabled,
    /// Bootstrap-mode: signature + inline public key, verified directly. Doubles
    /// as registration when [`AUTO_REGISTER_ON_BOOTSTRAP`] is on. **Appended last**
    /// so existing `RegistryMode`(0)/`Disabled`(1) SCALE indices never move.
    Bootstrap { sig: FalconSignature, pk: FalconPublic },
}

impl CheckFalconSignature {
    pub fn registry_mode(sig: FalconSignature, account: AccountId) -> Self {
        Self::RegistryMode { sig, account }
    }

    pub fn disabled() -> Self {
        Self::Disabled
    }

    pub fn bootstrap(sig: FalconSignature, pk: FalconPublic) -> Self {
        Self::Bootstrap { sig, pk }
    }
}

impl TransactionExtension<RuntimeCall> for CheckFalconSignature {
    const IDENTIFIER: &'static str = "CheckFalconSignature";
    type Implicit = ();
    /// `Some((account, pk))` when a bootstrap key is a candidate for
    /// auto-registration in `prepare`; `None` otherwise.
    type Val = Option<(AccountId, FalconPublic)>;
    type Pre = ();

    fn weight(&self, _call: &RuntimeCall) -> Weight {
        match self {
            // One storage read (registry lookup) + one host-fn signature verify.
            Self::RegistryMode { .. } => Weight::from_parts(100_000, 0),
            // One host-fn verify + a possible registry write (reserve + insert).
            Self::Bootstrap { .. } => Weight::from_parts(200_000, 0),
            Self::Disabled => Weight::zero(),
        }
    }

    fn validate(
        &self,
        mut origin: RuntimeOrigin,
        _call: &RuntimeCall,
        _info: &DispatchInfoOf<RuntimeCall>,
        _len: usize,
        _self_implicit: Self::Implicit,
        inherited_implication: &impl Implication,
        _source: TransactionSource,
    ) -> ValidateResult<Self::Val, RuntimeCall> {
        match self {
            // Bootstrap-mode signature was verified via the preamble; pass through.
            Self::Disabled => Ok((ValidTransaction::default(), None, origin)),

            Self::RegistryMode { sig, account } => {
                // This extension authorizes the origin itself; nothing earlier may have.
                if origin.is_transaction_authorized() {
                    return Err(InvalidTransaction::BadSigner.into());
                }
                let pk =
                    <pallet_falcon_registry::Pallet<Runtime> as KeyProvider<AccountId>>::key_of(
                        account,
                    )
                    .ok_or(InvalidTransaction::BadSigner)?;
                let msg_hash = inherited_implication.using_encoded(blake3_hash);
                if !falcon_crypto::verify(&sig.0, &msg_hash, &pk.0) {
                    return Err(InvalidTransaction::BadProof.into());
                }
                origin.set_caller_from_signed(account.clone());
                Ok((ValidTransaction::default(), None, origin))
            }

            Self::Bootstrap { sig, pk } => {
                if origin.is_transaction_authorized() {
                    return Err(InvalidTransaction::BadSigner.into());
                }
                // The account is `blake3(pk)`; the inline key proves ownership itself
                // (no registry lookup needed).
                let account = AccountId::from(falcon_crypto::pk_hash(&pk.0));
                let msg_hash = inherited_implication.using_encoded(blake3_hash);
                if !falcon_crypto::verify(&sig.0, &msg_hash, &pk.0) {
                    return Err(InvalidTransaction::BadProof.into());
                }
                origin.set_caller_from_signed(account.clone());
                Ok((ValidTransaction::default(), Some((account, pk.clone())), origin))
            }
        }
    }

    fn prepare(
        self,
        val: Self::Val,
        _origin: &RuntimeOrigin,
        _call: &RuntimeCall,
        _info: &DispatchInfoOf<RuntimeCall>,
        _len: usize,
    ) -> Result<Self::Pre, TransactionValidityError> {
        // §2.3 auto-registration: the first bootstrap tx doubles as registration.
        // Runs only in block execution (not pool validation), so the write persists
        // exactly when the extrinsic is applied. Best-effort — if the key is already
        // registered or the account can't cover the deposit, the underlying call
        // still dispatches (registration is a convenience, not a precondition).
        if AUTO_REGISTER_ON_BOOTSTRAP {
            if let Some((account, pk)) = val {
                let _ = pallet_falcon_registry::Pallet::<Runtime>::register_key(
                    RuntimeOrigin::signed(account),
                    pk,
                );
            }
        }
        Ok(())
    }
}
