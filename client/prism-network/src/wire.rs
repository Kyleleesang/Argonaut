//! Real `sc-network` transport for the two protocols declared in `lib.rs`:
//! `/argonaut/prism/blocks/1` (notifications, flood gossip) and
//! `/argonaut/prism/sync/1` (request/response, orphan/catch-up fetch).
//!
//! Protocol *config* construction (`blocks_protocol_config`/`sync_protocol_config`)
//! must run before `sc_service::build_network` is called -- see
//! `sc_network::config::FullNetworkConfiguration::add_notification_protocol`/
//! `add_request_response_protocol`, which take these configs by value and are
//! consumed when the network object is actually built. This mirrors exactly how
//! `sc_consensus_grandpa::grandpa_peers_set_config` and
//! `sc_network_transactions::TransactionsHandlerPrototype::new` build their own
//! protocol configs, generic over `N: NetworkBackend` so it works with either
//! network backend (`NetworkWorker`/libp2p or `Litep2pNetworkBackend`) the node
//! is started with.
use crate::{handle_announcement, BlockAnnouncement, SyncRequest, SyncResponse};
use argonaut_primitives_prism::{block_hash, H256};
use argonaut_prism_chain::PrismChain;
use argonaut_prism_db::PrismDb;
use parity_scale_codec::{Decode, Encode};
use sc_network::{
    config::{
        IncomingRequest, NonReservedPeerMode, NotificationService, OutgoingResponse, SetConfig,
    },
    service::traits::{NetworkBackend, NetworkService as NetworkServiceTrait, NotificationEvent, ValidationResult},
    IfDisconnected, NetworkRequest, NotificationMetrics, PeerId, ProtocolName,
};
use futures::FutureExt;
use sp_runtime::traits::Block as BlockT;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

/// Build the `/argonaut/prism/blocks/1` notification protocol config + its
/// `NotificationService` handle.
///
/// GRANDPA and the transactions protocol both register with zero dedicated
/// peer slots (`in_peers`/`out_peers` = 0, `Deny` non-reserved) and instead
/// mirror whichever peers the *default* sync protocol is already connected to,
/// by calling `NetworkPeers::add_peers_to_reserved_set` as sync peers come and
/// go. Replicating that mirroring is out of scope for this rehearsal, so this
/// protocol manages its own independent peer slots instead (`Accept` mode, a
/// generous slot budget) -- simpler, and sufficient for gossiping between the
/// small number of peers a devnet actually has.
pub fn blocks_protocol_config<B, N>(
    metrics: NotificationMetrics,
    peer_store_handle: Arc<dyn sc_network::config::PeerStoreProvider>,
) -> (N::NotificationProtocolConfig, Box<dyn NotificationService>)
where
    B: BlockT + 'static,
    N: NetworkBackend<B, <B as BlockT>::Hash>,
{
    N::notification_config(
        ProtocolName::from(crate::PRISM_BLOCKS_PROTOCOL),
        Vec::new(),
        1024 * 1024,
        None,
        SetConfig {
            in_peers: 25,
            out_peers: 25,
            reserved_nodes: Vec::new(),
            non_reserved_mode: NonReservedPeerMode::Accept,
        },
        metrics,
        peer_store_handle,
    )
}

/// Build the `/argonaut/prism/sync/1` request/response protocol config. The
/// caller keeps the returned `IncomingRequest` receiver to answer requests
/// (see `PrismNetworkWorker::run`'s handling below).
pub fn sync_protocol_config<B, N>(
    inbound_queue: async_channel::Sender<IncomingRequest>,
) -> N::RequestResponseProtocolConfig
where
    B: BlockT + 'static,
    N: NetworkBackend<B, <B as BlockT>::Hash>,
{
    N::request_response_config(
        ProtocolName::from(crate::PRISM_SYNC_PROTOCOL),
        Vec::new(),
        1024 * 1024,
        16 * 1024 * 1024,
        std::time::Duration::from_secs(20),
        Some(inbound_queue),
    )
}

/// Drives both protocols for a running node: gossips/validates/inserts blocks
/// received over `/argonaut/prism/blocks/1`, answers `/argonaut/prism/sync/1`
/// requests from `db`, and floods newly-mined blocks (received over
/// `mined_blocks`) to connected peers.
pub struct PrismNetworkWorker {
    notification_service: Box<dyn NotificationService>,
    sync_requests: async_channel::Receiver<IncomingRequest>,
    mined_blocks: futures::channel::mpsc::UnboundedReceiver<argonaut_primitives_prism::PrismBlock>,
    network: Arc<dyn NetworkServiceTrait>,
    chain: Arc<Mutex<PrismChain>>,
    db: Arc<PrismDb>,
    /// AF-1: signaled whenever an accepted block's `may_confirm` comes back
    /// true, so `node/src/bridge.rs`'s bridge task can wake immediately
    /// instead of waiting out a fixed poll interval. Shared with (constructed
    /// by) the caller, which also hands the same `Arc` to the bridge.
    confirmation_notify: Arc<tokio::sync::Notify>,
    seen: HashSet<H256>,
    peers: HashSet<PeerId>,
}

