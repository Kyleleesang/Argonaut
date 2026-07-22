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

// ── D4 retrieval + shard delivery wire protocols (off-chain, over iroh) ─────
//
// SCALE-encoded messages for two custom iroh protocols the D3/D4 daemon and
// client speak directly (not the stock iroh-blobs blob-transfer protocol,
// which has no concept of per-stride payment gating): shard delivery
// (client -> provider, after `accept_shard`) and gated retrieval (client <->
// provider, metered by payment-channel vouchers, spec §D.9 gate.rs /
// architecture guide §9.6). Both message sets live here rather than in the
// daemon or client crate because both sides need identical wire types.

/// Billing quantum for retrieval (spec §D.9 gate.rs): a client may fetch at
/// most this many bytes before the provider requires a fresh voucher
/// covering everything served so far. 1 MiB, per the spec's own choice.
pub const VOUCHER_STRIDE: u64 = 1024 * 1024;

/// ALPN identifiers for the two custom iroh protocols (client and daemon
/// both need the same bytes; kept here rather than in either crate so
/// neither has to depend on the other for just a byte-string constant).
pub const SHARD_PUSH_ALPN: &[u8] = b"argonaut/shard-push/1";
pub const RETRIEVAL_ALPN: &[u8] = b"argonaut/retrieval/1";

/// Client -> provider, once at the start of a shard-push session
/// (`/argonaut/shard-push/1`): "here are the bytes for shard `shard_idx` of
/// `deal_id`". The provider must independently verify
/// `blake3::hash(&bytes) == ` the on-chain `ShardSpec.root` before trusting
/// or storing them -- this message makes no claim about correctness itself.
#[derive(Encode, Decode, TypeInfo, Clone, Debug)]
pub struct ShardPushRequest {
    pub deal_id: DealId,
    pub shard_idx: ShardIndex,
    pub bytes: Vec<u8>,
}

#[derive(Encode, Decode, TypeInfo, Clone, PartialEq, Eq, Debug)]
pub enum ShardPushResponse {
    Ok,
    /// `blake3(bytes)` didn't match the on-chain `ShardSpec.root`.
    RootMismatch,
    /// No `Accepted`-state assignment for `(deal_id, shard_idx)` naming this
    /// provider -- pushing shard bytes nobody asked this provider to hold.
    NotAssigned,
    Error,
}

/// Client -> provider, one per stride, on a long-lived bidirectional stream
/// (`/argonaut/retrieval/1`, spec §D.9). `voucher`/`sig` must cover every
/// byte served so far on this stream, not just this stride -- vouchers are
/// cumulative by design (D.7), so the provider's check is simply
/// `voucher.cumulative >= price_per_stride * (strides_served_so_far + 1)`.
#[derive(Encode, Decode, TypeInfo, Clone, Debug)]
pub struct RetrievalRequest {
    pub channel_id: ChannelId,
    pub root: BlobRoot,
    pub offset: u64,
    /// Requested length; provider caps it at `VOUCHER_STRIDE` regardless of
    /// what's asked, so a client can't get more than one stride's worth of
    /// unpaid-for bytes ahead by requesting a huge range.
    pub length: u32,
    pub voucher: Voucher,
    pub sig: argonaut_primitives_falcon::FalconSignature,
}

#[derive(Encode, Decode, TypeInfo, Clone, Debug)]
pub enum RetrievalResponse {
    /// The requested byte range, exactly `length` bytes (or fewer at EOF).
    Data(Vec<u8>),
    /// Voucher doesn't cover this stride, or its signature/channel didn't
    /// check out -- the client must send a better voucher and retry, not
    /// treat this as fatal (mirrors "serve-then-verify at stride
    /// granularity" from the catalog spec's P-2, applied here first).
    PaymentRequired,
    NoSuchBlob,
    BadRange,
}

// ── Cross-pallet interface traits (D.4 / D.6) ───────────────────────────────
//
// These live here — the neutral shared crate — rather than in any one pallet, so
// that `storage-deals` and `storage-proofs` can reference each other's contracts
// *by trait* without a cyclic crate dependency (Cargo forbids those). Each pallet
// depends only on this crate; the runtime wires the concrete `Pallet` types into
// the associated `Config` slots. Deliberately frame-free (no `DispatchResult`,
// no `frame_support`) so this crate stays lightweight for the daemon/SDK too.

