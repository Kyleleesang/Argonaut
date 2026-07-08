//! Implementation D (decentralized storage platform) shared primitives.
//!
//! This crate is the single home for everything the storage pallets, the
//! provider daemon, and the client SDK must agree on byte-for-byte:
//!
//! - on-chain / on-wire types (`ShardSpec`, `DealParams`, `Manifest`, `Voucher`);
//! - the **deterministic challenge-selection formulas** (D.3.2) — one impl used by
//!   both the runtime (to decide which chunk a provider must prove) and the daemon
//!   (to decide which chunk to prove) so they can never disagree;
//! - the **`BaoProof` host function** (D.3.3) — native bao-tree slice verification,
//!   registered as `bao_proof::HostFunctions` in `node/src/service.rs` exactly like
//!   `falcon_crypto::HostFunctions`.
//!
//! See `docs/IMPLEMENTATION_D_SPEC.md` §D.3. All integer math here is total: every
//! op is a `%`, a bounded `*`, or a `checked_add`. Little-endian everywhere.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::vec::Vec;
use parity_scale_codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_core::{bounded::BoundedVec, ConstU32};
use sp_runtime_interface::runtime_interface;

// ── Type aliases ────────────────────────────────────────────────────────────

pub type BlobRoot = [u8; 32]; // BLAKE3 root == iroh-blobs content id
pub type DealId = u64;
pub type ChannelId = u64;
pub type EpochIndex = u32;
pub type ShardIndex = u16;

// ── Constants ───────────────────────────────────────────────────────────────

/// Bao leaf granularity. NEVER change: it defines the challenged-chunk space.
pub const CHUNK_SIZE: u32 = 1024;
/// Upper bound on a proof slice, checked BEFORE host dispatch.
pub const MAX_PROOF_BYTES: u32 = 4096;
/// `BoundedVec` bound for `DealParams.shards` (== n).
pub const MAX_SHARDS: u32 = 64;

// Domain-separation prefixes (blake3 input framing). Use verbatim.
pub const DS_SELECT: &[u8] = b"argn/store/select/v1";
pub const DS_CHUNK: &[u8] = b"argn/store/chunk/v1";
pub const DS_ACTIVATE: &[u8] = b"argn/store/activate/v1";
pub const DS_VOUCHER: &[u8] = b"argn/voucher/v1";

// ── On-chain deal types ─────────────────────────────────────────────────────

/// One erasure-coded shard: its BLAKE3 root and byte size (`size % 1024 == 0`, `> 0`).
#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, PartialEq, Eq, Debug)]
pub struct ShardSpec {
    pub root: BlobRoot,
    pub size: u64,
}

/// Parameters of a storage deal. Generic over the runtime's `Balance` type.
#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, PartialEq, Eq, Debug)]
pub struct DealParams<Balance> {
    /// SCALE [`Manifest`] blob root (NOT an iroh HashSeq — deviation D.13.3).
    pub manifest: BlobRoot,
    /// One entry per shard; `len() == n`.
    pub shards: BoundedVec<ShardSpec, ConstU32<MAX_SHARDS>>,
    /// RS data shards; `1 <= k < n`.
    pub k: u8,
    /// Total shards; `== shards.len()`.
    pub n: u8,
    /// Vesting horizon, in epochs from activation.
    pub duration_epochs: EpochIndex,
    pub price_per_shard_epoch: Balance,
    /// P(challenge per shard per epoch) in parts-per-million, `<= 1_000_000`.
    pub challenge_rate_ppm: u32,
}

// ── Deterministic selection formulas (D.3.2) ────────────────────────────────

/// Number of 1024-byte chunks in a shard. `size` is validated `% 1024 == 0`.
pub fn num_chunks(size: u64) -> u64 {
    size / CHUNK_SIZE as u64
}

/// blake3 of the concatenated parts, first 8 bytes as a little-endian u64.
fn h64(parts: &[&[u8]]) -> u64 {
    let mut hasher = blake3::Hasher::new();
    for p in parts {
        hasher.update(p);
    }
    let mut b = [0u8; 8];
    b.copy_from_slice(&hasher.finalize().as_bytes()[..8]);
    u64::from_le_bytes(b)
}

/// Is shard `(deal, idx)` challenged in the epoch whose beacon is `beacon`?
pub fn is_selected(beacon: &[u8; 32], deal: DealId, idx: ShardIndex, rate_ppm: u32) -> bool {
    h64(&[DS_SELECT, beacon, &deal.to_le_bytes(), &idx.to_le_bytes()]) % 1_000_000
        < rate_ppm as u64
}

