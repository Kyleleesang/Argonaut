# Implementation D — Detailed Build Spec (Decentralized Storage Platform)

**Status: spec in progress (2026-07-07).** This document expands §9 of
`ARGONAUT_ARCHITECTURE_GUIDE.md` into an implementation-ready spec for a
Sonnet-class model. **Where this document and §9 conflict, this document wins**
(deviations are listed in §D.13 with rationale).

Read §9 of the architecture guide first for the product rationale; read this
document for what to actually type.

---

## D.0 Current repo state (verified 2026-07-07 — do not re-derive)

- polkadot-sdk pinned to `stable2412` via git deps in the workspace
  `Cargo.toml` `[workspace.dependencies]`. All new crates use `workspace = true`
  deps; never introduce a second SDK version.
- Runtime (`runtime/src/lib.rs`): `System, RandomnessCollectiveFlip, Timestamp,
  Balances, FalconRegistry, TransactionPayment, Sudo`. **No Aura/GRANDPA** — the
  chain is driven by the Prism bridge (`node/src/bridge.rs`), one Substrate
  block per confirmed leader level, imported `finalized: true`, never reorgs.
- `Balance = u128`, `BlockNumber = u32`, `AccountId = AccountId32 =
  blake3(falcon_pk)`, `EXISTENTIAL_DEPOSIT = 500`, `BlockHashCount = 2400`.
- Signatures: `ArgonautSignature` (bootstrap `FalconWithKey` / registry-mode
  `Falcon`, checked by the `CheckFalconSignature` TransactionExtension which is
  **first** in `SignedExtra`). Dev sr25519 behind the `dev-signatures` feature.
- Patterns to copy verbatim:
  - Pallet shape: `pallets/falcon-registry/` (lib.rs + mock.rs + tests.rs +
    benchmarking.rs, `ReservableCurrency` deposits, `#[pallet::constant]`
    params, placeholder weights `Weight::from_parts(10_000, 0)`).
  - Host functions: `primitives/falcon/src/lib.rs` — `#[runtime_interface]`
    trait with a native body; registered in the node executor's
    `ExtendHostFunctions` tuple in `node/src/service.rs`.
  - Cargo feature plumbing: copy `pallets/falcon-registry/Cargo.toml`'s
    `std` / `runtime-benchmarks` feature lists.
- The bridge embeds `LeaderDigest { level: u64, leader: H256 }` as a
  `DigestItem::Other` **post-digest** (`node/src/bridge.rs`). Post-digests are
  merged after block-building, so **the runtime cannot read the leader hash
  during execution**. This kills §9.5's beacon design as written — see D.6.1
  for the replacement (epoch-snapshotted block hash; the block hash commits to
  the leader digest anyway because the digest is part of the header).

## D.1 Global rules for the implementing model

1. SCALE (`parity-scale-codec` + `scale_info::TypeInfo`) for every on-chain and
   on-wire type. No serde in pallets/primitives. `MaxEncodedLen` on every
   storage type; `BoundedVec` for every variable-length field in storage.
2. **No unbounded iteration anywhere in the runtime.** No `on_initialize` loops
   over deals/providers/assignments. All deal/shard state transitions are lazy:
   they happen inside user-called extrinsics that touch O(1) storage items
   (plus one O(1) beacon snapshot per epoch boundary, D.6.1).
3. All balance math in `u128` with `saturating_*` / `checked_*`; never bare
   `+`/`*` on balances. Epoch math in `u64`/`u32` with `saturating_sub`.
4. All extrinsics are **signed** (providers/clients/reporters have funded
   accounts). No `ValidateUnsigned` anywhere in Implementation D — transaction
   fees are the anti-spam mechanism; there is no report bond (deviation, D.13).
5. File bytes never touch the chain. On-chain: 32-byte roots, sizes, balances,
   epochs, ≤ `MAX_PROOF_BYTES` bao slices.
6. Native-only crypto (`bao-tree` slice verification) goes behind a
   `#[runtime_interface]` host function, same as Falcon.
7. Every iroh / iroh-blobs API call in the daemon/SDK lives behind the internal
   traits in D.9.2 (one module, ≤ ~500 lines). Pins chosen July 2026:
   `iroh = "=1.0.2"` (stable 1.0 line), `iroh-blobs = "=0.103.0"` (first
   new-API release built against iroh ^1.0; see D.2). Record the pins in the
   crate README. If a pinned API differs from the sketches here, keep the
   trait signatures and adapt the internals.
8. Placeholder weights (`Weight::from_parts(10_000, 0)`) like the existing
   pallets; benchmarking files can be stubs. Do not invent a weight system.
9. Constants: each pallet gets `parameters.rs` (pure `pub const` defaults) and
   takes them via `#[pallet::constant]` Config types so the runtime can tune.

## D.2 Workspace & dependency changes

New workspace members (add to root `Cargo.toml` `members`):

```
primitives/storage
pallets/provider-registry
pallets/storage-deals
pallets/storage-proofs
pallets/payment-channels
client/chain-client        # thin RPC client shared by daemon + CLI (D.8)
client/storage-provider    # provider daemon binary (D.9)
client/argonaut-store      # client SDK lib + CLI binary (D.10)
```

New `[workspace.dependencies]`:

