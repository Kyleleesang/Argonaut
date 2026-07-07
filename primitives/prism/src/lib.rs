//! Prism consensus primitive types, sortition, Merkle helpers, and block validation.
//!
//! All types are no_std + SCALE + BLAKE3. No serde. No std::hash. No Substrate BlockT.
//! Prism blocks are NOT Substrate blocks — they live entirely client-side.
//!
//! Reference: Bagaria et al., "Prism: Deconstructing the Blockchain to Approach
//! Physical Limits", arXiv:1810.08092. Sortition follows paper Alg.1 lines 41-47.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use parity_scale_codec::{Decode, Encode};
use scale_info::TypeInfo;
use sp_core::U256;

pub type H256 = sp_core::H256;

/// Voter chain index, 0..m-1.
pub type ChainId = u16;

/// Network parameters stored in chain spec, never hardcoded in block structs.
#[derive(Clone, Debug)]
pub struct Params {
    /// Number of voter chains (100 for testnet, 1000 for mainnet per paper §4.2).
    pub m_voter_chains: u16,
    /// PoW difficulty threshold.
    pub difficulty: U256,
    /// Difficulty fraction per voter-chain block (determines target mining rate).
    pub f_v: U256,
    /// Difficulty fraction for tx blocks.
    pub f_t: U256,
    /// Difficulty fraction for proposer blocks.
    pub f_p: U256,
    /// Target combined interval (any of the m+2 slots) between two Prism
    /// blocks, in milliseconds -- what [`DifficultyRetarget`] retargets
    /// `difficulty`/`f_v`/`f_t`/`f_p` toward as it observes the network's
    /// actual mining rate (architecture guide §3.5: "retarget like Bitcoin
    /// ... one shared retarget"). This value itself is a policy choice
    /// (roughly the guide's "round/delay target D ~= 1s"), not derived from
    /// the paper's §4.2 physical-parameter formula -- that formula needs
    /// real bandwidth-delay-product and block-size measurements this
    /// project doesn't have. `difficulty` above is only ever the *initial*
    /// guess retargeting starts correcting from, same as Bitcoin's genesis
    /// difficulty.
    pub target_interval_ms: u64,
    /// B-5: domain-separates every network's Prism DAG -- fed into
    /// [`genesis_hash`] to derive each voter chain's and the proposer
    /// tree's actual genesis hash, so a block mined against network A's
    /// genesis has no valid ancestry on network B. Set from the Substrate
    /// chain's own `client.chain_info().genesis_hash` in
    /// `node/src/service.rs`; tests use a fixed dummy (commonly
    /// `H256::zero()`, which is fine as a *value* now that it's just an
    /// input to `genesis_hash` rather than the genesis hash itself).
    pub chain_spec_genesis: H256,
}

impl Params {
    /// Testnet defaults per architecture guide §3.7:
    /// m=100, ~1 block/10s per tree, equal f_v : f_t : f_p split.
    pub fn testnet() -> Self {
        let m: u16 = 100;
        let difficulty = U256::max_value() >> 20;
        let total_slots = U256::from(m + 2);
        let f_v = difficulty / total_slots;
        let f_t = f_v;
        let f_p = f_v;
        Params {
            m_voter_chains: m,
            difficulty,
            f_v,
            f_t,
            f_p,
            target_interval_ms: 1_000,
            chain_spec_genesis: H256::zero(),
        }
    }

    /// Recompute `difficulty`/`f_v`/`f_t`/`f_p` for a new *combined* target,
    /// keeping the m+2-way equal split invariant `testnet()` establishes.
    /// This is the one and only place that should ever change `difficulty`
    /// after construction -- doing it here (rather than assigning `difficulty`
    /// directly at call sites) is what prevents reintroducing the m/f_v
    /// mismatch bug found earlier this session (a client-side tuning attempt
    /// that changed `m_voter_chains` without rederiving `f_v`/`f_t`/`f_p` from
    /// it, silently shrinking the real mining success range by ~14x).
    pub fn set_combined_target(&mut self, new_target: U256) {
        let total_slots = U256::from(self.m_voter_chains + 2);
        let per_slot = new_target / total_slots;
        self.difficulty = new_target;
        self.f_v = per_slot;
        self.f_t = per_slot;
        self.f_p = per_slot;
    }
}

// ── Difficulty retargeting (B-2) ──────────────────────────────────────────────

