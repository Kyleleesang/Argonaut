//! Prism gossip and sync protocols (wired over sc-network notifications + req/resp).
//!
//! Two protocols per §3.6:
//! - `/argonaut/prism/blocks/1` (notifications): push full PrismBlocks to peers.
//!   If tx blocks exceed notification size comfort, announce hash and serve via sync.
//! - `/argonaut/prism/sync/1` (request/response): `GetBlocks(Vec<H256>) → Vec<PrismBlock>`
//!   for orphan resolution and initial sync.
//!
//! On receive: validate_block → insert_block → re-gossip valid blocks (flood + seen-cache).
//! Missing refs from InsertOutcome drive sync requests.
use argonaut_primitives_prism::{validate_block, PrismBlock, Params, H256};
use parity_scale_codec::{Decode, Encode};

pub const PRISM_BLOCKS_PROTOCOL: &str = "/argonaut/prism/blocks/1";
pub const PRISM_SYNC_PROTOCOL: &str = "/argonaut/prism/sync/1";

/// N-3: capacity of the gossip seen-cache. Bounded so a flood of distinct
/// valid blocks can't grow it for the life of the process.
pub const SEEN_CACHE_CAP: usize = 50_000;

/// N-3: bounded FIFO-evicting set of block hashes already processed. Only
/// hashes of blocks that passed `validate_block` go in (a corrupt-proof relay
/// must not censor the genuine block), and hashes of orphans the chain
/// evicted are removed again so they can be re-fetched/re-gossiped.
#[derive(Debug)]
pub struct SeenCache {
    set: std::collections::HashSet<H256>,
    order: std::collections::VecDeque<H256>,
    cap: usize,
}

impl Default for SeenCache {
    fn default() -> Self {
        Self::new()
    }
}

impl SeenCache {
    pub fn new() -> Self {
        Self::with_capacity(SEEN_CACHE_CAP)
    }

    pub fn with_capacity(cap: usize) -> Self {
        SeenCache { set: Default::default(), order: Default::default(), cap: cap.max(1) }
    }

    pub fn contains(&self, h: &H256) -> bool {
        self.set.contains(h)
    }

    pub fn insert(&mut self, h: H256) {
        if !self.set.insert(h) {
            return;
        }
        self.order.push_back(h);
        while self.order.len() > self.cap {
            if let Some(old) = self.order.pop_front() {
                self.set.remove(&old);
            }
        }
    }

    pub fn remove(&mut self, h: &H256) {
        if self.set.remove(h) {
            self.order.retain(|x| x != h);
        }
    }

    pub fn len(&self) -> usize {
        self.set.len()
    }

    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }
}

/// Request message on the sync protocol.
#[derive(Encode, Decode, Clone, Debug)]
pub enum SyncRequest {
    GetBlocks(Vec<H256>),
    /// B-6: ask a peer for a snapshot of its DAG frontier, to discover
    /// whether it's ahead of us and seed a backfill. SCALE enums decode by
    /// variant index, so this is safe to append after `GetBlocks` -- an
    /// older peer that doesn't know this variant simply fails to decode the
    /// request and returns an error, which the requester already handles.
    GetStatus,
}

/// Response message on the sync protocol.
#[derive(Encode, Decode, Clone, Debug)]
pub enum SyncResponse {
    Blocks(Vec<PrismBlock>),
    /// Answer to `SyncRequest::GetStatus`: read straight off `PrismChain`'s
    /// public fields, no dedicated DB index needed (B-7 already keeps the
    /// in-memory DAG complete). `voter_tips` is one hash per voter chain
    /// (chain id = index); `unreferenced_proposers`/`unreferenced_tx_blocks`
    /// are exactly the DAG frontier a backfilling peer needs to walk
    /// backward from.
    Status {
        proposer_best_level: u64,
        voter_tips: Vec<H256>,
        unreferenced_proposers: Vec<H256>,
        unreferenced_tx_blocks: Vec<H256>,
    },
}

/// Announcement on the blocks notification protocol.
#[derive(Encode, Decode, Clone, Debug)]
pub enum BlockAnnouncement {
    /// Full block (used when it fits in the notification budget).
    Full(PrismBlock),
    /// Hash-only announcement; peer can fetch via sync if they want it.
    Hash(H256),
}

/// Outcome of processing a received block announcement.
pub struct ReceiveOutcome {
    /// Blocks to re-gossip to connected peers.
    pub to_gossip: Vec<PrismBlock>,
    /// Hashes to request from the announcing peer (missing refs).
    pub to_fetch: Vec<H256>,
    /// AF-1: threaded straight from `InsertOutcome::may_confirm` -- true if
    /// this block's acceptance might have advanced confirmation for some
    /// level, so the caller should wake the bridge rather than let it wait
    /// out its poll interval.
    pub may_confirm: bool,
    /// N-2: the announced block failed `validate_block`; the caller should
    /// penalise whoever sent it.
    pub invalid: bool,
}

/// Process an incoming PrismBlock announcement.
/// Validates structurally, then delegates to the chain state machine.
pub fn handle_announcement(
    announcement: BlockAnnouncement,
    params: &Params,
    seen: &mut SeenCache,
    chain: &mut argonaut_prism_chain::PrismChain,
) -> ReceiveOutcome {
    let block = match announcement {
        BlockAnnouncement::Full(b) => b,
        BlockAnnouncement::Hash(_) => {
            // Hash-only: caller should issue a sync request; nothing to do here.
            return ReceiveOutcome { to_gossip: vec![], to_fetch: vec![], may_confirm: false, invalid: false };
        }
    };

    let hash = argonaut_primitives_prism::block_hash(&block.header);
    if seen.contains(&hash) {
        return ReceiveOutcome { to_gossip: vec![], to_fetch: vec![], may_confirm: false, invalid: false };
    }

    // N-3: insert into `seen` only once the block validates. The hash covers
    // the header only, so a relayed header with a corrupt proof would
    // otherwise mark the genuine block as seen and censor it.
    if validate_block(&block, params).is_err() {
        return ReceiveOutcome { to_gossip: vec![], to_fetch: vec![], may_confirm: false, invalid: true };
    }
    seen.insert(hash);

    let outcome = chain.insert_block(block.clone());
    // N-3: an orphan the chain evicted can never resolve; forget it so a
    // later re-gossip is processed instead of dropped as a duplicate.
    for evicted in chain.take_evicted_orphans() {
        seen.remove(&evicted);
    }
    ReceiveOutcome {
        to_gossip: vec![block],
        to_fetch: outcome.missing,
        may_confirm: outcome.may_confirm,
        invalid: false,
    }
}

pub mod wire;

#[cfg(test)]
mod tests;