```toml
bao-tree = { version = "0.16", default-features = false }  # already in root Cargo.toml
reed-solomon-simd = "3"
# Verified July 2026: iroh 1.0 is stable (1.0.2 latest); iroh-blobs 0.103.0 is
# the first of the new-API line built against iroh ^1.0 (still labeled "not yet
# production quality" by maintainers — see guide §9.0). Re-check for newer
# patch releases at implementation time, but stay on these major/minor lines.
iroh = "=1.0.2"
iroh-blobs = "=0.103.0"
jsonrpsee = { version = "0.24", features = ["ws-client", "http-client"] }
```

Crate names follow the existing convention: `argonaut-primitives-storage`,
`pallet-provider-registry`, `pallet-storage-deals`, `pallet-storage-proofs`,
`pallet-payment-channels`, `argonaut-chain-client`, `argonaut-storage-provider`,
`argonaut-store`.

`bao-tree` must build `no_std`-compatible for the primitives crate's type side;
the host-fn *body* is native-only (inside `#[runtime_interface]`, like
`fn_dsa_vrfy` in `primitives/falcon`), so `bao-tree` itself may be a
std dependency gated the same way `fn-dsa-vrfy` is in `primitives/falcon/Cargo.toml`
— copy that gating exactly.

## D.3 `primitives/storage` (`argonaut-primitives-storage`)

### D.3.1 Types & constants

```rust
pub type BlobRoot = [u8; 32];   // BLAKE3 root == iroh-blobs content id
pub type DealId = u64;
pub type ChannelId = u64;
pub type EpochIndex = u32;
pub type ShardIndex = u16;

pub const CHUNK_SIZE: u32 = 1024;        // bao leaf granularity, never change
pub const MAX_PROOF_BYTES: u32 = 4096;   // bound checked BEFORE host dispatch
pub const MAX_SHARDS: u32 = 64;          // BoundedVec bound for DealParams.shards

// Domain-separation prefixes (blake3 input framing). Use verbatim.
pub const DS_SELECT: &[u8]   = b"argn/store/select/v1";
pub const DS_CHUNK: &[u8]    = b"argn/store/chunk/v1";
pub const DS_ACTIVATE: &[u8] = b"argn/store/activate/v1";
pub const DS_VOUCHER: &[u8]  = b"argn/voucher/v1";

#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, PartialEq, Eq, Debug)]
pub struct ShardSpec { pub root: BlobRoot, pub size: u64 } // size % 1024 == 0, > 0

#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, PartialEq, Eq, Debug)]
pub struct DealParams<Balance> {
    pub manifest: BlobRoot,          // SCALE Manifest blob (D.3.4) — NOT a HashSeq (deviation, D.13)
    pub shards: BoundedVec<ShardSpec, ConstU32<MAX_SHARDS>>, // len == n
    pub k: u8,                       // RS data shards; 1 <= k < n
    pub n: u8,                       // == shards.len()
    pub duration_epochs: EpochIndex, // vesting horizon, from activation of the deal
    pub price_per_shard_epoch: Balance,
    pub challenge_rate_ppm: u32,     // P(challenge per shard per epoch) in parts-per-million, <= 1_000_000
}
```

### D.3.2 Deterministic selection formulas (used by pallet AND daemon — one impl here)

```rust
/// Number of 1024-byte chunks in a shard. size is validated % 1024 == 0.
pub fn num_chunks(size: u64) -> u64 { size / CHUNK_SIZE as u64 }

fn h64(parts: &[&[u8]]) -> u64 {
    let mut hasher = blake3::Hasher::new();
    for p in parts { hasher.update(p); }
    let mut b = [0u8; 8];
    b.copy_from_slice(&hasher.finalize().as_bytes()[..8]);
    u64::from_le_bytes(b)
}

/// Is shard (deal, idx) challenged in the epoch whose beacon is `beacon`?
pub fn is_selected(beacon: &[u8; 32], deal: DealId, idx: ShardIndex, rate_ppm: u32) -> bool {
    h64(&[DS_SELECT, beacon, &deal.to_le_bytes(), &idx.to_le_bytes()]) % 1_000_000
        < rate_ppm as u64
}

/// Which chunk must be proven (only meaningful when is_selected is true).
pub fn challenged_chunk(beacon: &[u8; 32], deal: DealId, idx: ShardIndex, size: u64) -> u64 {
    h64(&[DS_CHUNK, beacon, &deal.to_le_bytes(), &idx.to_le_bytes()]) % num_chunks(size)
}

/// Activation challenge chunk (proves possession before escrow starts vesting).
pub fn activation_chunk(beacon: &[u8; 32], deal: DealId, idx: ShardIndex, size: u64) -> u64 {
    h64(&[DS_ACTIVATE, beacon, &deal.to_le_bytes(), &idx.to_le_bytes()]) % num_chunks(size)
}
```

All integer ops here are total (no overflow possible: `% 1_000_000` fits u64,
`rate_ppm <= 1_000_000` enforced at `create_deal`). Endianness is
little-endian everywhere; never use `to_be_bytes`.

### D.3.3 `BaoProof` host function

