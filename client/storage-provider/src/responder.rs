//! D.9 `responder.rs`: the epoch-driven challenge-response loop. On each new
//! epoch, for every shard this daemon holds, checks selection and submits a
//! proof if selected. Also handles `activate_shard` for freshly accepted
//! assignments once their activation window opens.
//!
//! Polls `finalized_head()` on an interval rather than using
//! `chain-client`'s `subscribe::finalized_heads` -- the daemon only needs to
//! notice epoch *boundaries* (granularity of `EpochDuration` blocks, not
//! every block), so a plain poll avoids the subscription stream's borrowed
//! lifetime on the underlying `WsClient` for no behavioral gain here. The
//! subscription capability stays available in chain-client for a caller that
//! genuinely needs per-block reactivity (e.g. a future block-height UI).

use crate::blobs::BlobStore;
use crate::chain::ChainHandle;
use argonaut_chain_client::extrinsic::SignMode;
use argonaut_primitives_storage::{challenged_chunk, is_selected, DealId, EpochIndex, ShardIndex};
use argonaut_runtime::AccountId;
use rand_core::OsRng;
use std::sync::Arc;
use std::time::Duration;

/// A shard this daemon is (or is trying to become) responsible for. Held
/// assignments are configured, not auto-discovered -- see the module doc in
/// `main.rs` for why D3's scope stops short of full deal-negotiation
/// automation (that's D4/the client-side wizard's job, spec §7.2).
#[derive(Clone, Debug)]
pub struct HeldAssignment {
    pub deal_id: DealId,
    pub shard_idx: ShardIndex,
}

pub struct Responder {
    pub chain: ChainHandle,
    pub blobs: Arc<dyn BlobStore>,
    pub held: Vec<HeldAssignment>,
    pub signer_account: AccountId,
    pub signing_key: Vec<u8>,
    pub epoch_duration_blocks: u64,
    pub poll_interval: Duration,
}

impl Responder {
    /// Registry-mode is always used here: by the time a provider has an
    /// assignment at all it must already be registered (accept_shard's
    /// `Providers::is_active` check), and registration itself is a one-time
    /// bootstrap-mode call the caller makes before constructing a
    /// `Responder` (see `main.rs`).
    fn mode(&self) -> SignMode {
        SignMode::Registry { account: self.signer_account.clone() }
    }

    /// Process one epoch: activate any newly-activatable assignment, then
    /// answer every selected challenge among `held`. Errors on individual
    /// shards are logged and skipped -- one bad assignment must not stall
    /// the rest of the responder's obligations (a stalled responder is
    /// exactly the failure mode this daemon exists to prevent).
    pub async fn run_epoch(&self, epoch: EpochIndex) -> anyhow::Result<()> {
        for h in &self.held {
            if let Err(e) = self.try_activate(h).await {
                log::warn!("activate_shard({}, {}) failed: {e:#}", h.deal_id, h.shard_idx);
            }
        }

        let beacon = match self.chain.beacon(epoch).await? {
            Some(b) => b,
            None => {
                log::debug!("epoch {epoch} beacon not yet available, skipping this pass");
                return Ok(());
            }
        };

        for h in &self.held {
            if let Err(e) = self.try_answer_challenge(h, epoch, &beacon).await {
                log::warn!("submit_proof({}, {}, epoch {epoch}) failed: {e:#}", h.deal_id, h.shard_idx);
            }
        }
        Ok(())
    }

    async fn try_activate(&self, h: &HeldAssignment) -> anyhow::Result<()> {
        let Some(assignment) = self.chain.assignment(h.deal_id, h.shard_idx).await? else {
            return Ok(());
        };
        if assignment.state != pallet_storage_deals::ShardState::Accepted {
            return Ok(()); // already activated, or not ours to activate
        }
        let Some(deal) = self.chain.deal(h.deal_id).await? else { return Ok(()) };
        let shard = deal
            .params
            .shards
            .get(h.shard_idx as usize)
            .ok_or_else(|| anyhow::anyhow!("shard_idx {} out of range for deal {}", h.shard_idx, h.deal_id))?;
        // Activation is keyed to accepted_epoch+1's beacon (D.5.3) -- the
        // deals pallet computes the chunk itself from the same beacon; the
        // daemon must match exactly, so it reads the same beacon storage.
        let activation_epoch = assignment.accepted_epoch.saturating_add(1);
        let Some(beacon) = self.chain.beacon(activation_epoch).await? else { return Ok(()) };
        let chunk = argonaut_primitives_storage::activation_chunk(&beacon, h.deal_id, h.shard_idx, shard.size);
        let slice = self.blobs.bao_slice(&shard.root, chunk).await?;

        let mut rng = OsRng;
        self.chain
            .activate_shard(h.deal_id, h.shard_idx, slice, self.mode(), &self.signing_key, &self.signer_account, &mut rng)
            .await?;
        log::info!("activated shard: deal={} shard={}", h.deal_id, h.shard_idx);
        Ok(())
    }

    async fn try_answer_challenge(
        &self,
        h: &HeldAssignment,
        epoch: EpochIndex,
        beacon: &[u8; 32],
    ) -> anyhow::Result<()> {
        let Some(assignment) = self.chain.assignment(h.deal_id, h.shard_idx).await? else {
            return Ok(());
        };
        if assignment.state != pallet_storage_deals::ShardState::Activated {
            return Ok(());
        }
        let Some(deal) = self.chain.deal(h.deal_id).await? else { return Ok(()) };
        if !is_selected(beacon, h.deal_id, h.shard_idx, deal.params.challenge_rate_ppm) {
            return Ok(());
        }
        let shard = deal
            .params
            .shards
            .get(h.shard_idx as usize)
            .ok_or_else(|| anyhow::anyhow!("shard_idx {} out of range for deal {}", h.shard_idx, h.deal_id))?;
        let chunk = challenged_chunk(beacon, h.deal_id, h.shard_idx, shard.size);
        let slice = self.blobs.bao_slice(&shard.root, chunk).await?;

        let mut rng = OsRng;
        self.chain
            .submit_proof(
                h.deal_id,
                h.shard_idx,
                epoch,
                slice,
                self.mode(),
                &self.signing_key,
                &self.signer_account,
                &mut rng,
            )
            .await?;
        log::info!("answered challenge: deal={} shard={} epoch={epoch}", h.deal_id, h.shard_idx);
        Ok(())
    }

    /// Run forever, polling for epoch boundaries. A crash-and-restart replays
    /// the current epoch (idempotent: `submit_proof` rejects an
    /// already-proven `(deal, shard, epoch)` with `AlreadyProven`, which this
    /// loop treats as a benign, logged no-op via the per-shard error
    /// swallowing above -- matches the catalog spec's P-1 at-least-once
    /// convention even though this daemon predates that doc).
    pub async fn run_forever(&self) -> anyhow::Result<()> {
        let mut last_epoch: Option<EpochIndex> = None;
        loop {
            match self.chain.current_epoch(self.epoch_duration_blocks).await {
                Ok(epoch) => {
                    if last_epoch != Some(epoch) {
                        log::info!("epoch {epoch} (was {last_epoch:?})");
                        self.run_epoch(epoch).await?;
                        last_epoch = Some(epoch);
                    }
                }
                Err(e) => log::warn!("failed to read current epoch: {e:#}"),
            }
            tokio::time::sleep(self.poll_interval).await;
        }
    }
}