/// Number of superblocks (any of the m+2 slots) averaged per retarget
/// decision. Bitcoin uses 2016; this is far smaller because Prism blocks are
/// found far more often (fine-grained per-slot mining, not one block per ~10
/// minutes) -- a smaller window keeps retargeting responsive without
/// needing to wait an impractically long time to react to a real hash-rate
/// change.
pub const RETARGET_WINDOW: u64 = 20;

/// Tracks a rolling window of block timestamps and proposes a new combined
/// difficulty target once per `RETARGET_WINDOW` blocks -- the client-side
/// analog of Bitcoin's rolling-window retarget (architecture guide §3.5).
/// Deliberately simple: no persistence of the window itself (only the
/// resulting target is persisted, by the caller -- see
/// `client/prism-chain`), so a restart loses at most one window's worth of
/// smoothing, not correctness.
#[derive(Clone, Debug, Default)]
pub struct DifficultyRetarget {
    timestamps: VecDeque<u64>,
}

impl DifficultyRetarget {
    pub fn new() -> Self {
        Self { timestamps: VecDeque::with_capacity(RETARGET_WINDOW as usize + 1) }
    }

    /// Feed a newly-accepted block's timestamp (any content type -- the
    /// *combined* mining rate across all m+2 slots is what `difficulty`
    /// gates; sortition only splits an already-successful hash afterward).
    /// Returns the new combined target once every `RETARGET_WINDOW` blocks;
    /// `None` otherwise (not enough samples yet, or no time signal to act
    /// on -- see below).
    pub fn observe(
        &mut self,
        timestamp_ms: u64,
        target_interval_ms: u64,
        current_target: U256,
    ) -> Option<U256> {
        self.timestamps.push_back(timestamp_ms);
        if self.timestamps.len() as u64 <= RETARGET_WINDOW {
            return None;
        }
        let oldest = *self.timestamps.front().expect("just checked len > 0");
        let newest = *self.timestamps.back().expect("just checked len > 0");
        self.timestamps.pop_front();

        if newest <= oldest {
            // No usable time signal -- e.g. synthetic/scripted blocks that
            // don't set real timestamps (as in prism-chain's own unit
            // tests), or (live) a run of blocks sharing a timestamp at
            // millisecond granularity. Leave the target unchanged rather
            // than dividing by a ~0 elapsed time and producing a spurious
            // spike.
            return None;
        }

        let actual_elapsed = newest - oldest;
        let expected_elapsed = target_interval_ms.saturating_mul(RETARGET_WINDOW).max(1);

        // Clamp the *ratio* to [1/4, 4] (Bitcoin-style) by clamping the
        // elapsed time fed into it, so one adjustment can't swing difficulty
        // by more than 4x in either direction -- resists a single
        // fast-forking miner's manipulated timestamps from causing a wild
        // difficulty swing.
        let clamped_actual =
            actual_elapsed.max(expected_elapsed / 4).min(expected_elapsed.saturating_mul(4));

        let new_target = current_target
            .saturating_mul(U256::from(clamped_actual))
            .checked_div(U256::from(expected_elapsed))
            .unwrap_or(current_target)
            .max(U256::one()); // never let the target collapse to 0 (impossible to mine)

        Some(new_target)
    }
}

// ── Block types ───────────────────────────────────────────────────────────────

/// PoW superblock header shared by all Prism block variants.
/// Parent and content hashes are committed via Merkle roots, not inline.
#[derive(Encode, Decode, TypeInfo, Clone, PartialEq, Eq, Debug)]
pub struct SuperHeader {
    /// Merkle root over m+2 parent hashes: voter 0..m-1, tx, proposer (in order).
    pub parents_root: H256,
    /// Merkle root over m+2 content hashes (same slot order).
    pub content_root: H256,
    pub timestamp_ms: u64,
    pub nonce: u64,
    /// The difficulty threshold this block was mined against.
    pub difficulty: U256,
}

/// Typed content for each Prism block variant.
#[derive(Encode, Decode, TypeInfo, Clone, PartialEq, Eq, Debug)]
pub enum BlockContent {
    /// Tx block: carries opaque SCALE-encoded extrinsics (max B_t = 32 KB).
    Transaction { txs: Vec<Vec<u8>> },
    /// Proposer block: references tx blocks and other proposers to include.
    Proposer { tx_refs: Vec<H256>, prop_refs: Vec<H256> },
    /// Voter block: votes on proposer levels on a specific voter chain.
    Voter {
        chain: ChainId,
        voter_parent: H256,
        /// Proposer hashes this block votes on — one per unvoted level, strictly ascending.
        votes: Vec<H256>,
    },
}