impl PrismNetworkWorker {
    pub fn new(
        notification_service: Box<dyn NotificationService>,
        sync_requests: async_channel::Receiver<IncomingRequest>,
        network: Arc<dyn NetworkServiceTrait>,
        chain: Arc<Mutex<PrismChain>>,
        db: Arc<PrismDb>,
        confirmation_notify: Arc<tokio::sync::Notify>,
    ) -> (Self, futures::channel::mpsc::UnboundedSender<argonaut_primitives_prism::PrismBlock>) {
        let (mined_tx, mined_blocks) = futures::channel::mpsc::unbounded();
        (
            PrismNetworkWorker {
                notification_service,
                sync_requests,
                mined_blocks,
                network,
                chain,
                db,
                confirmation_notify,
                seen: HashSet::new(),
                peers: HashSet::new(),
            },
            mined_tx,
        )
    }

    /// Flood `block` to every currently-connected peer. Harmless if it echoes
    /// back to whoever sent it to us -- their own `seen` cache (via
    /// `handle_announcement`) drops the duplicate silently.
    fn gossip(&mut self, block: &argonaut_primitives_prism::PrismBlock) {
        let bytes = BlockAnnouncement::Full(block.clone()).encode();
        for peer in self.peers.iter().copied().collect::<Vec<_>>() {
            self.notification_service.send_sync_notification(&peer, bytes.clone());
        }
    }

    /// Ask `from` (the peer who announced something we're missing refs for)
    /// to send us the missing blocks, and insert whatever comes back.
    async fn fetch(&mut self, from: PeerId, missing: Vec<H256>) {
        if missing.is_empty() {
            return;
        }
        let request = SyncRequest::GetBlocks(missing).encode();
        let Some(response) = sync_request(&self.network, from, request).await else { return };
        let Ok(SyncResponse::Blocks(blocks)) = SyncResponse::decode(&mut response.as_slice()) else {
            return;
        };
        for block in blocks {
            // `process` -> `fetch` -> `process` is a genuine (bounded by how
            // many hops of missing refs actually exist) recursive cycle;
            // async fn requires explicit boxing to give the future a static size.
            Box::pin(self.process(BlockAnnouncement::Full(block), Some(from))).await;
        }
    }

    /// Shared path for both self-mined and network-received blocks: validate,
    /// insert, gossip onward, and chase any refs it's still missing.
    async fn process(&mut self, announcement: BlockAnnouncement, source: Option<PeerId>) {
        let outcome = {
            let mut chain = self.chain.lock().expect("PrismChain mutex poisoned");
            // B-2: read the *live* (possibly retargeted) params straight off
            // `chain` rather than this worker's own frozen copy -- otherwise,
            // once `DifficultyRetarget` moves `f_v`/`f_t`/`f_p`, this
            // validation path would keep checking incoming blocks' sortition
            // consistency against stale thresholds, either rejecting validly
            // mined blocks or misclassifying their content type.
            let params = chain.params.clone();
            handle_announcement(announcement, &params, &mut self.seen, &mut chain)
        };
        if outcome.may_confirm {
            // AF-1: wake the bridge immediately rather than making it wait
            // out its own poll interval -- `notify_one` stores a permit if
            // nothing is waiting yet, so this is safe regardless of which
            // task reaches its side of the handshake first.
            self.confirmation_notify.notify_one();
        }
        for block in &outcome.to_gossip {
            log::info!(
                target: "prism",
                "accepted block {:?} (source: {})",
                block_hash(&block.header),
                source.map(|p| p.to_string()).unwrap_or_else(|| "self-mined".into()),
            );
            self.gossip(block);
        }
        if let Some(peer) = source {
            self.fetch(peer, outcome.to_fetch).await;
        }
    }

    fn answer_sync_request(&self, payload: &[u8]) -> Result<Vec<u8>, ()> {
        match SyncRequest::decode(&mut &payload[..]).map_err(|_| ())? {
            SyncRequest::GetBlocks(hashes) => {
                let blocks = hashes
                    .into_iter()
                    .filter_map(|h| self.db.get_block(&h).ok().flatten())
                    .collect();
                Ok(SyncResponse::Blocks(blocks).encode())
            }
            SyncRequest::GetStatus => {
                let chain = self.chain.lock().expect("PrismChain mutex poisoned");
                Ok(SyncResponse::Status {
                    proposer_best_level: chain.proposer_best_level,
                    voter_tips: chain.voter_tips.iter().map(|(h, _)| *h).collect(),
                    unreferenced_proposers: chain.unreferenced_proposers.iter().copied().collect(),
                    unreferenced_tx_blocks: chain.unreferenced_tx_blocks.iter().copied().collect(),
                }
                .encode())
            }
        }
    }