```rust
#[runtime_interface]
pub trait BaoProof {
    /// Verify a bao slice proving possession of chunk range
    /// [chunk_start, chunk_start + chunk_len) of the blob (root, total_size).
    /// `slice` is EXACTLY the bytes produced by
    /// `bao_tree::io::sync::encode_ranges_validated(data, outboard,
    ///     &ChunkRanges::from(ChunkNum(chunk_start)..ChunkNum(chunk_start+chunk_len)), writer)`
    /// — whatever framing that function writes, verbatim, nothing prepended.
    /// Returns false on ANY error (never panics, never returns Result).
    fn verify_slice(root: &[u8; 32], total_size: u64,
                    chunk_start: u64, chunk_len: u32, slice: &[u8]) -> bool;
}
```

Native implementation notes (adapt names to the pinned `bao-tree` version; keep
the signature above frozen):

- Build `BaoTree::new(ByteNum(total_size), BlockSize::ZERO)` — chunk-level
  verification is independent of whatever outboard block size the provider
  stores (the 16 KiB chunk-group outboards iroh uses share the same root).
- Decode with the crate's response-decoding path
  (`bao_tree::io::sync::decode_ranges` / `DecodeResponseIter` — whichever the
  pinned version exposes) against `root` and the requested `ChunkRanges`;
  return `true` iff decoding consumes the slice without error and yields the
  requested range. Wrap the whole body in a helper returning
  `Option<()>`/`Result` and map to `bool` — no `unwrap`.
- The **encode side lives only in tests and the daemon**, never in the runtime.
- Callers (pallets) must check `slice.len() <= MAX_PROOF_BYTES`, `chunk_len == 1`,
  and `chunk_start < num_chunks(total_size)` **before** calling the host fn.

Register `bao_proof::HostFunctions` in `node/src/service.rs` next to
`falcon_crypto::HostFunctions` (same `ExtendHostFunctions` tuple).

### D.3.4 Manifest format (client-side; on-chain only as its BLAKE3 root)

```rust
/// Stored as a plain iroh blob whose BLAKE3 root == DealParams.manifest.
#[derive(Encode, Decode, TypeInfo, Clone, Debug)]
pub struct Manifest {
    pub version: u8,            // = 1
    pub file_size: u64,         // original file length (before zero-padding)
    pub file_hash: [u8; 32],    // blake3 of the original file bytes
    pub k: u8, pub n: u8,
    pub shard_size: u64,        // identical for all shards; % 1024 == 0
    pub shards: Vec<BlobRoot>,  // len n; index i == ShardIndex i == RS shard i
}
```

RS layout rule (enforced by `argonaut-store encode`, D.10.1):
`shard_size = ceil(file_size / k)` rounded **up** to the next multiple of 1024;
file is zero-padded to `k * shard_size`; shards `0..k` are the data shards in
order, `k..n` the parity shards from `reed-solomon-simd`
(`ReedSolomonEncoder::new(k, n - k, shard_size)`). Decode truncates the
reassembled bytes to `file_size` and verifies `blake3 == file_hash`.

## D.4 `pallets/provider-registry`

Config: `RuntimeEvent`, `Currency: ReservableCurrency`, constants `MinStake:
Balance`.

```rust
#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, PartialEq, Eq, Debug)]
pub enum ProviderStatus { Active, Deregistering }

#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, Debug)]
pub struct ProviderInfo<Balance> {
    pub stake: Balance,                            // amount reserved
    pub iroh_node_id: [u8; 32],
    pub relay_hint: BoundedVec<u8, ConstU32<256>>, // opaque; daemon interprets
    pub price_per_shard_epoch: Balance,            // advertised; deals set the real price
    pub active_assignments: u32,                   // incremented on accept_shard, decremented on fault/expiry/termination
    pub faults: u32,                               // lifetime counter, informational
    pub status: ProviderStatus,
}

Providers: StorageMap<AccountId, ProviderInfo<Balance>>  // Blake2_128Concat
```

Extrinsics (all signed by the provider):
- `register(stake, iroh_node_id, relay_hint, price)` — `stake >= MinStake`,
  not already registered, `Currency::reserve(who, stake)`, insert `Active`.
- `top_up_stake(amount)` — reserve more, add to `stake`.
- `update_terms(iroh_node_id, relay_hint, price)`.
- `deregister()` — require `active_assignments == 0`; unreserve full stake,
  remove entry. (No unbonding period in v1 — deviation, D.13.)

Traits exported (consumed by deals + proofs pallets via Config assoc types):

```rust
pub trait ProviderInterface<AccountId, Balance> {
    fn is_active(who: &AccountId) -> bool;
    fn free_stake(who: &AccountId) -> Balance; // stake minus StakeLock (below)
    /// Move `amount` of reserved stake to `beneficiary` (repatriate_reserved,
    /// BalanceStatus::Free). Returns actually-slashed amount. Decrements stake.
    fn slash(who: &AccountId, amount: Balance, beneficiary: &AccountId) -> Balance;
    fn note_assignment_started(who: &AccountId, escrow_at_risk: Balance) -> DispatchResult;
    fn note_assignment_ended(who: &AccountId, escrow_at_risk: Balance);
    fn note_fault(who: &AccountId);
}
```

Stake sizing rule (§9.3): a provider may only accept a shard while
`stake >= 2 * (sum of per-shard escrow across its active assignments,
including the new one)`. Track that running sum as `StakeLocked:
StorageMap<AccountId, Balance>` updated by
`note_assignment_started/ended(escrow_at_risk = duration_epochs *
price_per_shard_epoch)`. `note_assignment_started` fails with
`InsufficientStake` when the 2× rule would break.

Events: `Registered`, `StakeToppedUp`, `TermsUpdated`, `Deregistered`,
`Slashed { who, amount, beneficiary }`.
Errors: `AlreadyRegistered`, `NotRegistered`, `StakeTooLow`,
`HasActiveAssignments`, `InsufficientStake`.

## D.5 `pallets/storage-deals`

Config: `RuntimeEvent`, `Currency: ReservableCurrency`, `Providers:
ProviderInterface<..>`, `Epochs: EpochProvider` (D.6.1), constants:
`AssignTimeoutEpochs` (default 4), `ActivationTimeoutEpochs` (default 2),
`CloseGraceEpochs` (default 8), `CancelFeeEpochs` (default 2),
`MaxDealsPerBlock`-style limits are NOT needed (no iteration).

### D.5.1 Storage

```rust
#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, PartialEq, Eq, Debug)]
pub enum DealState { Listed, Assigning, Active, Degraded, Expired, Cancelled, Lost }

