//! D2 tests for `pallet-payment-channels` using REAL Falcon-512 vouchers.

use crate::{mock::*, Channel, Error, Event};
use argonaut_primitives_falcon::{blake3_hash, FalconPublic, FalconSignature};
use argonaut_primitives_storage::{voucher_signing_payload, Voucher};
use frame_support::{assert_noop, assert_ok, traits::Currency, BoundedVec};
use sp_core::crypto::AccountId32;

const PROVIDER_BYTES: [u8; 32] = [0x22u8; 32];
/// Providers hold funded accounts (they stake), so the payee already exists;
/// `repatriate_reserved` will not create a dead account.
const PROVIDER_INIT: u128 = 10_000;

fn provider() -> AccountId32 {
    AccountId32::from(PROVIDER_BYTES)
}

fn fund_provider() {
    Balances::make_free_balance_be(&provider(), PROVIDER_INIT);
}

// ── Falcon signing (dev-dep fn-dsa) ─────────────────────────────────────────

fn rand_seed(seed: u8) -> impl rand_core::RngCore + rand_core::CryptoRng {
    use rand_core::SeedableRng;
    rand_chacha::ChaCha20Rng::from_seed([seed; 32])
}

fn keypair(seed: u8) -> (Vec<u8>, Vec<u8>) {
    use fn_dsa::{
        sign_key_size, vrfy_key_size, KeyPairGenerator, KeyPairGeneratorStandard, FN_DSA_LOGN_512,
    };
    let mut kg = KeyPairGeneratorStandard::default();
    let mut sk = vec![0u8; sign_key_size(FN_DSA_LOGN_512)];
    let mut vk = vec![0u8; vrfy_key_size(FN_DSA_LOGN_512)];
    kg.keygen(FN_DSA_LOGN_512, &mut rand_seed(seed), &mut sk, &mut vk);
    (sk, vk)
}

fn sign(sign_key: &[u8], msg: &[u8]) -> FalconSignature {
    use fn_dsa::{signature_size, SigningKey, SigningKeyStandard, DOMAIN_NONE, HASH_ID_RAW};
    let mut sk = SigningKeyStandard::decode(sign_key).unwrap();
    let mut sig = vec![0u8; signature_size(sk.get_logn())];
    sk.sign(&mut rand_seed(200), &DOMAIN_NONE, &HASH_ID_RAW, msg, &mut sig);
    FalconSignature(BoundedVec::try_from(sig).unwrap())
}

/// A funded client whose account id is `blake3(vk)`, plus its keys.
fn client(seed: u8) -> (AccountId32, Vec<u8>, FalconPublic) {
    let (sk, vk) = keypair(seed);
    let acc = AccountId32::from(blake3_hash(&vk));
    let pk = FalconPublic(BoundedVec::try_from(vk).unwrap());
    (acc, sk, pk)
}

fn voucher_sig(sign_key: &[u8], v: &Voucher) -> FalconSignature {
    sign(sign_key, &voucher_signing_payload(v))
}

// ── tests ────────────────────────────────────────────────────────────────────

#[test]
fn open_reserves_deposit() {
    new_test_ext().execute_with(|| {
        let (from, _sk, _pk) = client(1);
        Balances::make_free_balance_be(&from, 10_000);
        assert_ok!(PaymentChannels::open(RuntimeOrigin::signed(from.clone()), provider(), 1_000, 50));
        assert_eq!(Balances::reserved_balance(&from), 1_000);
        let ch = PaymentChannels::channels(0).unwrap();
        assert_eq!(ch, Channel { from, to: provider(), deposit: 1_000, claimed: 0, expiry: 51 });
    });
}

#[test]
fn open_rejects_low_deposit() {
    new_test_ext().execute_with(|| {
        let (from, _sk, _pk) = client(2);
        Balances::make_free_balance_be(&from, 10_000);
        assert_noop!(
            PaymentChannels::open(RuntimeOrigin::signed(from), provider(), 5, 50),
            Error::<Test>::DepositTooLow
        );
    });
}

#[test]
fn settle_pays_provider_and_is_monotone() {
    new_test_ext().execute_with(|| {
        let (from, sk, pk) = client(3);
        Balances::make_free_balance_be(&from, 10_000);
        fund_provider();
        assert_ok!(PaymentChannels::open(RuntimeOrigin::signed(from.clone()), provider(), 1_000, 50));

        // First voucher: cumulative 300.
        let v1 = Voucher { channel_id: 0, cumulative: 300 };
        assert_ok!(PaymentChannels::settle(
            RuntimeOrigin::signed(provider()),
            0,
            v1.clone(),
            voucher_sig(&sk, &v1),
            Some(pk.clone()),
        ));
        assert_eq!(Balances::free_balance(&provider()), PROVIDER_INIT + 300);
        assert_eq!(PaymentChannels::channels(0).unwrap().claimed, 300);

        // Second voucher: cumulative 700 → pays the 400 delta.
        let v2 = Voucher { channel_id: 0, cumulative: 700 };
        assert_ok!(PaymentChannels::settle(
            RuntimeOrigin::signed(provider()),
            0,
            v2.clone(),
            voucher_sig(&sk, &v2),
            Some(pk.clone()),
        ));
        assert_eq!(Balances::free_balance(&provider()), PROVIDER_INIT + 700);
        System::assert_last_event(Event::ChannelSettled { channel_id: 0, paid: 400 }.into());
    });
}

