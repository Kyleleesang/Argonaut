//! Prism PoW mining loop — template assembly from live chain state + sortition.
//!
//! # Queue coloring (§4.4)
//! Tx-block content is drawn from one of `q` queues (keyed by `blake3(tx) % q`).
//! One queue is selected per template refresh, bounding worst-case tx duplication
//! across concurrent tx blocks. The caller picks which queue (kept out of
//! `build_template` itself so template construction stays a pure, testable
//! function); a real node would rotate/randomize it each refresh.
//!
//! # Threading model
//! Mining runs on a plain `std::thread` (not async) per spec §3.5. The nonce
//! search itself (`try_mine`) is a separate, synchronous, pure function so it
//! can be unit-tested without spinning up a thread or waiting on real PoW.
use argonaut_primitives_prism::{
    block_hash, merkle_proof, merkle_root, sortition, BlockContent, Params, PrismBlock, Sortition,
    SuperHeader, H256, MAX_TX_BLOCK_BYTES,
};
use argonaut_prism_chain::{genesis_sentinel, PrismChain};
use arc_swap::ArcSwap;
use parity_scale_codec::Encode;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// A fully assembled mining template: one header, m+2 content slots.
/// Slot order: voter 0..m-1, tx, proposer (matches parents_root / content_root Merkle leaves).
pub struct MinerTemplate {
    /// Parent hashes for each of the m+2 slots.
    pub parents: Vec<H256>,
    /// Contents for each slot.
    pub contents: Vec<BlockContent>,
    pub difficulty: sp_core::U256,
}

fn content_hash(c: &BlockContent) -> H256 {
    H256::from_slice(blake3::hash(&c.encode()).as_bytes())
}

fn parent_leaf(h: &H256) -> H256 {
    H256::from_slice(blake3::hash(h.as_bytes()).as_bytes())
}

/// `blake3(tx) % q` — which queue a pending transaction belongs to.
pub fn queue_of(tx: &[u8], q: u64) -> u64 {
    let h = blake3::hash(tx);
    u64::from_le_bytes(h.as_bytes()[..8].try_into().unwrap()) % q.max(1)
}

/// Select up to `MAX_TX_BLOCK_BYTES` worth of `mempool` transactions belonging
/// to `queue` (out of `q` queues total), in mempool order.
fn select_queue_txs(mempool: &[Vec<u8>], queue: u64, q: u64) -> Vec<Vec<u8>> {
    let mut total = 0usize;
    let mut selected = Vec::new();
    for tx in mempool {
        if queue_of(tx, q) != queue {
            continue;
        }
        if total + tx.len() > MAX_TX_BLOCK_BYTES {
            break;
        }
        total += tx.len();
        selected.push(tx.clone());
    }
    selected
}

/// Build the next mining template from live chain state and a pending-tx pool
/// (architecture guide §3.5):
/// - voter\[i\] content: `chain.unvoted_levels[i]`, first-seen proposer per level,
///   ascending (guaranteed by `BTreeMap` iteration order).
/// - tx content: up to `MAX_TX_BLOCK_BYTES` of `mempool`, restricted to `queue`
///   (one of `q` queues, per §4.4 queue coloring).
/// - proposer content: `chain.unreferenced_tx_blocks` + `chain.unreferenced_proposers`
///   (minus the chosen parent itself).
///
/// Parent selection: voter slots use their own chain's tip
/// (`chain.voter_tips[i]`); tx and proposer slots both use the "proposer
/// parent" -- the deepest known unreferenced proposer, ties broken by hash for
/// determinism within a single call (architecture guide: "this block's own
/// parent (proposer parent for tx/proposer, voter-chain parent for voter)").
pub fn build_template(
    chain: &PrismChain,
    mempool: &[Vec<u8>],
    queue: u64,
    q: u64,
    params: &Params,
) -> MinerTemplate {
    let m = params.m_voter_chains as usize;

    let proposer_parent = chain
        .unreferenced_proposers
        .iter()
        .copied()
        .max_by_key(|h| (chain.proposer_level(h).unwrap_or(0), *h))
        .unwrap_or_else(genesis_sentinel);

    let mut parents = Vec::with_capacity(m + 2);
    let mut contents = Vec::with_capacity(m + 2);

    for i in 0..m {
        let (tip_hash, _) = chain.voter_tips[i];
        parents.push(tip_hash);
        // `unvoted_levels` iterates in ascending *level* order (it's a
        // BTreeMap<level, proposer_hash>), but `validate_block`'s structural
        // "strictly ascending" check compares the proposer *hashes themselves*
        // (it has no DAG access to resolve a hash back to its level) -- so the
        // wire order has to be sorted by hash value, not level, to pass.
        let mut votes: Vec<H256> = chain.unvoted_levels[i].values().copied().collect();
        votes.sort();
        contents.push(BlockContent::Voter { chain: i as u16, voter_parent: tip_hash, votes });
    }

    parents.push(proposer_parent);
    contents.push(BlockContent::Transaction { txs: select_queue_txs(mempool, queue, q) });

    parents.push(proposer_parent);
    let tx_refs: Vec<H256> = chain.unreferenced_tx_blocks.iter().copied().collect();
    let prop_refs: Vec<H256> = chain
        .unreferenced_proposers
        .iter()
        .copied()
        .filter(|h| *h != proposer_parent)
        .collect();
    contents.push(BlockContent::Proposer { tx_refs, prop_refs });

    MinerTemplate { parents, contents, difficulty: params.difficulty }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn slot_for<'a>(
    sort: &Sortition,
    m: usize,
    tmpl: &'a MinerTemplate,
) -> (usize, &'a BlockContent, H256) {
    let slot = match sort {
        Sortition::Voter(c) => *c as usize,
        Sortition::Transaction => m,
        Sortition::Proposer => m + 1,
        Sortition::Fail => unreachable!("caller must check for Fail before calling slot_for"),
    };
    (slot, &tmpl.contents[slot], tmpl.parents[slot])
}