#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, Debug)]
pub struct Deal<AccountId, Balance> {
    pub client: AccountId,
    pub params: DealParams<Balance>,
    pub state: DealState,
    pub created_epoch: EpochIndex,
    pub start_epoch: EpochIndex,      // set when state -> Active (all n activated)
    pub end_epoch: EpochIndex,        // start_epoch + duration_epochs
    pub escrow_remaining: Balance,    // reserved on client; decremented on payouts/refunds
    pub repair_pot: Balance,          // funded by FaultSlash; paid to repairers
    pub activated_count: u16,
    pub faulted_count: u16,
}

#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, PartialEq, Eq, Debug)]
pub enum ShardState { Accepted, Activated, Faulted }

#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, Debug)]
pub struct Assignment<AccountId> {
    pub provider: AccountId,
    pub state: ShardState,
    pub accepted_epoch: EpochIndex,
    pub activated_epoch: EpochIndex,   // valid when state >= Activated
    pub claimed_through: EpochIndex,   // last epoch already paid out (init = activated_epoch)
    pub consecutive_misses: u8,
}

Deals: StorageMap<DealId, Deal<..>>
ShardAssignments: StorageDoubleMap<DealId, ShardIndex, Assignment<..>>
DealProviders: StorageDoubleMap<DealId, AccountId, ()>   // enforces 1 shard/provider/deal
NextDealId: StorageValue<DealId>
```

### D.5.2 Escrow & vesting math (the exact rules — do not improvise)

- `per_shard_escrow = duration_epochs as u128 * price_per_shard_epoch`
  (checked_mul, error `Overflow`).
- `create_deal` reserves `total_escrow = n as u128 * per_shard_escrow` on the
  client; `escrow_remaining = total_escrow`.
- **Vesting is time-based and independent of proofs** (enforcement is slashing,
  not payment withholding — deviation, D.13): a shard in state `Activated`
  earns `price_per_shard_epoch` for every completed epoch `e` with
  `activated_epoch < e <= min(current_epoch - 1, end_epoch)`.
  A shard that becomes `Faulted` at epoch `f` earns only through `f - 1`
  (store the cutoff by setting `activated_epoch..` untouched and recording the
  fault epoch in `consecutive_misses` path — concretely: on fault, immediately
  auto-claim nothing; instead store `fault_epoch` — add
  `pub fault_epoch: EpochIndex` to `Assignment`, 0 = none).
- `claim_earnings(deal_id, shard_idx)` (signed by the assignment's provider):
  `upper = min(current_epoch.saturating_sub(1), deal.end_epoch)`;
  if `state == Faulted { upper = min(upper, fault_epoch.saturating_sub(1)) }`;
  `owed = price * (upper.saturating_sub(claimed_through)) as u128`;
  require `owed > 0`; `repatriate_reserved(client -> provider, owed)`;
  `claimed_through = upper`; `escrow_remaining -= owed`.
- Payout primitive everywhere:
  `T::Currency::repatriate_reserved(&deal.client, &beneficiary, amount,
  BalanceStatus::Free)` — escrow never leaves the client account until paid.

### D.5.3 Extrinsics & transitions (all lazy; every timeout is checked inside a call)

- `create_deal(params)` — validate: `1 <= k < n == shards.len() <= 64`, every
  `shard.size % 1024 == 0 && > 0`, `challenge_rate_ppm <= 1_000_000`,
  `duration_epochs > 0`, escrow math no overflow. Reserve escrow. State
  `Listed`. Event `DealCreated { deal_id, client }`.
- `accept_shard(deal_id, shard_idx)` — provider signed; deal state in
  `{Listed, Assigning, Active, Degraded}`; if deal is `Listed/Assigning`:
  `current_epoch <= created_epoch + AssignTimeoutEpochs` else fail
  `AssignWindowClosed`; slot must be empty **or** `Faulted` (repair path);
  `!DealProviders::contains(deal_id, who)`; `Providers::is_active`;
  `Providers::note_assignment_started(who, per_shard_escrow)?`. Insert
  `Assignment { state: Accepted, accepted_epoch: current_epoch, .. }`,
  insert `DealProviders`, state `Listed -> Assigning`.
- `activate_shard(deal_id, shard_idx, slice)` — provider signed; assignment
  `Accepted`; window: `current_epoch > accepted_epoch` (beacon of
  `accepted_epoch + 1` must exist — unpredictable at accept time) and
  `current_epoch <= accepted_epoch + 1 + ActivationTimeoutEpochs`.
  `chunk = activation_chunk(beacon(accepted_epoch + 1), deal_id, shard_idx, size)`;
  bounds-check slice (D.3.3) then `bao_proof::verify_slice(root, size, chunk, 1, slice)`.
  On success: `state = Activated`, `activated_epoch = current_epoch`,
  `claimed_through = current_epoch`, `activated_count += 1`; if repairing a
  `Faulted` slot: `faulted_count -= 1`, pay `repair_pot` to the provider
  (from... `repair_pot` is provider-slash money already repatriated to a pallet
  sub-account — see D.7.4), and if `faulted_count == 0` set `Degraded -> Active`;
  else if `activated_count == n` (first full activation):
  `state = Active`, `start_epoch = current_epoch`,
  `end_epoch = start_epoch + duration_epochs`, and for every shard set
  `activated_epoch = claimed_through = start_epoch` (vesting starts together —
  simplest deterministic rule; early activators wait for the last one).
- `expire_assignment(deal_id, shard_idx)` — permissionless; assignment
  `Accepted` and activation window passed: remove assignment + `DealProviders`
  entry, `note_assignment_ended`. Slot becomes acceptable again.
- `cancel_deal(deal_id)` — client signed; state `Listed`/`Assigning`; unreserve
  `escrow_remaining` fully; remove all assignments (iterate
  `ShardAssignments::drain_prefix(deal_id)` — bounded by n ≤ 64, fine) with
  `note_assignment_ended` for each; state `Cancelled`.
- `terminate(deal_id)` — client signed; state `Active`/`Degraded`; for each
  assignment (drain_prefix, ≤ 64): pay vested-through-`current_epoch - 1` as in
  claim, plus cancellation fee `CancelFeeEpochs * price` per Activated shard;
  `note_assignment_ended`; unreserve remainder to client; state `Cancelled`.
- `close_deal(deal_id)` — permissionless; require
  `current_epoch > end_epoch + CloseGraceEpochs` and state
  `Active`/`Degraded`/`Lost`; drain assignments (unclaimed vested earnings are
  forfeited to the client after the grace window — that's the point of the
  grace window), `note_assignment_ended` each, unreserve `escrow_remaining`,
  state `Expired`, prune the proofs pallet's records via its `on_deal_closed`
  hook (D.7.5).
- Fault escalation is driven by the **proofs pallet** through a
  `DealFaultHandler` trait this pallet implements:
  `fn on_shard_faulted(deal_id, shard_idx, provider)` — sets assignment
  `Faulted` + `fault_epoch = current_epoch`, `faulted_count += 1`, removes
  `DealProviders` entry (so a replacement — or even the same provider — can
  re-accept), `note_assignment_ended`, `note_fault`; if
  `n - faulted_count < k`: state -> `Lost` (terminal; escrow unreserved to
  client immediately, remaining providers keep claim rights via
  `claim_earnings` until `close_deal`); else state `Active -> Degraded`.

Events (one per transition, with ids): `DealCreated`, `ShardAccepted`,
`ShardActivated`, `DealActive`, `AssignmentExpired`, `EarningsClaimed`,
`DealCancelled`, `DealTerminated`, `DealClosed`, `ShardFaulted`,
`DealDegraded`, `DealRepaired`, `DealLost`.

Errors: `DealNotFound`, `BadParams`, `Overflow`, `AssignWindowClosed`,
`SlotOccupied`, `AlreadyProviding`, `NotProvider`, `WrongState`,
`ActivationWindowClosed`, `ActivationWindowNotOpen`, `BadProof`,
`ProofTooLarge`, `NothingToClaim`, `NotClient`, `TooEarly`.

## D.6 `pallets/storage-proofs`

Config: `RuntimeEvent`, `Currency`, `Deals: DealInspector + DealFaultHandler`
(read deal params/assignment + fault callback), `Providers:
ProviderInterface`, constants: `EpochDuration: BlockNumber` (default 100),
`BeaconDelay: BlockNumber` (default 10), `BeaconRetention: EpochIndex`
(default 8), `ReportWindowEpochs` (default 4), `MissedProofSlash: Balance`,
`FaultSlash: Balance`, `MaxConsecutiveMisses: u8` (default 3).

### D.6.1 Epochs & beacon (replaces §9.5's digest design — deviation, D.13)

- `current_epoch = (frame_system block_number / EpochDuration) as EpochIndex`.
  Expose as `pub trait EpochProvider { fn current_epoch() -> EpochIndex; }`
  implemented by this pallet and consumed by storage-deals.
- `Beacons: StorageMap<EpochIndex, [u8; 32]>`.
- `on_initialize(n)`: if `n % EpochDuration == 0` (epoch boundary, epoch
  `e = n / EpochDuration`): `Beacons::insert(e,
  frame_system::Pallet::<T>::block_hash(n.saturating_sub(BeaconDelay)))` and
  `Beacons::remove(e.saturating_sub(BeaconRetention))`. O(1), constant weight.
  (`BeaconDelay < BlockHashCount = 2400` — always available.)
- Rationale: the runtime cannot read the bridge's leader post-digest, but the
  Substrate block hash **commits to it** (the digest is in the header), the
  chain never reorgs, and this works identically on an Aura devnet — no bridge
  changes, no pre-B3 stubbing needed. Grinding analysis of §9.5 unchanged.
- `beacon(e)` missing (pre-genesis epochs / pruned) → extrinsic fails
  `BeaconUnavailable`. Deals created in epoch 0 just start challenges at
  epoch 1; harmless.

### D.6.2 Storage

```rust
// Proofs actually submitted, per (deal, shard) per epoch. Needed because a
// provider may prove epoch e+1 but miss e; a scalar last_proven_epoch would
// mask the miss. Pruned by on_deal_closed.
ProofRecord: StorageDoubleMap<(DealId, ShardIndex), EpochIndex, ()>
// Successful miss reports (prevents double-slash for the same miss).
MissRecord:  StorageDoubleMap<(DealId, ShardIndex), EpochIndex, ()>
```

(Double-map first key: use the tuple with `Blake2_128Concat`; second key
`Twox64Concat` on the epoch.)

### D.6.3 `submit_proof(deal_id, shard_idx, epoch, slice)` — provider signed

Checks in order:
1. assignment exists, caller == provider, state `Activated`;
2. `epoch >= current_epoch - 1 && epoch <= current_epoch` (accepted during the
   challenged epoch or the one after — the deadline);
3. deal state `Active`/`Degraded` and `epoch <= end_epoch`;
4. beacon exists; `is_selected(beacon, deal_id, shard_idx, rate_ppm)` — else
   `NotSelected`;
5. `!ProofRecord::contains(..)` — else `AlreadyProven`;
6. `slice.len() <= MAX_PROOF_BYTES`;
7. `verify_slice(shard.root, shard.size,
   challenged_chunk(beacon, deal_id, shard_idx, shard.size), 1, slice)` — else
   `BadProof`.

Effects: insert `ProofRecord`, reset `consecutive_misses = 0` (via a
`DealInspector` setter or by owning the counter here — put
`consecutive_misses` in THIS pallet as
`Misses: StorageMap<(DealId, ShardIndex), u8>` to avoid cross-pallet mutable
coupling; remove the field from `Assignment`), event
`ProofAccepted { deal_id, shard_idx, epoch }`.

### D.6.4 `report_missed(deal_id, shard_idx, epoch)` — anyone signed, no bond

Checks: assignment `Activated`; deal `Active`/`Degraded`;
`epoch <= end_epoch`; **deadline passed**: `current_epoch >= epoch + 2`;
within window: `epoch >= current_epoch.saturating_sub(ReportWindowEpochs)`;
beacon still retained; `is_selected(...)` true; `!ProofRecord::contains`;
`!MissRecord::contains`.

Effects: insert `MissRecord`; `misses = Misses[..] + 1`;
`Providers::slash(provider, MissedProofSlash, ..)` split 50% to the reporter,
50% to the deal client (direct compensation — no escrow top-up bookkeeping;
deviation, D.13); `faults += 1` info counter; if
`misses >= MaxConsecutiveMisses`: additionally
`Providers::slash(provider, FaultSlash, deals_pallet_account)` credited to
`deal.repair_pot`, then call `Deals::on_shard_faulted(..)`. Events
`MissReported`, `ShardFaultEscalated`.

`ReportWindowEpochs + 2 <= BeaconRetention` — assert in an integration test.

### D.6.5 Pruning & aggregation hook

- `on_deal_closed(deal_id)` (called by storage-deals `close_deal`):
  `let _ = ProofRecord::clear_prefix((deal_id, idx), ..)` per shard idx 0..n
  (bounded: n ≤ 64 prefixes; pass a generous limit and ignore the cursor —
  devnet-acceptable) plus `MissRecord` and `Misses` likewise.
- Define the call argument as
  `pub enum ProofPayload { Single(BoundedVec<u8, ConstU32<MAX_PROOF_BYTES>>), Aggregated(H256) }`
  with `Aggregated` rejected (`Unsupported` error) — the v2 STARK-bundle hook
  from §9.5, additive later.

## D.7 `pallets/payment-channels`

Config: `RuntimeEvent`, `Currency: ReservableCurrency`, `KeyProvider:
pallet_falcon_registry::KeyProvider<AccountId>`, constant `MinDeposit`.

```rust
#[derive(Encode, Decode, MaxEncodedLen, TypeInfo, Clone, Debug)]
pub struct Channel<AccountId, Balance, BlockNumber> {
    pub from: AccountId,     // client (payer)
    pub to: AccountId,       // provider (payee)
    pub deposit: Balance,    // reserved on `from`
    pub claimed: Balance,    // cumulative already settled
    pub expiry: BlockNumber, // after this, client may reclaim remainder
}
Channels: StorageMap<ChannelId, Channel<..>>
NextChannelId: StorageValue<ChannelId>
```

Voucher (defined in `primitives/storage` so daemon/SDK/pallet share it):

```rust
#[derive(Encode, Decode, TypeInfo, Clone, Debug)]
pub struct Voucher { pub channel_id: ChannelId, pub cumulative: u128 }
/// Signed payload = blake3(DS_VOUCHER ++ scale(Voucher)) — matches the §2
/// convention of signing 32-byte blake3 digests.
pub fn voucher_signing_payload(v: &Voucher) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(DS_VOUCHER);
    h.update(&v.encode());
    *h.finalize().as_bytes()
}
```

Extrinsics:
- `open(to, deposit, ttl_blocks)` — client signed; `deposit >= MinDeposit`;
  reserve; `expiry = now + ttl_blocks`. Event `ChannelOpened { channel_id }`.
- `settle(channel_id, voucher: Voucher, sig: FalconSignature,
  pk: Option<FalconPublic>)` — **provider** signed (`who == channel.to`);
  resolve pk: if `Some(pk)` require `blake3(pk) == channel.from`, else
  `KeyProvider::key_of(&channel.from)` (error `NoKey`); verify
  `falcon_crypto::verify(&sig.0, &voucher_signing_payload(&voucher), &pk.0)`;
  require `voucher.channel_id == channel_id`,
  `claimed < voucher.cumulative <= deposit`; pay
  `repatriate_reserved(from -> to, cumulative - claimed)`;
  `claimed = cumulative`. Callable repeatedly with increasing cumulative.
- `close(channel_id)` — by `to` any time, or by `from` once `now >= expiry`:
  unreserve `deposit - claimed` back to `from`, remove channel. (Provider
  should `settle` its best voucher first — document in the daemon.)

Monotone-cumulative vouchers mean no challenge game is needed. Falcon voucher
signatures (~666 B) live off-chain; only the final one hits the chain.

## D.8 Runtime wiring (one PR-sized change)

- `construct_runtime!` additions, in this order:
  `ProviderRegistry: pallet_provider_registry, StorageDeals:
  pallet_storage_deals, StorageProofs: pallet_storage_proofs,
  PaymentChannels: pallet_payment_channels`.
- Config impls with the D.4–D.7 defaults; all constants via
  `parameter_types!` next to `FALCON_KEY_DEPOSIT`. Suggested devnet values:
  `MinStake = 1_000 * EXISTENTIAL_DEPOSIT`, `MissedProofSlash = 50 * ED`,
  `FaultSlash = 500 * ED`, `MinDeposit = 10 * ED`, `EpochDuration = 100`.
- `bao_proof::HostFunctions` added in `node/src/service.rs`.
- Deals-pallet sub-account for `repair_pot` custody: use
  `PalletId(*b"argnstor").into_account_truncating()` (add `PalletId` to the
  deals pallet Config), and move slash money there via
  `Providers::slash(provider, FaultSlash, &Self::account_id())`.

## D.9 `client/storage-provider` (daemon, binary `argonaut-provider`) — sketch

Single tokio process; modules:
- `blobs.rs` — the ONLY module importing `iroh`/`iroh-blobs`/`bao-tree`
  directly. Traits:

```rust
#[async_trait]
pub trait BlobStore {
    async fn add_path(&self, path: &Path) -> Result<(BlobRoot, u64)>;
    async fn has(&self, root: &BlobRoot) -> Result<bool>;
    async fn bao_slice(&self, root: &BlobRoot, chunk: u64) -> Result<Vec<u8>>; // encode_ranges_validated output
    async fn fetch(&self, root: &BlobRoot, from: NodeAddrBytes) -> Result<u64>;
    async fn delete(&self, root: &BlobRoot) -> Result<()>;
}
pub trait ServingGate: Send + Sync {
    /// Called from iroh-blobs provider events per request.
    fn allow(&self, node: &[u8; 32], root: &BlobRoot, requested_bytes: u64) -> bool;
}
```

- `chain.rs` — uses `argonaut-chain-client` (D.11): watch finalized heads,
  decode `System::Events`, submit signed extrinsics (Falcon key from the §2.4
  keystore file).
- `responder.rs` — on each new epoch (first finalized block with
  `n % EpochDuration == 0`): recompute the beacon locally
  (`block_hash(n - BeaconDelay)` from chain), for every held assignment run
  `is_selected`; if selected, `bao_slice(...challenged_chunk...)` and submit
  `submit_proof` **early in the epoch**; retry on pool errors; log/webhook on
  any failure (a silent failure = slashed stake). Also handles
  `activate_shard` after accepting.
- `gate.rs` — retrieval gating at **request granularity** (deviation from
  §9.6's mid-stream halting — simpler and fits provider-events): clients must
  fetch in ranged requests ≤ `VOUCHER_STRIDE` (1 MiB); `allow` returns true iff
  the requesting NodeId is mapped to an open channel whose freshest voucher
  covers all previously served bytes. NodeId↔channel mapping via a tiny iroh
  protocol `/argonaut/retrieval/1`: client opens a stream, sends SCALE
  `Hello { channel_id }` + a fresh `Voucher` + signature; subsequent vouchers
  arrive on the same stream. Persist the freshest voucher with fsync **before**
  allowing the next stride (mirror of §4.5 discipline).
- `main.rs` — config file (chain RPC URL, keystore path, blob store dir,
  accepted price), wiring.

## D.10 `client/argonaut-store` (SDK lib + CLI) — sketch

- `encode <file> --k 10 --n 16` → shard files + manifest per D.3.4; prints
  manifest root + shard roots (uses `reed-solomon-simd` + blake3; no iroh
  needed for pure encode).
- `deal create <manifest>` → serve shards from an embedded iroh node; submit
  `create_deal`; watch events; on `ShardAccepted` push/serve the shard to that
  provider's `iroh_node_id`; confirm `DealActive`; then local copies deletable.
- `fetch <manifest_root> --out <path>` → obtain manifest (any provider), open
  channels to ≥ k providers, ranged-fetch with per-stride vouchers, RS-decode,
  truncate to `file_size`, verify `file_hash`.
- `repair <deal_id>` (also `--daemon` keeper mode): on `DealDegraded`, fetch
  any k live shards, RS-reconstruct the faulted shard, `accept_shard` +
  `activate_shard` as the replacement provider, collect `repair_pot`.

## D.11 `client/chain-client` — sketch

Thin, native-only helper crate shared by daemon/CLI (avoids subxt metadata
churn): depends on the `runtime` crate directly; builds `UncheckedExtrinsic`s
with `ArgonautSignature` (reuse the §2.4 signer code), submits via jsonrpsee
`author_submitExtrinsic`, subscribes `chain_subscribeFinalizedHeads`, reads
storage via `state_getStorage` with manually computed keys
(`twox128(pallet) ++ twox128(item) ++ hasher(key)`), decodes with the
runtime's types. Keep it < ~600 lines.

## D.12 Milestones (each lands with its tests green; live-verify per repo rule)

- **D1** (no chain): `primitives/storage` — selection-formula unit tests
  (deterministic, distribution sanity: ~rate_ppm/1e6 of 10k trials selected);
  bao round-trip: 64 MiB random blob, outboard via bao-tree, random chunk,
  `verify_slice` true; flip 1 byte in slice/root/size/chunk → all false;
  slice for chunk i never verifies as chunk j. RS: encode 10-of-16, drop any
  6 shards, reconstruct, byte-identical, `file_hash` matches.
- **D2**: provider-registry + storage-deals + storage-proofs + payment-channels
  pallets with mock.rs/tests.rs per the falcon-registry pattern (use REAL bao
  slices in tests via dev-dep bao-tree on tiny 4 KiB shards — no proof
  mocking). Full lifecycle test: create → accept×n → activate×n → Active →
  epochs advance → submit_proof → claim → close. Timeout paths, cancel,
  terminate, Lost, voucher settle/close — every transition event-asserted.
  Runtime wired (D.8), `cargo build --release` green, devnet still produces
  blocks (live verification).
- **D3**: chain-client + provider daemon; 3 provider daemons + 1 client on the
  devnet (extend `docker-compose.yml`); 50 epochs compressed
  (`EpochDuration` lowered in dev chain-spec): all selected challenges
  answered, earnings claimed; kill one daemon → `report_missed` → slashes →
  `Degraded`.
- **D4**: retrieval: 100 MiB file, 10-of-16, fetch via ≥ 10 providers with
  per-stride vouchers; byte-identity; provider revenue == settled voucher
  totals; client stops vouchering → provider stops within one stride.
- **D5**: repair: keeper reconstructs the faulted shard, deal returns
  `Active`, `repair_pot` paid.

## D.13 Deviations from architecture-guide §9 (binding)

1. **Beacon**: epoch-snapshotted Substrate block hash in `on_initialize`
   (D.6.1), not a bridge-written leader-hash digest — the runtime can't read
   post-digests, and the block hash commits to the leader digest anyway.
   No bridge changes; works pre/post-B3 identically.
2. **Challenge-rate units**: `challenge_rate_ppm` per-epoch probability
   (ppm), not "challenges per day" — removes a day/epoch conversion Sonnet
   would get wrong.
3. **Manifest**: plain SCALE blob (D.3.4), not an iroh HashSeq — we need
   params in it anyway; one decode path.
4. **No report bond**: invalid `report_missed` fails synchronously and costs
   fees; a bond protects nothing here.
5. **Slash split**: 50% reporter / 50% deal client directly; `FaultSlash`
   funds the per-deal `repair_pot`. No "escrow top-up" bookkeeping.
6. **Vesting independent of proofs**: payment is time-based; misbehavior is
   punished by slashing, not payment withholding. Simpler and deterministic.
7. **No unbonding period** on provider deregister in v1 (requires
   `active_assignments == 0` anyway).
8. **Retrieval gating at request granularity** (≤ 1 stride per request), not
   mid-stream QUIC halting.
9. **All extrinsics signed**; no `ValidateUnsigned` in D.

## D.14 Known pitfalls for the implementing model (checklist)

- `Verify`/pallets have no storage access rules and wasm/native feature-gate
  traps — see the long comment on `ArgonautSignature::Sr25519` in
  `primitives/falcon/src/lib.rs` before adding ANY `#[cfg(feature = "std")]`
  to a shared enum.