/// Merkle membership proof: sibling path from leaf to root.
/// Each tuple is (sibling_hash, sibling_is_right_of_current).
#[derive(Encode, Decode, TypeInfo, Clone, PartialEq, Eq, Debug, Default)]
pub struct MerkleProof(pub Vec<(H256, bool)>);

/// A complete Prism block: superheader + typed content + two Merkle membership proofs.
#[derive(Encode, Decode, TypeInfo, Clone, PartialEq, Eq, Debug)]
pub struct PrismBlock {
    pub header: SuperHeader,
    /// This block's own parent (voter-chain parent for voters; proposer parent for others).
    pub parent: H256,
    pub content: BlockContent,
    /// Proves `parent` at its slot index under `header.parents_root`.
    pub parent_proof: MerkleProof,
    /// Proves hash(SCALE(content)) at its slot index under `header.content_root`.
    pub content_proof: MerkleProof,
}

// ── Hash and sortition ────────────────────────────────────────────────────────

/// Compute the canonical block hash: blake3(SCALE(header)).
/// Used as the PoW hash and the block's content-addressed identity.
pub fn block_hash(header: &SuperHeader) -> H256 {
    let encoded = header.encode();
    let h = blake3::hash(&encoded);
    H256::from_slice(h.as_bytes())
}

/// Sortition result — maps a PoW hash to a Prism block type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sortition {
    Voter(ChainId),
    Transaction,
    Proposer,
    /// Hash falls above all thresholds — not a valid block.
    Fail,
}

/// Map a PoW hash to a block type per Prism paper Alg.1 §3.
///
/// Thresholds (h interpreted as U256 big-endian):
///   h < m*f_v                        → Voter(h / f_v)
///   m*f_v ≤ h < m*f_v + f_t          → Transaction
///   m*f_v+f_t ≤ h < m*f_v+f_t+f_p   → Proposer
///   else                             → Fail
pub fn sortition(hash: &H256, params: &Params) -> Sortition {
    let h = U256::from_big_endian(hash.as_bytes());
    let m = U256::from(params.m_voter_chains);

    let voter_range = params.f_v.saturating_mul(m);
    let tx_upper = voter_range.saturating_add(params.f_t);
    let prop_upper = tx_upper.saturating_add(params.f_p);

    if h < voter_range {
        let chain_idx = (h / params.f_v).low_u32() as u16;
        Sortition::Voter(chain_idx.min(params.m_voter_chains - 1))
    } else if h < tx_upper {
        Sortition::Transaction
    } else if h < prop_upper {
        Sortition::Proposer
    } else {
        Sortition::Fail
    }
}

// ── Merkle tree (binary blake3, ~80 lines) ───────────────────────────────────

fn hash_pair(left: &H256, right: &H256) -> H256 {
    let mut buf = [0u8; 64];
    buf[..32].copy_from_slice(left.as_bytes());
    buf[32..].copy_from_slice(right.as_bytes());
    H256::from_slice(blake3::hash(&buf).as_bytes())
}

/// Build a binary blake3 Merkle tree over `leaves` and return the root.
pub fn merkle_root(leaves: &[H256]) -> H256 {
    if leaves.is_empty() {
        return H256::zero();
    }
    let mut level: Vec<H256> = leaves.to_vec();
    while level.len() > 1 {
        let mut next = Vec::with_capacity((level.len() + 1) / 2);
        let mut i = 0;
        while i < level.len() {
            let right = if i + 1 < level.len() { level[i + 1] } else { level[i] };
            next.push(hash_pair(&level[i], &right));
            i += 2;
        }
        level = next;
    }
    level[0]
}

/// Generate a Merkle proof for the leaf at `index` in `leaves`.
pub fn merkle_proof(leaves: &[H256], index: usize) -> MerkleProof {
    let mut proof = Vec::new();
    let mut level: Vec<H256> = leaves.to_vec();
    let mut idx = index;

    while level.len() > 1 {
        let sibling_is_right = idx % 2 == 0;
        let sibling_idx = if sibling_is_right { idx + 1 } else { idx - 1 };
        let sibling = if sibling_idx < level.len() { level[sibling_idx] } else { level[idx] };
        proof.push((sibling, sibling_is_right));

        let mut next = Vec::with_capacity((level.len() + 1) / 2);
        let mut i = 0;
        while i < level.len() {
            let right = if i + 1 < level.len() { level[i + 1] } else { level[i] };
            next.push(hash_pair(&level[i], &right));
            i += 2;
        }
        level = next;
        idx /= 2;
    }
    MerkleProof(proof)
}