    /// B-6: ask `peer` for its DAG frontier and, for anything in it we don't
    /// already have, kick off `backfill`. Filtering against
    /// `PrismChain::has_block` (rather than e.g. this worker's own `seen`
    /// cache) is what makes this safe across restarts: `seen` starts empty
    /// on every new `PrismNetworkWorker`, but `has_block` reads the
    /// replay-restored chain state directly, so a restarted node doesn't
    /// re-request blocks it already recovered via `PrismChain::with_db`.
    ///
    /// B-18: the frontier-derived seed alone is not enough -- `has_block` is
    /// true for orphans, so once gossip has delivered the tips the seed is
    /// empty forever while the orphans' ancestors are still missing. The seed
    /// is therefore also extended with `PrismChain::orphan_missing_refs`.
    /// Because this runs on every 30s resync tick for every peer, an ancestor
    /// a pruning peer can't serve (and `backfill` dropped after
    /// `MAX_ROUNDS_MISSING`) is re-requested from every other peer (e.g. the
    /// archive node) until it resolves.
    async fn request_status_and_maybe_backfill(&mut self, peer: PeerId) {
        let request = SyncRequest::GetStatus.encode();
        let Some(response) = sync_request(&self.network, peer, request).await else { return };
        let Ok(SyncResponse::Status { voter_tips, unreferenced_proposers, unreferenced_tx_blocks, .. }) =
            SyncResponse::decode(&mut response.as_slice())
        else {
            return;
        };

        let (seed, frontier_n, orphan_n): (Vec<H256>, usize, usize) = {
            let chain = self.chain.lock().expect("PrismChain mutex poisoned");
            let mut seed: Vec<H256> = voter_tips
                .iter()
                .chain(unreferenced_proposers.iter())
                .chain(unreferenced_tx_blocks.iter())
                .filter(|h| !chain.has_block(h))
                .copied()
                .collect();
            let frontier_n = seed.len();
            let mut in_seed: HashSet<H256> = seed.iter().copied().collect();
            for h in chain.orphan_missing_refs() {
                if in_seed.insert(h) {
                    seed.push(h);
                }
            }
            let orphan_n = seed.len() - frontier_n;
            (seed, frontier_n, orphan_n)
        };
        if seed.is_empty() {
            return;
        }
        log::info!(
            target: "prism",
            "backfill: {frontier_n} unknown hash(es) + {orphan_n} orphan ancestor(s) from {peer}"
        );
        self.backfill(peer, seed).await;
    }

    /// Iteratively resolve `seed` and everything it transitively depends on
    /// from `peer`, in batches, retrying a bounded number of rounds before
    /// dropping a hash a peer never actually delivers. Each returned block
    /// is inserted via `insert_backfill_block`, which -- unlike `process` --
    /// never re-gossips (a fresh/long-offline node backfilling years of
    /// history shouldn't flood it back out to everyone else).
    async fn backfill(&mut self, peer: PeerId, seed: Vec<H256>) {
        const BATCH: usize = 64;
        const MAX_ROUNDS_MISSING: u32 = 5;

        let mut pending: HashMap<H256, u32> = seed.into_iter().map(|h| (h, 0)).collect();
        while !pending.is_empty() {
            let batch: Vec<H256> = pending.keys().take(BATCH).copied().collect();
            let request = SyncRequest::GetBlocks(batch.clone()).encode();
            let Some(response) = sync_request(&self.network, peer, request).await else {
                // Peer unreachable this round -- drop the batch rather than
                // spin on a dead peer; a live peer re-advertises via the
                // next periodic status check.
                for h in &batch {
                    pending.remove(h);
                }
                continue;
            };
            let blocks = match SyncResponse::decode(&mut response.as_slice()) {
                Ok(SyncResponse::Blocks(blocks)) => blocks,
                _ => Vec::new(),
            };

            let mut returned = HashSet::new();
            for block in blocks {
                let hash = block_hash(&block.header);
                returned.insert(hash);
                pending.remove(&hash);
                for missing in self.insert_backfill_block(block) {
                    pending.entry(missing).or_insert(0);
                }
            }
            for h in &batch {
                if !returned.contains(h) {
                    let retries = pending.entry(*h).or_insert(0);
                    *retries += 1;
                    if *retries > MAX_ROUNDS_MISSING {
                        pending.remove(h);
                    }
                }
            }
        }
    }