- `MaxEncodedLen` is required for every storage type — `DealParams` contains a
  `BoundedVec`, fine; never store an unbounded `Vec`.
- `repatriate_reserved` returns the *unmoved* remainder — check/handle it, and
  remember it no-ops amounts below ED for account creation; providers/clients
  in tests must be pre-funded above ED.
- Epoch off-by-ones: proofs for epoch `e` accepted while
  `current_epoch ∈ {e, e+1}`; reports valid from `e+2`; beacons retained
  `BeaconRetention = 8 ≥ ReportWindowEpochs + 2`. Copy D.6.3/D.6.4 literally.
- `drain_prefix`/`clear_prefix` are bounded here only because `n ≤ 64` —
  keep `MAX_SHARDS` enforcement in `create_deal`.
- Deferred compile rule (repo convention): batch edits, then
  `cargo build`/`test` once per batch, not per edit.
- Don't wander into Proof-of-Replication, don't add a second commitment
  scheme, don't put file bytes on chain (§9.1 binding decisions).

## D.15 Handoff prompts (supersede §9.11)

- **P-D1:** "Implement D.2 workspace changes + D.3 `primitives/storage`
  (types, selection formulas, `BaoProof` host function, `Manifest`,
  `Voucher`) and the `argonaut-store encode` CLI (D.10), with all D1 tests
  (D.12). Register the host function in `node/src/service.rs`."
- **P-D2:** "Implement D.4–D.7 pallets exactly as specified (storage layouts,
  extrinsic check-order, escrow math, state machine) and wire the runtime per
  D.8, with the D2 tests. Copy the falcon-registry pallet layout. Constants in
  `parameters.rs`."
- **P-D3:** "Implement D.11 `chain-client` and the D.9 provider daemon with
  iroh-blobs pinned at `=<version>`; all iroh calls behind the D.9 traits.
  Deliver D3 on the docker-compose devnet."
- **P-D4:** "Implement D.10 `deal create`/`fetch`/`repair` with per-stride
  vouchers and the D.9 retrieval gate. Deliver D4–D5."

Rules to restate in every prompt: SCALE everywhere; no serde; no unbounded
iteration in the runtime; native crypto behind host functions; all extrinsics
signed; when a pinned upstream API differs from this doc's sketches, keep this
doc's trait/extrinsic signatures and adapt internals, noting the deviation in
the crate README.
