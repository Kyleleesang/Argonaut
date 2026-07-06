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

pub const PRISM_BLOCKS_PROTOCOL: &[u8] = b"/argonaut/prism/blocks/1";
pub const PRISM_SYNC_PROTOCOL: &[u8] = b"/argonaut/prism/sync/1";

/// Request message on the sync protocol.
#[derive(Encode, Decode, Clone, Debug)]
pub enum SyncRequest {
    GetBlocks(Vec<H256>),
}

/// Response message on the sync protocol.
#[derive(Encode, Decode, Clone, Debug)]
pub enum SyncResponse {
    Blocks(Vec<PrismBlock>),
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
}

/// Process an incoming PrismBlock announcement.
/// Validates structurally, then delegates to the chain state machine.
pub fn handle_announcement(
    announcement: BlockAnnouncement,
    params: &Params,
    seen: &mut std::collections::HashSet<H256>,
    chain: &mut argonaut_prism_chain::PrismChain,
) -> ReceiveOutcome {
    let block = match announcement {
        BlockAnnouncement::Full(b) => b,
        BlockAnnouncement::Hash(_) => {
            // Hash-only: caller should issue a sync request; nothing to do here.
            return ReceiveOutcome { to_gossip: vec![], to_fetch: vec![] };
        }
    };

    let hash = argonaut_primitives_prism::block_hash(&block.header);
    if seen.contains(&hash) {
        return ReceiveOutcome { to_gossip: vec![], to_fetch: vec![] };
    }
    seen.insert(hash);

    if validate_block(&block, params).is_err() {
        return ReceiveOutcome { to_gossip: vec![], to_fetch: vec![] };
    }

    let outcome = chain.insert_block(block.clone());
    ReceiveOutcome {
        to_gossip: vec![block],
        to_fetch: outcome.missing,
    }
}

#[cfg(test)]
mod tests;