pub mod interfaces {
    use super::{DealId, EpochIndex, ShardIndex};

    /// Implemented by `pallet-provider-registry`; consumed by `storage-deals`
    /// and `storage-proofs` via a `Config` associated type. All mutation happens
    /// inside the registry's own storage; callers only signal transitions.
    pub trait ProviderInterface<AccountId, Balance> {
        /// Is the provider registered and `Active` (not deregistering)?
        fn is_active(who: &AccountId) -> bool;
        /// Stake not currently locked against active assignments (`stake − locked`).
        fn free_stake(who: &AccountId) -> Balance;
        /// Repatriate up to `amount` of the provider's reserved stake to
        /// `beneficiary` as *free* balance; decrement the tracked stake. Returns
        /// the amount actually moved (may be `< amount` if stake ran short).
        fn slash(who: &AccountId, amount: Balance, beneficiary: &AccountId) -> Balance;
        /// Record a new assignment carrying `escrow_at_risk`. Enforces the §9.3
        /// 2× rule (`stake ≥ 2 · (locked + escrow_at_risk)`); `Err(())` if it
        /// would break, or the provider is not active.
        fn note_assignment_started(who: &AccountId, escrow_at_risk: Balance) -> Result<(), ()>;
        /// Release an assignment's `escrow_at_risk` from the locked total and
        /// decrement `active_assignments`. Saturating; never fails.
        fn note_assignment_ended(who: &AccountId, escrow_at_risk: Balance);
        /// Bump the lifetime informational fault counter.
        fn note_fault(who: &AccountId);
    }

    /// Implemented by `storage-proofs` (which owns epoch math and the beacon map);
    /// consumed by `storage-deals` for activation-window checks. `beacon` returns
    /// `None` for pre-genesis / pruned epochs.
    pub trait EpochProvider {
        fn current_epoch() -> EpochIndex;
        fn beacon(epoch: EpochIndex) -> Option<[u8; 32]>;
    }

    /// Read-only view `storage-proofs` needs to validate a proof or miss report,
    /// returned only when the deal is chargeable (`Active`/`Degraded`) and the
    /// assignment exists and is `Activated`. Frame-free / balance-free by design.
    #[derive(Clone, PartialEq, Eq, Debug)]
    pub struct ProofContext<AccountId> {
        pub provider: AccountId,
        pub client: AccountId,
        pub shard_root: [u8; 32],
        pub shard_size: u64,
        pub challenge_rate_ppm: u32,
        pub end_epoch: EpochIndex,
    }

    /// Implemented by `storage-deals`; consumed by `storage-proofs` (read side).
    pub trait DealInspector<AccountId> {
        fn proof_context(deal: DealId, shard: ShardIndex) -> Option<ProofContext<AccountId>>;
    }

    /// Implemented by `storage-proofs`; consumed by `storage-deals::close_deal`
    /// to prune all proof/miss records for a closed deal's `n` shards (D.6.5).
    pub trait ProofPruner {
        fn on_deal_closed(deal: DealId, n: ShardIndex);
    }

    /// Implemented by `storage-deals`; consumed by `storage-proofs` (fault side).
    /// `storage-proofs` first repatriates the `FaultSlash` into `deals_account()`,
    /// then calls `on_shard_faulted` to book the bounty and transition the deal.
    pub trait DealFaultHandler<AccountId, Balance> {
        /// Pallet sub-account custodying slashed repair bounties (slash target).
        fn deals_account() -> AccountId;
        /// Mark `(deal, shard)` `Faulted`, credit `repair_bounty` to the deal's
        /// `repair_pot`, and escalate deal state (`Degraded`, or `Lost` if
        /// surviving shards `< k`).
        fn on_shard_faulted(
            deal: DealId,
            shard: ShardIndex,
            provider: &AccountId,
            repair_bounty: Balance,
        );
    }
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
