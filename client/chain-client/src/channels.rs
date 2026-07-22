//! Payment-channel operations (D.7), shared by the D4 client CLI (opens
//! channels, signs vouchers per retrieval stride) and the provider daemon
//! (settles the freshest voucher on-chain). Lives in chain-client rather
//! than the daemon's own `chain.rs` because both sides need it -- vouchers
//! are the payment primitive for retrieval, not a daemon-only concern.

use crate::extrinsic::SignMode;
use crate::keys::{map_key, Hasher};
use crate::{ChainClient, ClientError};
use argonaut_primitives_falcon::{FalconPublic, FalconSignature};
use argonaut_primitives_storage::{voucher_signing_payload, ChannelId, Voucher};
use argonaut_runtime::{self as runtime, AccountId, Balance, BlockNumber, RuntimeCall};
use rand_core::{CryptoRng, RngCore};
use sp_core::bounded::BoundedVec;
use sp_core::H256;

pub type ChannelInfo = pallet_payment_channels::Channel<AccountId, Balance, BlockNumber>;

impl ChainClient {
    pub async fn channel(&self, channel_id: ChannelId) -> Result<Option<ChannelInfo>, ClientError> {
        let key = map_key("PaymentChannels", "Channels", Hasher::Blake2_128Concat, &channel_id);
        self.read_storage(&key).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn open_channel(
        &self,
        to: AccountId,
        deposit: Balance,
        ttl_blocks: BlockNumber,
        mode: SignMode,
        signing_key: &[u8],
        signer_account: &AccountId,
        rng: &mut (impl RngCore + CryptoRng),
    ) -> Result<H256, ClientError> {
        let call: RuntimeCall = runtime::PaymentChannelsCall::open { to, deposit, ttl_blocks }.into();
        self.submit_call(call, mode, signing_key, signer_account, rng).await
    }

    /// Settle the freshest voucher the provider has received for `channel_id`.
    /// Callable repeatedly with an ever-larger `voucher.cumulative`.
    #[allow(clippy::too_many_arguments)]
    pub async fn settle_channel(
        &self,
        channel_id: ChannelId,
        voucher: Voucher,
        sig: FalconSignature,
        pk: Option<FalconPublic>,
        mode: SignMode,
        signing_key: &[u8],
        signer_account: &AccountId,
        rng: &mut (impl RngCore + CryptoRng),
    ) -> Result<H256, ClientError> {
        let call: RuntimeCall =
            runtime::PaymentChannelsCall::settle { channel_id, voucher, sig, pk }.into();
        self.submit_call(call, mode, signing_key, signer_account, rng).await
    }
}

/// Sign a voucher with the client's Falcon key -- exactly the digest
/// `pallet_payment_channels::settle` verifies against
/// (`voucher_signing_payload`, itself already a blake3 hash; see
/// `argonaut_signer_core::sign_digest`'s doc comment for why this must not
/// go through `sign_payload`, which would hash it a second time).
pub fn sign_voucher(
    signing_key: &[u8],
    voucher: &Voucher,
    rng: &mut (impl RngCore + CryptoRng),
) -> Result<FalconSignature, ClientError> {
    let digest = voucher_signing_payload(voucher);
    let sig_bytes = argonaut_signer_core::sign_digest(signing_key, &digest, rng)
        .map_err(ClientError::Extrinsic)?;
    Ok(FalconSignature(
        BoundedVec::try_from(sig_bytes)
            .map_err(|_| ClientError::Extrinsic("voucher signature exceeds the on-chain bound".into()))?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand_chacha::rand_core::SeedableRng;
    use rand_chacha::ChaCha20Rng;

    #[test]
    fn sign_voucher_produces_a_signature_the_real_host_fn_accepts() {
        let mut rng = ChaCha20Rng::seed_from_u64(42);
        let kp = argonaut_signer_core::generate_keypair(&mut rng);
        let voucher = Voucher { channel_id: 7, cumulative: 1_000 };

        let sig = sign_voucher(&kp.signing_key, &voucher, &mut rng).unwrap();

        let digest = voucher_signing_payload(&voucher);
        assert!(argonaut_primitives_falcon::falcon_crypto::verify(&sig.0, &digest, &kp.verifying_key));

        // A different voucher must not verify against this signature.
        let other = Voucher { channel_id: 7, cumulative: 1_001 };
        let other_digest = voucher_signing_payload(&other);
        assert!(!argonaut_primitives_falcon::falcon_crypto::verify(&sig.0, &other_digest, &kp.verifying_key));
    }
}