#[test]
fn settle_rejects_non_monotone_voucher() {
    new_test_ext().execute_with(|| {
        let (from, sk, pk) = client(4);
        Balances::make_free_balance_be(&from, 10_000);
        assert_ok!(PaymentChannels::open(RuntimeOrigin::signed(from.clone()), provider(), 1_000, 50));
        let v1 = Voucher { channel_id: 0, cumulative: 300 };
        assert_ok!(PaymentChannels::settle(
            RuntimeOrigin::signed(provider()),
            0,
            v1.clone(),
            voucher_sig(&sk, &v1),
            Some(pk.clone()),
        ));
        // A voucher not exceeding `claimed` is rejected.
        let v_stale = Voucher { channel_id: 0, cumulative: 200 };
        assert_noop!(
            PaymentChannels::settle(
                RuntimeOrigin::signed(provider()),
                0,
                v_stale.clone(),
                voucher_sig(&sk, &v_stale),
                Some(pk),
            ),
            Error::<Test>::BadVoucher
        );
    });
}

#[test]
fn settle_rejects_bad_signature() {
    new_test_ext().execute_with(|| {
        let (from, _sk, pk) = client(5);
        let (_from2, sk_other, _pk2) = client(6); // wrong signer
        Balances::make_free_balance_be(&from, 10_000);
        assert_ok!(PaymentChannels::open(RuntimeOrigin::signed(from.clone()), provider(), 1_000, 50));
        let v = Voucher { channel_id: 0, cumulative: 300 };
        assert_noop!(
            PaymentChannels::settle(
                RuntimeOrigin::signed(provider()),
                0,
                v.clone(),
                voucher_sig(&sk_other, &v), // signed by the wrong key
                Some(pk),
            ),
            Error::<Test>::BadSignature
        );
    });
}

#[test]
fn settle_registry_mode_resolves_key() {
    new_test_ext().execute_with(|| {
        let (from, sk, pk) = client(7);
        Balances::make_free_balance_be(&from, 10_000);
        fund_provider();
        register_key(from.clone(), pk.clone());
        assert_ok!(PaymentChannels::open(RuntimeOrigin::signed(from.clone()), provider(), 1_000, 50));
        let v = Voucher { channel_id: 0, cumulative: 250 };
        assert_ok!(PaymentChannels::settle(
            RuntimeOrigin::signed(provider()),
            0,
            v.clone(),
            voucher_sig(&sk, &v),
            None, // resolved via the registry
        ));
        assert_eq!(Balances::free_balance(&provider()), PROVIDER_INIT + 250);
    });
}

#[test]
fn close_by_provider_refunds_remainder() {
    new_test_ext().execute_with(|| {
        let (from, sk, pk) = client(8);
        Balances::make_free_balance_be(&from, 10_000);
        fund_provider();
        assert_ok!(PaymentChannels::open(RuntimeOrigin::signed(from.clone()), provider(), 1_000, 50));
        let v = Voucher { channel_id: 0, cumulative: 400 };
        assert_ok!(PaymentChannels::settle(
            RuntimeOrigin::signed(provider()),
            0,
            v.clone(),
            voucher_sig(&sk, &v),
            Some(pk),
        ));
        // Provider closes; client gets back deposit - claimed = 600.
        assert_ok!(PaymentChannels::close(RuntimeOrigin::signed(provider()), 0));
        assert!(PaymentChannels::channels(0).is_none());
        assert_eq!(Balances::reserved_balance(&from), 0);
        assert_eq!(Balances::free_balance(&from), 10_000 - 400);
    });
}

#[test]
fn close_by_client_requires_expiry() {
    new_test_ext().execute_with(|| {
        let (from, _sk, _pk) = client(9);
        Balances::make_free_balance_be(&from, 10_000);
        assert_ok!(PaymentChannels::open(RuntimeOrigin::signed(from.clone()), provider(), 1_000, 50));
        // Before expiry (block 1 < 51): client cannot close.
        assert_noop!(
            PaymentChannels::close(RuntimeOrigin::signed(from.clone()), 0),
            Error::<Test>::NotExpired
        );
        System::set_block_number(51);
        assert_ok!(PaymentChannels::close(RuntimeOrigin::signed(from.clone()), 0));
        assert_eq!(Balances::reserved_balance(&from), 0);
    });
}