    /// Validate+insert a single backfilled block without gossiping it.
    /// Returns any refs it's still missing, so the caller can fold them
    /// back into its own pending queue -- this is the iterative-work-queue
    /// analogue of `process`'s recursive `fetch`.
    fn insert_backfill_block(&mut self, block: argonaut_primitives_prism::PrismBlock) -> Vec<H256> {
        let outcome = {
            let mut chain = self.chain.lock().expect("PrismChain mutex poisoned");
            let params = chain.params.clone();
            handle_announcement(BlockAnnouncement::Full(block), &params, &mut self.seen, &mut chain)
        };
        if outcome.may_confirm {
            self.confirmation_notify.notify_one();
        }
        for block in &outcome.to_gossip {
            log::info!(target: "prism", "backfilled block {:?}", block_hash(&block.header));
        }
        outcome.to_fetch
    }

    pub async fn run(mut self) {
        // B-6: slow periodic re-check so a node that's long-offline (or that
        // missed backfilling everything on first connect, e.g. a peer that
        // was itself still catching up) eventually converges without
        // needing a fresh `NotificationStreamOpened` event.
        let mut resync = tokio::time::interval(std::time::Duration::from_secs(30));
        resync.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            futures::select! {
                event = self.notification_service.next_event().fuse() => {
                    let Some(event) = event else { return };
                    match event {
                        NotificationEvent::ValidateInboundSubstream { peer, result_tx, .. } => {
                            log::info!(target: "prism", "validating inbound substream from {peer}");
                            let _ = result_tx.send(ValidationResult::Accept);
                        }
                        NotificationEvent::NotificationStreamOpened { peer, .. } => {
                            log::info!(target: "prism", "substream opened with {peer}");
                            self.peers.insert(peer);
                            self.request_status_and_maybe_backfill(peer).await;
                        }
                        NotificationEvent::NotificationStreamClosed { peer } => {
                            log::info!(target: "prism", "substream closed with {peer}");
                            self.peers.remove(&peer);
                        }
                        NotificationEvent::NotificationReceived { peer, notification } => {
                            if let Ok(announcement) = BlockAnnouncement::decode(&mut notification.as_slice()) {
                                self.process(announcement, Some(peer)).await;
                            }
                        }
                    }
                }
                request = self.sync_requests.recv().fuse() => {
                    let Ok(IncomingRequest { payload, pending_response, .. }) = request else { return };
                    let result = self.answer_sync_request(&payload);
                    let _ = pending_response.send(OutgoingResponse {
                        result,
                        reputation_changes: Vec::new(),
                        sent_feedback: None,
                    });
                }
                mined = futures::stream::StreamExt::next(&mut self.mined_blocks).fuse() => {
                    let Some(block) = mined else { continue };
                    self.process(BlockAnnouncement::Full(block), None).await;
                }
                _ = resync.tick().fuse() => {
                    for peer in self.peers.iter().copied().collect::<Vec<_>>() {
                        self.request_status_and_maybe_backfill(peer).await;
                    }
                }
            }
        }
    }
}

/// B-8: send a `/argonaut/prism/sync/1` request via `start_request`
/// (callback-based) rather than the trait's async `request` wrapper --
/// `Litep2pNetworkService::request` is a bare `unimplemented!()` (see
/// `sc-network`'s litep2p backend source), so every call site that used it
/// only worked under `--network-backend libp2p`. `start_request` is
/// implemented identically by both backends (the libp2p backend's own
/// `request` is itself just `start_request` + awaiting the same kind of
/// oneshot), so routing through it here makes both backends interchangeable.
///
/// Deliberately a free function taking `&Arc<dyn NetworkServiceTrait>`
/// rather than a `&self` method on `PrismNetworkWorker`: an `async fn(&self)`
/// held across its own internal `.await` requires `PrismNetworkWorker: Sync`
/// for the *caller's* future to stay `Send` (needed for
/// `SpawnTaskHandle::spawn`), and `PrismNetworkWorker` isn't `Sync` --
/// `notification_service: Box<dyn NotificationService>` isn't. `network`
/// alone is `Arc<dyn NetworkServiceTrait>`, which is `Send + Sync` (it's
/// already shared across many independently-spawned tasks in
/// `node/src/service.rs`), so borrowing only that field avoids dragging the
/// non-`Sync` field into the borrow.
///
/// Returns `None` uniformly for every failure mode (unreachable peer,
/// protocol mismatch, dropped channel) -- callers already treat "no
/// response" as "nothing to do this round," not an error worth
/// distinguishing.
async fn sync_request(
    network: &Arc<dyn NetworkServiceTrait>,
    peer: PeerId,
    payload: Vec<u8>,
) -> Option<Vec<u8>> {
    let (tx, rx) = futures::channel::oneshot::channel();
    network.start_request(
        peer,
        ProtocolName::from(crate::PRISM_SYNC_PROTOCOL),
        payload,
        None,
        tx,
        IfDisconnected::ImmediateError,
    );
    match rx.await {
        Ok(Ok((response, _))) => Some(response),
        _ => None,
    }
}