/// Try up to `max_attempts` nonces, starting at `start_nonce`, against `tmpl`.
/// Returns the mined block (if any nonce in range satisfied sortition) and the
/// next nonce the caller should resume from -- callers MUST carry this forward
/// rather than restarting at `start_nonce` on every call, or they will re-search
/// the exact same (already-failed) range forever.
pub fn try_mine(
    tmpl: &MinerTemplate,
    params: &Params,
    start_nonce: u64,
    max_attempts: u64,
) -> (Option<PrismBlock>, u64) {
    let m = params.m_voter_chains as usize;
    let parent_leaves: Vec<H256> = tmpl.parents.iter().map(parent_leaf).collect();
    let content_hashes: Vec<H256> = tmpl.contents.iter().map(content_hash).collect();
    let parents_root = merkle_root(&parent_leaves);
    let content_root = merkle_root(&content_hashes);

    let mut nonce = start_nonce;
    for _ in 0..max_attempts {
        let header = SuperHeader {
            parents_root,
            content_root,
            timestamp_ms: now_ms(),
            nonce,
            difficulty: tmpl.difficulty,
        };
        let hash = block_hash(&header);
        let sort = sortition(&hash, params);
        nonce = nonce.wrapping_add(1);

        if sort != Sortition::Fail {
            let (slot, content, parent) = slot_for(&sort, m, tmpl);
            let parent_proof = merkle_proof(&parent_leaves, slot);
            let content_proof = merkle_proof(&content_hashes, slot);
            let block = PrismBlock {
                header,
                parent,
                content: content.clone(),
                parent_proof,
                content_proof,
            };
            return (Some(block), nonce);
        }
    }
    (None, nonce)
}

pub struct PrismMiner {
    /// B-2: swappable, not a frozen `Arc<Params>` -- see `refresh`'s doc
    /// comment for why a retargeted `Params` has to reach this, not just the
    /// template's `difficulty` field.
    params: Arc<ArcSwap<Params>>,
    template: Arc<ArcSwap<MinerTemplate>>,
}

impl PrismMiner {
    pub fn new(params: Params) -> Self {
        let m = params.m_voter_chains as usize;
        let genesis = genesis_sentinel();
        let default_tmpl = MinerTemplate {
            parents: vec![genesis; m + 2],
            contents: (0..m + 2)
                .map(|i| {
                    if i < m {
                        BlockContent::Voter { chain: i as u16, voter_parent: genesis, votes: vec![] }
                    } else if i == m {
                        BlockContent::Transaction { txs: vec![] }
                    } else {
                        BlockContent::Proposer { tx_refs: vec![], prop_refs: vec![] }
                    }
                })
                .collect(),
            difficulty: params.difficulty,
        };
        PrismMiner {
            params: Arc::new(ArcSwap::from(Arc::new(params))),
            template: Arc::new(ArcSwap::from(Arc::new(default_tmpl))),
        }
    }

    /// Swap in a new mining template *and* the `Params` it was built
    /// against (wait-free from mining threads). Both together, not just the
    /// template: `try_mine`'s actual accept/reject decision is
    /// `sortition(hash, params) != Fail` -- the template's own `difficulty`
    /// field is only ever stamped into the mined header for *other* nodes'
    /// later `validate_block` check, never consulted by the miner itself.
    /// Before B-2 this didn't matter (`params` never changed after
    /// construction); now that `client/prism-chain`'s `DifficultyRetarget`
    /// can change it over time, passing a live `Params` into
    /// `build_template` alone -- without also updating what the miner's own
    /// `try_mine` gates against -- would let `header.difficulty` (from the
    /// new target) silently disagree with the threshold actually used to
    /// accept the block (the stale one), which a validating peer's `hash <=
    /// header.difficulty` check could then incorrectly reject or accept.
    pub fn refresh(&self, template: MinerTemplate, params: Params) {
        self.params.store(Arc::new(params));
        self.template.store(Arc::new(template));
    }

    /// Spawn a mining thread. Calls `on_found` with every valid block.
    pub fn spawn_mining_thread(
        self: Arc<Self>,
        thread_id: u64,
        on_found: impl Fn(PrismBlock) + Send + 'static,
    ) {
        std::thread::spawn(move || {
            let mut nonce: u64 = thread_id;
            loop {
                let tmpl = self.template.load();
                let params = self.params.load();
                let (found, next_nonce) = try_mine(&tmpl, &params, nonce, 100_000);
                nonce = next_nonce;
                if let Some(block) = found {
                    on_found(block);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests;
