//! Extrinsic construction, generalized from the two reference examples
//! (`node/examples/submit_transfer_registry.rs` for registry mode,
//! `node/src/falcon_sign.rs`'s bootstrap signing for the envelope) into one
//! shared path: `CheckFalconSignature::Bootstrap` and `::RegistryMode` are
//! both `General`-preamble, both sign
//! `(extension_version, call, tail_extra, tail_implicit).encode()` via
//! `inherited_implication` (see `runtime/src/extensions.rs`'s `validate` for
//! both variants) -- the only difference is which value goes in `extra`'s
//! first slot. Only the signature bytes differ per call; nothing about the
//! surrounding tail extensions does.

use argonaut_runtime::{self as runtime, CheckFalconSignature, RuntimeCall, SignedExtra, UncheckedExtrinsic};
use argonaut_primitives_falcon::{FalconPublic, FalconSignature};
use parity_scale_codec::Encode;
use rand_core::{CryptoRng, RngCore};
use sp_core::crypto::AccountId32;
use sp_core::H256;
use sp_runtime::generic::Era;

/// Which `CheckFalconSignature` variant signs this extrinsic. Both are
/// `General`-preamble and share the exact same tail-extension construction;
/// see the module doc.
pub enum SignMode {
    /// First transaction for a not-yet-registered key: carries the public key
    /// inline, doubles as registration if the chain has
    /// `AUTO_REGISTER_ON_BOOTSTRAP` on (runtime/src/extensions.rs).
    Bootstrap { pk: FalconPublic },
    /// Every transaction after the key is registered: cheaper (~897 fewer
    /// bytes), key resolved from `pallet_falcon_registry` storage on-chain.
    Registry { account: AccountId32 },
}

/// Everything needed to fill in `CheckEra`/`CheckNonce`/implicit data, fetched
/// from chain state by the caller (`ChainClient`) so this module stays a pure
/// builder with no RPC of its own.
pub struct SigningContext {
    pub genesis_hash: H256,
    pub checkpoint_hash: H256,
    pub checkpoint_number: u64,
    pub spec_version: u32,
    pub tx_version: u32,
    pub nonce: u32,
}

/// Build and sign a `General`-preamble extrinsic for `call`, in `mode`, using
/// `signing_key` (raw fn-dsa signing-key bytes, e.g. from a
/// `FalconKeystoreFile`). Returns the SCALE-encoded extrinsic, ready for
/// `author_submitExtrinsic`.
pub fn build_and_sign(
    call: RuntimeCall,
    mode: SignMode,
    ctx: &SigningContext,
    signing_key: &[u8],
    rng: &mut (impl RngCore + CryptoRng),
) -> Result<UncheckedExtrinsic, String> {
    let period = runtime::BlockHashCount::get()
        .checked_next_power_of_two()
        .map(|c| c / 2)
        .unwrap_or(2) as u64;
    let era = Era::mortal(period, ctx.checkpoint_number);

    // The 8 extensions after `CheckFalconSignature` -- identical for both
    // sign modes (see module doc).
    let tail_extra = (
        frame_system::CheckNonZeroSender::<runtime::Runtime>::new(),
        frame_system::CheckSpecVersion::<runtime::Runtime>::new(),
        frame_system::CheckTxVersion::<runtime::Runtime>::new(),
        frame_system::CheckGenesis::<runtime::Runtime>::new(),
        frame_system::CheckEra::<runtime::Runtime>::from(era),
        frame_system::CheckNonce::<runtime::Runtime>::from(ctx.nonce),
        frame_system::CheckWeight::<runtime::Runtime>::new(),
        pallet_transaction_payment::ChargeTransactionPayment::<runtime::Runtime>::from(0),
    );
    let tail_implicit = (
        (),
        ctx.spec_version,
        ctx.tx_version,
        ctx.genesis_hash,
        ctx.checkpoint_hash,
        (),
        (),
        (),
    );

    const EXTENSION_VERSION: u8 = 0; // matches `UncheckedExtrinsic::new_transaction`'s constant
    let signing_payload = (EXTENSION_VERSION, &call, &tail_extra, &tail_implicit).encode();
    let sig_bytes = argonaut_signer_core::sign_payload(signing_key, &signing_payload, rng)?;
    let sig = FalconSignature(
        sp_core::bounded::BoundedVec::try_from(sig_bytes)
            .map_err(|_| "signature exceeds the on-chain bound".to_string())?,
    );

    let falcon_ext = match mode {
        SignMode::Bootstrap { pk } => CheckFalconSignature::bootstrap(sig, pk),
        SignMode::Registry { account } => CheckFalconSignature::registry_mode(sig, account),
    };

    let extra: SignedExtra = (
        falcon_ext,
        tail_extra.0,
        tail_extra.1,
        tail_extra.2,
        tail_extra.3,
        tail_extra.4,
        tail_extra.5,
        tail_extra.6,
        tail_extra.7,
    );

    Ok(UncheckedExtrinsic::new_transaction(call, extra))
}