/// Verify a Merkle proof: does this path from `leaf` match `root`?
pub fn verify_merkle_proof(root: &H256, leaf: &H256, proof: &MerkleProof) -> bool {
    let mut current = *leaf;
    for (sibling, sibling_is_right) in &proof.0 {
        current = if *sibling_is_right {
            hash_pair(&current, sibling)
        } else {
            hash_pair(sibling, &current)
        };
    }
    &current == root
}

// ── Block validation ──────────────────────────────────────────────────────────

/// Errors returned by `validate_block`.
#[derive(Debug, PartialEq, Eq)]
pub enum ValidationError {
    /// block_hash > difficulty.
    PowCheckFailed,
    /// The sortition range of the hash does not match the claimed content type.
    SortitionMismatch,
    /// The parent Merkle proof does not verify under parents_root.
    InvalidParentProof,
    /// The content Merkle proof does not verify under content_root.
    InvalidContentProof,
    /// Voter block votes are not strictly ascending.
    VotesNotAscending,
    /// Transaction block exceeds B_t size limit.
    TxBlockTooLarge,
    /// Voter block claims a chain_id ≥ m.
    InvalidChainId,
}

/// Marker type: a `PrismBlock` that has passed all structural checks.
pub struct Validated(pub PrismBlock);

/// Transaction block size limit (32 KB per §3.7).
pub const MAX_TX_BLOCK_BYTES: usize = 32 * 1024;

/// Validate an incoming Prism block.
/// Checks: PoW, sortition consistency, Merkle proofs, content-specific sanity.
pub fn validate_block(block: &PrismBlock, params: &Params) -> Result<Validated, ValidationError> {
    let hash = block_hash(&block.header);

    // PoW check
    if U256::from_big_endian(hash.as_bytes()) > block.header.difficulty {
        return Err(ValidationError::PowCheckFailed);
    }

    // Sortition consistency
    match (sortition(&hash, params), &block.content) {
        (Sortition::Voter(c), BlockContent::Voter { chain, .. }) if c == *chain => {}
        (Sortition::Transaction, BlockContent::Transaction { .. }) => {}
        (Sortition::Proposer, BlockContent::Proposer { .. }) => {}
        _ => return Err(ValidationError::SortitionMismatch),
    }

    // Content-specific sanity checks
    match &block.content {
        BlockContent::Voter { chain, votes, .. } => {
            if *chain >= params.m_voter_chains {
                return Err(ValidationError::InvalidChainId);
            }
            for w in votes.windows(2) {
                if w[0] >= w[1] {
                    return Err(ValidationError::VotesNotAscending);
                }
            }
        }
        BlockContent::Transaction { txs } => {
            let total: usize = txs.iter().map(|t| t.len()).sum();
            if total > MAX_TX_BLOCK_BYTES {
                return Err(ValidationError::TxBlockTooLarge);
            }
        }
        BlockContent::Proposer { .. } => {}
    }

    // Merkle proof for parent
    let parent_leaf = H256::from_slice(blake3::hash(block.parent.as_bytes()).as_bytes());
    if !verify_merkle_proof(&block.header.parents_root, &parent_leaf, &block.parent_proof) {
        return Err(ValidationError::InvalidParentProof);
    }

    // Merkle proof for content
    let content_encoded = block.content.encode();
    let content_leaf = H256::from_slice(blake3::hash(&content_encoded).as_bytes());
    if !verify_merkle_proof(&block.header.content_root, &content_leaf, &block.content_proof) {
        return Err(ValidationError::InvalidContentProof);
    }

    Ok(Validated(block.clone()))
}

