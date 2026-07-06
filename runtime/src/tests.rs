//! Integration tests for `CheckFalconSignature` (registry-mode signature checking).
use crate::{
    extensions::CheckFalconSignature, AccountId, Balances, FalconRegistry, RuntimeCall,
    RuntimeGenesisConfig, RuntimeOrigin,
};
use argonaut_primitives_falcon::{blake3_hash, FalconPublic, FalconSignature};
use frame_support::{assert_ok, dispatch::DispatchInfo, traits::Currency, BoundedVec};
use parity_scale_codec::Encode;
use sp_runtime::{
    traits::{AsSystemOriginSigner, TransactionExtension, TxBaseImplication},
    transaction_validity::{InvalidTransaction, TransactionSource, TransactionValidityError},
    BuildStorage,
};

fn new_test_ext() -> sp_io::TestExternalities {
    RuntimeGenesisConfig::default().build_storage().unwrap().into()
}

/// Real Falcon-512 keypair via `fn-dsa` (dev-dependency).
fn keypair(seed: u8) -> (Vec<u8>, Vec<u8>) {
    use fn_dsa::{sign_key_size, vrfy_key_size, KeyPairGenerator, KeyPairGeneratorStandard, FN_DSA_LOGN_512};
    use rand_core::SeedableRng;
    let mut rng = rand_chacha::ChaCha20Rng::from_seed([seed; 32]);
    let mut kg = KeyPairGeneratorStandard::default();
    let mut sign_key = vec![0u8; sign_key_size(FN_DSA_LOGN_512)];
    let mut vrfy_key = vec![0u8; vrfy_key_size(FN_DSA_LOGN_512)];
    kg.keygen(FN_DSA_LOGN_512, &mut rng, &mut sign_key, &mut vrfy_key);
    (sign_key, vrfy_key)
}

fn sign(sign_key: &[u8], seed: u8, msg: &[u8]) -> Vec<u8> {
    use fn_dsa::{signature_size, SigningKey, SigningKeyStandard, DOMAIN_NONE, HASH_ID_RAW};
    use rand_core::SeedableRng;
    let mut rng = rand_chacha::ChaCha20Rng::from_seed([seed; 32]);
    let mut sk = SigningKeyStandard::decode(sign_key).unwrap();
    let mut sig = vec![0u8; signature_size(sk.get_logn())];
    sk.sign(&mut rng, &DOMAIN_NONE, &HASH_ID_RAW, msg, &mut sig);
    sig
}

/// Registers `vrfy_key` under its derived AccountId (funding it first) and returns
/// that AccountId.
fn register(vrfy_key: &[u8], fund_seed: u8) -> AccountId {
    let account = AccountId::from(blake3_hash(vrfy_key));
    Balances::make_free_balance_be(&account, 10_000);
    let pk = FalconPublic(BoundedVec::try_from(vrfy_key.to_vec()).unwrap());
    assert_ok!(FalconRegistry::register_key(RuntimeOrigin::signed(account.clone()), pk));
    let _ = fund_seed;
    account
}

fn remark_call() -> RuntimeCall {
    frame_system::Call::remark { remark: vec![1, 2, 3] }.into()
}

#[test]
fn registry_mode_accepts_valid_signature_and_authorizes_origin() {
    new_test_ext().execute_with(|| {
        let (sign_key, vrfy_key) = keypair(1);
        let account = register(&vrfy_key, 1);

        let call = remark_call();
        let msg_hash = call.using_encoded(blake3_hash);
        let sig_bytes = sign(&sign_key, 2, &msg_hash);
        let sig = FalconSignature(BoundedVec::try_from(sig_bytes).unwrap());

        let ext = CheckFalconSignature::registry_mode(sig, account.clone());
        let info = DispatchInfo::default();
        let origin = RuntimeOrigin::none();

        let (_, _, resulting_origin) = ext
            .validate(origin, &call, &info, 0, (), &TxBaseImplication(&call), TransactionSource::External)
            .expect("valid registry-mode signature must be accepted");

        assert_eq!(resulting_origin.as_system_origin_signer(), Some(&account));
    });
}

#[test]
fn registry_mode_rejects_unregistered_account() {
    new_test_ext().execute_with(|| {
        let (_sign_key, vrfy_key) = keypair(3);
        // Deliberately never registered.
        let account = AccountId::from(blake3_hash(&vrfy_key));

        let call = remark_call();
        let sig = FalconSignature(BoundedVec::try_from(vec![0u8; 666]).unwrap());
        let ext = CheckFalconSignature::registry_mode(sig, account);
        let info = DispatchInfo::default();
        let origin = RuntimeOrigin::none();

        let result = ext.validate(origin, &call, &info, 0, (), &TxBaseImplication(&call), TransactionSource::External);
        assert_eq!(
            result.unwrap_err(),
            TransactionValidityError::Invalid(InvalidTransaction::BadSigner)
        );
    });
}

#[test]
fn registry_mode_rejects_wrong_signature() {
    new_test_ext().execute_with(|| {
        let (_sign_key, vrfy_key) = keypair(4);
        let account = register(&vrfy_key, 4);

        // Sign with a *different* keypair's secret; the registered pk won't match.
        let (other_sign_key, _other_vrfy_key) = keypair(5);
        let call = remark_call();
        let msg_hash = call.using_encoded(blake3_hash);
        let sig_bytes = sign(&other_sign_key, 6, &msg_hash);
        let sig = FalconSignature(BoundedVec::try_from(sig_bytes).unwrap());

        let ext = CheckFalconSignature::registry_mode(sig, account);
        let info = DispatchInfo::default();
        let origin = RuntimeOrigin::none();

        let result = ext.validate(origin, &call, &info, 0, (), &TxBaseImplication(&call), TransactionSource::External);
        assert_eq!(
            result.unwrap_err(),
            TransactionValidityError::Invalid(InvalidTransaction::BadProof)
        );
    });
}

#[test]
fn registry_mode_rejects_already_authorized_origin() {
    new_test_ext().execute_with(|| {
        let (sign_key, vrfy_key) = keypair(7);
        let account = register(&vrfy_key, 7);

        let call = remark_call();
        let msg_hash = call.using_encoded(blake3_hash);
        let sig_bytes = sign(&sign_key, 8, &msg_hash);
        let sig = FalconSignature(BoundedVec::try_from(sig_bytes).unwrap());

        let ext = CheckFalconSignature::registry_mode(sig, account.clone());
        let info = DispatchInfo::default();
        // Origin already signed by someone -- CheckFalconSignature must never run
        // after an authorizing extension/preamble.
        let origin = RuntimeOrigin::signed(account);

        let result = ext.validate(origin, &call, &info, 0, (), &TxBaseImplication(&call), TransactionSource::External);
        assert_eq!(
            result.unwrap_err(),
            TransactionValidityError::Invalid(InvalidTransaction::BadSigner)
        );
    });
}

#[test]
fn disabled_is_a_pure_passthrough() {
    new_test_ext().execute_with(|| {
        let call = remark_call();
        let ext = CheckFalconSignature::disabled();
        let info = DispatchInfo::default();
        let origin = RuntimeOrigin::none();

        let (_, _, resulting_origin) = ext
            .validate(origin, &call, &info, 0, (), &TxBaseImplication(&call), TransactionSource::External)
            .unwrap();
        assert_eq!(resulting_origin.as_system_origin_signer(), None);
    });
}
