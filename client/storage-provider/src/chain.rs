//! D.9 `chain.rs`: typed wrapper over `argonaut-chain-client` for exactly the
//! calls/reads this daemon needs. Storage types are the pallets' own
//! `Encode`/`Decode` structs (`pallet-storage-deals`/`-proofs`/
//! `-provider-registry` depended on directly), not hand-mirrored copies --
//! decode correctness comes from reusing the real type, not restating it.

use argonaut_chain_client::extrinsic::SignMode;
use argonaut_chain_client::keys::{double_map_key, map_key, Hasher};
use argonaut_chain_client::{ChainClient, ClientError};
use argonaut_primitives_storage::{BlobRoot, DealId, EpochIndex, ShardIndex};
use argonaut_runtime::{self as runtime, AccountId, Balance, RuntimeCall};
use pallet_provider_registry::ProviderInfo;
use pallet_storage_deals::{Assignment, Deal};
use rand_core::{CryptoRng, RngCore};
use sp_core::bounded::BoundedVec;

pub struct ChainHandle {
    client: ChainClient,
}

impl ChainHandle {
    pub async fn connect(url: &str) -> Result<Self, ClientError> {
        Ok(Self { client: ChainClient::connect(url).await? })
    }

    // ── provider-registry ───────────────────────────────────────────────

    pub async fn provider_info(&self, who: &AccountId) -> Result<Option<ProviderInfo<Balance>>, ClientError> {
        let key = map_key("ProviderRegistry", "Providers", Hasher::Blake2_128Concat, who);
        self.client.read_storage(&key).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn register_provider(
        &self,
        stake: Balance,
        iroh_node_id: [u8; 32],
        relay_hint: Vec<u8>,
        price_per_shard_epoch: Balance,
        mode: SignMode,
        signing_key: &[u8],
        signer_account: &AccountId,
        rng: &mut (impl RngCore + CryptoRng),
    ) -> Result<sp_core::H256, ClientError> {
        let call: RuntimeCall = runtime::ProviderRegistryCall::register {
            stake,
            iroh_node_id,
            relay_hint: BoundedVec::try_from(relay_hint)
                .map_err(|_| ClientError::Extrinsic("relay_hint exceeds 256 bytes".into()))?,
            price_per_shard_epoch,
        }
        .into();
        self.client.submit_call(call, mode, signing_key, signer_account, rng).await
    }

    // ── storage-deals ───────────────────────────────────────────────────

    pub async fn deal(&self, deal_id: DealId) -> Result<Option<Deal<AccountId, Balance>>, ClientError> {
        let key = map_key("StorageDeals", "Deals", Hasher::Blake2_128Concat, &deal_id);
        self.client.read_storage(&key).await
    }

    pub async fn assignment(
        &self,
        deal_id: DealId,
        shard_idx: ShardIndex,
    ) -> Result<Option<Assignment<AccountId>>, ClientError> {
        let key = double_map_key(
            "StorageDeals",
            "ShardAssignments",
            Hasher::Blake2_128Concat,
            &deal_id,
            Hasher::Twox64Concat,
            &shard_idx,
        );
        self.client.read_storage(&key).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn accept_shard(
        &self,
        deal_id: DealId,
        shard_idx: ShardIndex,
        mode: SignMode,
        signing_key: &[u8],
        signer_account: &AccountId,
        rng: &mut (impl RngCore + CryptoRng),
    ) -> Result<sp_core::H256, ClientError> {
        let call: RuntimeCall =
            runtime::StorageDealsCall::accept_shard { deal_id, shard_idx }.into();
        self.client.submit_call(call, mode, signing_key, signer_account, rng).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn activate_shard(
        &self,
        deal_id: DealId,
        shard_idx: ShardIndex,
        slice: Vec<u8>,
        mode: SignMode,
        signing_key: &[u8],
        signer_account: &AccountId,
        rng: &mut (impl RngCore + CryptoRng),
    ) -> Result<sp_core::H256, ClientError> {
        let call: RuntimeCall = runtime::StorageDealsCall::activate_shard {
            deal_id,
            shard_idx,
            slice: BoundedVec::try_from(slice)
                .map_err(|_| ClientError::Extrinsic("activation slice exceeds MAX_PROOF_BYTES".into()))?,
        }
        .into();
        self.client.submit_call(call, mode, signing_key, signer_account, rng).await
    }

    // ── storage-proofs ──────────────────────────────────────────────────

    pub async fn beacon(&self, epoch: EpochIndex) -> Result<Option<[u8; 32]>, ClientError> {
        let key = map_key("StorageProofs", "Beacons", Hasher::Twox64Concat, &epoch);
        self.client.read_storage(&key).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn submit_proof(
        &self,
        deal_id: DealId,
        shard_idx: ShardIndex,
        epoch: EpochIndex,
        slice: Vec<u8>,
        mode: SignMode,
        signing_key: &[u8],
        signer_account: &AccountId,
        rng: &mut (impl RngCore + CryptoRng),
    ) -> Result<sp_core::H256, ClientError> {
        let payload = pallet_storage_proofs::ProofPayload::Single(
            BoundedVec::try_from(slice)
                .map_err(|_| ClientError::Extrinsic("proof slice exceeds MAX_PROOF_BYTES".into()))?,
        );
        let call: RuntimeCall =
            runtime::StorageProofsCall::submit_proof { deal_id, shard_idx, epoch, payload }.into();
        self.client.submit_call(call, mode, signing_key, signer_account, rng).await
    }

    /// Current epoch, derived from the finalized head's block number --
    /// mirrors `pallet_storage_proofs::Pallet::epoch_now` exactly
    /// (`number / EpochDuration`), computed client-side so the daemon
    /// doesn't need a runtime API call for it.
    pub async fn current_epoch(&self, epoch_duration_blocks: u64) -> Result<EpochIndex, ClientError> {
        let (_, number) = self.client.finalized_head().await?;
        Ok((number / epoch_duration_blocks) as EpochIndex)
    }

    pub async fn account_nonce(&self, account: &AccountId) -> Result<u32, ClientError> {
        self.client.account_nonce(account).await
    }
}

/// A blake3-hashed blob root is exactly what `argonaut_primitives_storage`
/// calls it -- re-exported here so callers of this module never need to
/// import the primitives crate themselves just for the type name.
pub type Root = BlobRoot;