/// Compute a deterministic genesis block hash for a given chain slot.
/// B-5: every voter chain (slot `0..m-1`) and the proposer tree (slot `m`)
/// gets a distinct genesis, domain-separated by `chain_spec_genesis` so a
/// block mined against one network's genesis has no valid ancestry on
/// another's. Tx blocks parent onto proposers and need no genesis slot of
/// their own -- only `m + 1` distinct geneses exist, not `m + 2`.
pub fn genesis_hash(chain_spec_genesis: &H256, slot: u16) -> H256 {
    let mut buf = [0u8; 34];
    buf[..32].copy_from_slice(chain_spec_genesis.as_bytes());
    buf[32..].copy_from_slice(&slot.to_le_bytes());
    H256::from_slice(blake3::hash(&buf).as_bytes())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_h256(b: u8) -> H256 {
        H256::from([b; 32])
    }

    #[test]
    fn merkle_root_single_leaf() {
        let leaves = [dummy_h256(1)];
        assert_eq!(merkle_root(&leaves), leaves[0]);
    }

    #[test]
    fn merkle_proof_round_trip() {
        let leaves: Vec<H256> = (0u8..4).map(dummy_h256).collect();
        let root = merkle_root(&leaves);
        for i in 0..leaves.len() {
            let proof = merkle_proof(&leaves, i);
            let leaf = leaves[i];
            assert!(verify_merkle_proof(&root, &leaf, &proof), "proof failed for index {i}");
        }
    }

    #[test]
    fn sortition_ranges_are_disjoint() {
        let params = Params::testnet();
        // Hash = 0: should be Voter(0)
        let hash_zero = H256::zero();
        assert_eq!(sortition(&hash_zero, &params), Sortition::Voter(0));
        // Hash = max: should be Fail
        let hash_max = H256::from([0xff; 32]);
        assert_eq!(sortition(&hash_max, &params), Sortition::Fail);
    }

    #[test]
    fn block_hash_is_deterministic() {
        let h = SuperHeader {
            parents_root: H256::zero(),
            content_root: H256::zero(),
            timestamp_ms: 1000,
            nonce: 42,
            difficulty: U256::from(100u32),
        };
        assert_eq!(block_hash(&h), block_hash(&h));
    }

    #[test]
    fn set_combined_target_preserves_equal_split() {
        let mut p = Params::testnet();
        let new_target = U256::from(700u32);
        p.set_combined_target(new_target);
        assert_eq!(p.difficulty, new_target);
        assert_eq!(p.f_v, U256::from(700u32 / (100 + 2)));
        assert_eq!(p.f_v, p.f_t);
        assert_eq!(p.f_v, p.f_p);
    }

    #[test]
    fn retarget_returns_none_before_the_window_fills() {
        let mut r = DifficultyRetarget::new();
        for i in 0..RETARGET_WINDOW {
            assert_eq!(r.observe(1000 * i, 1000, U256::from(1000u32)), None);
        }
    }

    #[test]
    fn retarget_holds_steady_when_actual_matches_expected() {
        let mut r = DifficultyRetarget::new();
        let target = U256::from(1_000_000u32);
        let mut result = None;
        for i in 0..=RETARGET_WINDOW {
            // Exactly on-target: one block every 1000ms.
            result = r.observe(1000 * i, 1000, target);
        }
        assert_eq!(result, Some(target));
    }

    #[test]
    fn retarget_shrinks_target_when_blocks_come_too_fast() {
        let mut r = DifficultyRetarget::new();
        let target = U256::from(1_000_000u32);
        let mut result = None;
        for i in 0..=RETARGET_WINDOW {
            // Blocks arriving twice as fast as the 1000ms target.
            result = r.observe(500 * i, 1000, target);
        }
        // Too-fast blocks -> tighten the target (smaller = harder to hit).
        assert!(result.unwrap() < target);
    }

    #[test]
    fn retarget_grows_target_when_blocks_come_too_slow() {
        let mut r = DifficultyRetarget::new();
        let target = U256::from(1_000_000u32);
        let mut result = None;
        for i in 0..=RETARGET_WINDOW {
            // Blocks arriving twice as slowly as the 1000ms target.
            result = r.observe(2000 * i, 1000, target);
        }
        assert!(result.unwrap() > target);
    }

    #[test]
    fn retarget_clamps_extreme_ratios_to_4x() {
        let mut r = DifficultyRetarget::new();
        let target = U256::from(1_000_000u32);
        let mut result = None;
        for i in 0..=RETARGET_WINDOW {
            // Blocks arriving 100x slower than target -- would imply a 100x
            // easier target uncapped; must clamp to exactly 4x.
            result = r.observe(100_000 * i, 1000, target);
        }
        assert_eq!(result.unwrap(), target * 4);
    }

    #[test]
    fn retarget_ignores_non_advancing_timestamps() {
        // Matches this crate's own test convention elsewhere (SuperHeader
        // built with a fixed timestamp) -- must never fire on these.
        let mut r = DifficultyRetarget::new();
        let target = U256::from(1_000_000u32);
        for _ in 0..(RETARGET_WINDOW * 3) {
            assert_eq!(r.observe(0, 1000, target), None);
        }
    }
}