/// Which chunk must be proven (only meaningful when [`is_selected`] is true).
pub fn challenged_chunk(beacon: &[u8; 32], deal: DealId, idx: ShardIndex, size: u64) -> u64 {
    h64(&[DS_CHUNK, beacon, &deal.to_le_bytes(), &idx.to_le_bytes()]) % num_chunks(size)
}

/// Activation challenge chunk (proves possession before escrow starts vesting).
pub fn activation_chunk(beacon: &[u8; 32], deal: DealId, idx: ShardIndex, size: u64) -> u64 {
    h64(&[DS_ACTIVATE, beacon, &deal.to_le_bytes(), &idx.to_le_bytes()]) % num_chunks(size)
}

// ── Manifest (client-side; on-chain only as its BLAKE3 root) ─────────────────

/// Stored as a plain iroh blob whose BLAKE3 root `== DealParams.manifest`.
/// Unbounded `Vec<BlobRoot>`: this type NEVER enters runtime storage — it lives
/// off-chain and is referenced on-chain only by its 32-byte root.
#[derive(Encode, Decode, TypeInfo, Clone, PartialEq, Eq, Debug)]
pub struct Manifest {
    pub version: u8, // = 1
    pub file_size: u64,
    pub file_hash: [u8; 32],
    pub k: u8,
    pub n: u8,
    pub shard_size: u64, // identical for all shards; % 1024 == 0
    pub shards: Vec<BlobRoot>, // len n; index i == ShardIndex i == RS shard i
}

pub const MANIFEST_VERSION: u8 = 1;

// ── Payment-channel voucher (D.7) ───────────────────────────────────────────

/// Off-chain monotone-cumulative micropayment authorization. Only the final one
/// per channel ever hits the chain (`payment-channels::settle`).
#[derive(Encode, Decode, TypeInfo, Clone, PartialEq, Eq, Debug)]
pub struct Voucher {
    pub channel_id: ChannelId,
    pub cumulative: u128,
}

/// Signed payload = `blake3(DS_VOUCHER ++ scale(Voucher))` — matches the §2
/// convention of signing 32-byte blake3 digests.
pub fn voucher_signing_payload(v: &Voucher) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(DS_VOUCHER);
    h.update(&v.encode());
    *h.finalize().as_bytes()
}

// ── BaoProof host function (D.3.3) ──────────────────────────────────────────

/// Native bao-tree slice verification. Register `bao_proof::HostFunctions` in the
/// node executor's `ExtendHostFunctions` tuple (`node/src/service.rs`), next to
/// `falcon_crypto::HostFunctions`. Nothing here executes inside wasm; wasm calls
/// compile to host-function dispatch.
#[runtime_interface]
pub trait BaoProof {
    /// Verify a bao slice proving possession of chunk range
    /// `[chunk_start, chunk_start + chunk_len)` of the blob `(root, total_size)`.
    ///
    /// `slice` is EXACTLY the bytes produced by `bao_tree`'s
    /// `encode_ranges_validated` for that range — nothing prepended.
    /// Returns `false` on ANY error; never panics, never returns `Result`.
    ///
    /// Callers MUST bounds-check first (D.3.3): `slice.len() <= MAX_PROOF_BYTES`,
    /// `chunk_len == 1`, `chunk_start < num_chunks(total_size)`.
    fn verify_slice(
        root: &[u8; 32],
        total_size: u64,
        chunk_start: u64,
        chunk_len: u32,
        slice: &[u8],
    ) -> bool {
        use bao_tree::{
            io::sync::DecodeResponseIter, io::BaoContentItem, BaoTree, BlockSize, ChunkNum,
            ChunkRanges,
        };

        let end = match chunk_start.checked_add(chunk_len as u64) {
            Some(e) => e,
            None => return false,
        };
        // Chunk-level verification (BlockSize::ZERO) is independent of whatever
        // chunk-group outboard the provider stores — same root either way.
        let tree = BaoTree::new(total_size, BlockSize::ZERO);
        let root_hash = bao_tree::blake3::Hash::from(*root);
        let ranges = ChunkRanges::from(ChunkNum(chunk_start)..ChunkNum(end));
        let iter = DecodeResponseIter::new(root_hash, tree, slice, ranges.as_ref());

        let mut saw_leaf = false;
        for item in iter {
            match item {
                Ok(BaoContentItem::Leaf(_)) => saw_leaf = true,
                Ok(BaoContentItem::Parent(_)) => {}
                Err(_) => return false, // hash mismatch / truncation / bad framing
            }
        }
        // The range must actually have been delivered and validated.
        saw_leaf
    }
}

#[cfg(test)]
mod tests;
