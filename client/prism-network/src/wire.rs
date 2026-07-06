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
use argonaut_primitives_prism::{block_hash, Params, H256};
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
use std::collections::HashSet;
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
    params: Params,
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
        params: Params,
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
                params,
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
        let result = self
            .network
            .request(from, ProtocolName::from(crate::PRISM_SYNC_PROTOCOL), request, None, IfDisconnected::ImmediateError)
            .await;
        let Ok((response, _)) = result else { return };
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
            handle_announcement(announcement, &self.params, &mut self.seen, &mut chain)
        };
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
        let SyncRequest::GetBlocks(hashes) = SyncRequest::decode(&mut &payload[..]).map_err(|_| ())?;
        let blocks = hashes
            .into_iter()
            .filter_map(|h| self.db.get_block(&h).ok().flatten())
            .collect();
        Ok(SyncResponse::Blocks(blocks).encode())
    }

    pub async fn run(mut self) {
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
            }
        }
    }
}
