//! Prism PoW mining loop.
//!
//! Mines all m+2 Prism block types simultaneously from a shared superheader.
//! Template is rebuilt atomically via ArcSwap whenever the DAG or pool changes.
//!
//! # Queue coloring (§3.5)
//! Tx-block content is drawn from one of q=8 queues (keyed by blake3(tx) % q).
//! One queue is randomly selected per template refresh. This bounds worst-case
//! tx duplication across concurrent tx blocks.
//!
//! # Threading model
//! One or more mining threads spin on the nonce; each calls `on_found` with
//! the complete PrismBlock when a valid PoW is found. Template swap via ArcSwap
//! is wait-free from the mining threads' perspective.
use argonaut_primitives_prism::{
    block_hash, merkle_proof, merkle_root, sortition, BlockContent, MerkleProof, Params, PrismBlock,
    Sortition, SuperHeader, H256,
};
use arc_swap::ArcSwap;
use parity_scale_codec::Encode;
use sp_core::U256; // pulled from workspace dep sp-core
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// A fully assembled mining template: one header, m+2 content slots.
/// Slot order: voter 0..m-1, tx, proposer (matches parents_root / content_root Merkle leaves).
pub struct MinerTemplate {
    /// Parent hashes for each of the m+2 slots.
    pub parents: Vec<H256>,
    /// Contents for each slot.
    pub contents: Vec<BlockContent>,
    pub difficulty: U256,
}

fn content_hash(c: &BlockContent) -> H256 {
    H256::from_slice(blake3::hash(&c.encode()).as_bytes())
}

fn parent_leaf(h: &H256) -> H256 {
    H256::from_slice(blake3::hash(h.as_bytes()).as_bytes())
}

pub struct PrismMiner {
    pub params: Arc<Params>,
    template: Arc<ArcSwap<MinerTemplate>>,
}

impl PrismMiner {
    pub fn new(params: Params) -> Self {
        let params = Arc::new(params);
        let m = params.m_voter_chains as usize;
        let default_tmpl = MinerTemplate {
            parents: vec![H256::zero(); m + 2],
            contents: (0..m + 2).map(|i| {
                if i < m {
                    BlockContent::Voter {
                        chain: i as u16,
                        voter_parent: H256::zero(),
                        votes: vec![],
                    }
                } else if i == m {
                    BlockContent::Transaction { txs: vec![] }
                } else {
                    BlockContent::Proposer { tx_refs: vec![], prop_refs: vec![] }
                }
            }).collect(),
            difficulty: params.difficulty,
        };
        PrismMiner {
            params,
            template: Arc::new(ArcSwap::from(Arc::new(default_tmpl))),
        }
    }

    /// Swap in a new mining template (wait-free from mining threads).
    pub fn refresh(&self, template: MinerTemplate) {
        self.template.store(Arc::new(template));
    }

    /// Spawn a mining thread. Calls `on_found` with every valid block.
    /// Mining threads use `std::thread` (not async) per spec §3.5.
    pub fn spawn_mining_thread(
        self: Arc<Self>,
        thread_id: u64,
        on_found: impl Fn(PrismBlock) + Send + 'static,
    ) {
        let miner = self.clone();
        std::thread::spawn(move || {
            let mut nonce: u64 = thread_id;
            loop {
                let tmpl = miner.template.load();
                let m = miner.params.m_voter_chains as usize;

                // Build Merkle roots
                let parent_leaves: Vec<H256> = tmpl.parents.iter().map(parent_leaf).collect();
                let content_hashes: Vec<H256> = tmpl.contents.iter().map(content_hash).collect();
                let parents_root = merkle_root(&parent_leaves);
                let content_root = merkle_root(&content_hashes);

                let ts = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);

                let header = SuperHeader {
                    parents_root,
                    content_root,
                    timestamp_ms: ts,
                    nonce,
                    difficulty: tmpl.difficulty,
                };

                let hash = block_hash(&header);
                let sort = sortition(&hash, &miner.params);

                if sort != Sortition::Fail {
                    let (slot, content, parent) = match &sort {
                        Sortition::Voter(c) => {
                            let s = *c as usize;
                            (s, tmpl.contents[s].clone(), tmpl.parents[s])
                        }
                        Sortition::Transaction => {
                            (m, tmpl.contents[m].clone(), tmpl.parents[m])
                        }
                        Sortition::Proposer => {
                            (m + 1, tmpl.contents[m + 1].clone(), tmpl.parents[m + 1])
                        }
                        Sortition::Fail => unreachable!(),
                    };

                    let parent_proof = merkle_proof(&parent_leaves, slot);
                    let content_proof_leaves: Vec<H256> = content_hashes.clone();
                    let content_proof = merkle_proof(&content_proof_leaves, slot);

                    let block = PrismBlock {
                        header,
                        parent,
                        content,
                        parent_proof,
                        content_proof,
                    };
                    on_found(block);
                }

                nonce = nonce.wrapping_add(1);
            }
        });
    }
}
