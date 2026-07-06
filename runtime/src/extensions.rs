//! `CheckFalconSignature` — registry-mode signature checking (architecture guide §2.2).
//!
//! Bootstrap-mode (`ArgonautSignature::FalconWithKey`) is checked the classic way, via
//! `Verify::verify` on the `Signed` extrinsic preamble — see `primitives/falcon`. That
//! path needs no extension involvement at all, so this extension is `Disabled` for it.
//!
//! Registry-mode carries no inline public key, and `General`-preamble extrinsics (the
//! only way to submit a transaction with no `Verify`-checked signature at all) carry no
//! `Address`/`Signature` in the preamble either — so the signature and the claimed
//! account must live *inside the extension itself*. This mirrors polkadot-sdk's own
//! `pallet-verify-signature` (`substrate/frame/verify-signature/src/extension.rs`),
//! which is the reference pattern this implementation follows.
//!
//! Must run first in the runtime's extension tuple: for a `General` extrinsic, the
//! origin starts as `RawOrigin::None`, and every extension after this one (`CheckNonce`,
//! `CheckNonZeroSender`, ...) expects an already-authorized signer.

use crate::{AccountId, Runtime, RuntimeCall, RuntimeOrigin};
use argonaut_primitives_falcon::{blake3_hash, falcon_crypto, FalconSignature};
use frame_support::{traits::OriginTrait, weights::Weight};
use parity_scale_codec::{Decode, Encode};
use pallet_falcon_registry::KeyProvider;
use scale_info::TypeInfo;
use sp_runtime::{
    impl_tx_ext_default,
    traits::{AsTransactionAuthorizedOrigin, Implication, TransactionExtension, ValidateResult},
    transaction_validity::{InvalidTransaction, TransactionSource, ValidTransaction},
};

/// Registry-mode signature check for `General` (unsigned-with-extension) extrinsics.
#[derive(Encode, Decode, Clone, Eq, PartialEq, TypeInfo, Debug)]
pub enum CheckFalconSignature {
    /// Registry-mode transaction: signature + the claimed signer, resolved against
    /// `pallet-falcon-registry` storage (no inline public key on the wire).
    RegistryMode { sig: FalconSignature, account: AccountId },
    /// No-op: used for `Signed`-preamble (bootstrap-mode) extrinsics, whose signature
    /// was already checked via `Verify::verify` before the extension pipeline runs.
    Disabled,
}

impl CheckFalconSignature {
    pub fn registry_mode(sig: FalconSignature, account: AccountId) -> Self {
        Self::RegistryMode { sig, account }
    }

    pub fn disabled() -> Self {
        Self::Disabled
    }
}

impl TransactionExtension<RuntimeCall> for CheckFalconSignature {
    const IDENTIFIER: &'static str = "CheckFalconSignature";
    type Implicit = ();
    type Val = ();
    type Pre = ();

    fn weight(&self, _call: &RuntimeCall) -> Weight {
        match self {
            // One storage read (registry lookup) + one host-fn signature verify.
            Self::RegistryMode { .. } => Weight::from_parts(100_000, 0),
            Self::Disabled => Weight::zero(),
        }
    }

    fn validate(
        &self,
        mut origin: RuntimeOrigin,
        _call: &RuntimeCall,
        _info: &sp_runtime::traits::DispatchInfoOf<RuntimeCall>,
        _len: usize,
        _self_implicit: Self::Implicit,
        inherited_implication: &impl Implication,
        _source: TransactionSource,
    ) -> ValidateResult<Self::Val, RuntimeCall> {
        let (sig, account) = match self {
            Self::Disabled => return Ok((Default::default(), (), origin)),
            Self::RegistryMode { sig, account } => (sig, account),
        };

        // Defensive: this extension authorizes the origin itself, so nothing earlier
        // in the pipeline should have authorized it already.
        if origin.is_transaction_authorized() {
            return Err(InvalidTransaction::BadSigner.into());
        }

        let pk = <pallet_falcon_registry::Pallet<Runtime> as KeyProvider<AccountId>>::key_of(
            account,
        )
        .ok_or(InvalidTransaction::BadSigner)?;

        let msg_hash = inherited_implication.using_encoded(blake3_hash);
        if !falcon_crypto::verify(&sig.0, &msg_hash, &pk.0) {
            return Err(InvalidTransaction::BadProof.into());
        }

        origin.set_caller_from_signed(account.clone());
        Ok((ValidTransaction::default(), (), origin))
    }

    impl_tx_ext_default!(RuntimeCall; prepare);
}
