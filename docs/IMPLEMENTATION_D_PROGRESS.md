# Implementation D — Progress, Bugs, Concerns & Decisions

**Living log.** Tracks what's built, what broke, what was decided, and what to
watch for while implementing Implementation D (decentralized storage platform).
Spec of record: `docs/IMPLEMENTATION_D_SPEC.md`. Update this file as work
proceeds; it is the running counterpart to the spec.

---

## Milestone status

**Current checkpoint (2026-07-09):** **D2 CODE-COMPLETE + runtime wasm build
GREEN.** All four pallets implemented + wired into the runtime (D.4–D.8).
**49 pallet unit tests green** (provider-registry 11, storage-deals 13,
storage-proofs 13, payment-channels 10 — counts exclude the 2 macro-generated
integrity tests per pallet), all using REAL bao slices / Falcon vouchers (no
proof mocking, spec D.12). **`cargo build -p argonaut-runtime` finished clean in
4m33s** — all four pallets compiled in BOTH std and the `no_std` wasm graph (the
D.14 feature-gate trap is clear); only 3 pre-existing `create_runtime_str`
deprecation warnings, unrelated to D2. **`cargo build -p argonaut-node` also green
(exit 0)** and **`cargo test -p argonaut-runtime` green (10/10)** — the full node
binary links and the runtime + wiring pass. Still owed: a live devnet
block-production check (C-D2.1). **Committed 2026-07-09** (branch `Base`).

| Milestone | State | Notes |
|---|---|---|
| **D1** — `primitives/storage` + `argonaut-store encode` + host-fn reg | ✅ **DONE — 14/14 tests + node compiles** | committed |
| **D2** — pallets (registry/deals/proofs/channels) + runtime wiring | 🟢 **CODE-DONE — 49 pallet tests + runtime tests + wasm + node binary all green; devnet check pending** | committed |
| D3 — chain-client + provider daemon | not started | P-D3 (needs iroh) |
| D4/D5 — retrieval + repair | not started | P-D4 |

### D2 detail (2026-07-09)
- **Four pallets**, each `lib.rs + mock.rs + tests.rs + benchmarking.rs +
  parameters.rs`, copied from the `falcon-registry` layout:
  - `pallets/provider-registry` — stake/terms/slashing + the `ProviderInterface`
    trait impl; §9.3 2× stake rule enforced in `note_assignment_started`.
  - `pallets/storage-deals` — full deal state machine (create→accept→activate→
    Active→claim→close, plus cancel/terminate/expire/fault/repair/Lost); escrow
    + time-based vesting (D.5.2); implements `DealInspector` + `DealFaultHandler`.
  - `pallets/storage-proofs` — epochs + `on_initialize` beacon snapshot (D.6.1),
    `submit_proof` / `report_missed`, 50/50 slash + fault escalation; implements
    `EpochProvider` + `ProofPruner`.
  - `pallets/payment-channels` — Falcon-signed monotone vouchers (`open`/`settle`
    /`close`); registry-mode + inline-pk key resolution both tested.
- **Cross-pallet interface traits live in `primitives/storage::interfaces`**
  (`ProviderInterface`, `EpochProvider`, `DealInspector`, `DealFaultHandler`,
  `ProofPruner`, `ProofContext`). This is the key structural decision — it lets
  `storage-deals` and `storage-proofs` reference each other's contracts with NO
  crate cycle (Cargo forbids those). See DEC-D2.a.
- Runtime wired (D.8): 4 `construct_runtime!` entries appended after `Sudo`,
  config impls + `parameter_types!` (ED-scaled balances), `PalletId(*b"argnstor")`
  repair-pot sub-account. Storage pallets deliberately NOT added to
  `define_benchmarks!` (proofs/channels benchmarks are stubs) — see C-D2.2.

### D1 detail (2026-07-09)
- `primitives/storage/{lib.rs,tests.rs}` were pre-written by a prior session;
  reviewed, one test-only fix applied, **9 tests pass** (`cargo test -p
  argonaut-primitives-storage`): selection formulas (determinism, beacon
  dependence, rate 0/max, ~25% distribution, chunk-in-range), voucher payload
  binding, and the bao slice round-trip against **real bao-tree 0.16 encoding**
  (honest proof verifies; flip slice byte / root / size / chunk index → all
  reject). The `BaoProof` host-fn `lib.rs` body compiled clean against the
  pinned bao-tree — the sketched API matched.
- `client/argonaut-store` created (lib + CLI). **5 tests pass** (`cargo test -p
  argonaut-store`): the D1 RS test (encode 10-of-16, drop any 6, reconstruct
  byte-identical, `file_hash` verified), all-data-shards fast path, manifest
  roots == shard blob roots, too-few-shards error, bad-params rejection.
- `bao_proof::HostFunctions` registered in `node/src/service.rs`; storage
  primitive added to `node/Cargo.toml`; `client/argonaut-store` added to
  workspace members. **Node compile verification: IN PROGRESS** (see Concerns).

---

## Bugs found & fixed

- **B-D1.1** `bao-tree` 0.16 API mismatch (test only). Spec sketch used
  `PreOrderMemOutboard::root()` as a method; in 0.16 `root` is a **public
  field** (a `blake3::Hash`). Fixed `tests.rs` to `*ob.root.as_bytes()`. The
  `Outboard` trait also exposes `root()` if the field is ever made private.
  *Library (host-fn) code was unaffected — only the test's encode helper.*
- **B-D1.2** `u64::div_ceil` is a stable inherent method (Rust ≥1.73), so the
  fallback `DivCeil` trait added to `argonaut-store` was dead code (warning).
  Removed it; using the inherent method. No behavior change.

---

## Bugs found & fixed (D2)

- **B-D2.1** `repatriate_reserved` will NOT pay into a *dead* (non-existent)
  account — it returns the amount as un-moved remainder, silently paying 0. Hit
  in `payment-channels::settle` tests (provider account had never been created).
  Not a pallet bug: real providers hold funded accounts (they stake). *Fix was
  test-only* (pre-fund the payee). **But this is a live-money hazard**: if any
  slash/payout beneficiary can be a brand-new account, wrap it and check the
  returned remainder. Already relevant to `storage-proofs` miss-slash (reporter
  may be new) — the miss reporter should generally be a funded account; noted in
  C-D2.3.
- **B-D2.2** (test only) `register_provider` test helper funds `stake + 10_000`,
  so a provider keeps 10_000 free after staking; two lifecycle assertions wrongly
  expected the raw earned/bounty amount. Switched to before/after deltas. Pallet
  math was correct.

## Decisions (D2 — recorded so they aren't silently reversed)

- **DEC-D2.a — Interface traits in `primitives/storage::interfaces`, frame-free.**
  All cross-pallet contracts (`ProviderInterface`, `EpochProvider`,
  `DealInspector`, `DealFaultHandler`, `ProofPruner`, `ProofContext`) live in the
  neutral primitives crate, NOT in any pallet. Rationale: `storage-deals` needs
  `EpochProvider`+beacon (from proofs) and proofs needs `DealInspector`+
  `DealFaultHandler` (from deals) → a **crate cycle** if each trait lived in its
  owning pallet. Traits are deliberately frame-free (`note_assignment_started`
  returns `Result<(),()>`, not `DispatchResult`) so the primitives crate stays
  light for the daemon/SDK. The runtime wires concrete `Pallet` types into the
  `Config` slots (`type Epochs = StorageProofs`, `type Deals = StorageDeals`, …).
- **DEC-D2.b — `EpochProvider` also exposes `beacon(epoch)`.** `storage-deals`
  activation needs `beacon(accepted_epoch+1)` but must not import the proofs
  crate; so beacon lookup is a trait method, keeping beacon storage solely in
  proofs.
- **DEC-D2.c — `activated_count` is decremented on fault.** The spec text was
  ambiguous (it only says `faulted_count += 1` on fault, `+1` activated on
  repair). To keep the invariant `activated_count == n ⇔ all shards live`, faults
  do `activated_count -= 1` and repairs `+= 1`. "First full activation" is
  detected by `deal.state == Assigning` (not `activated_count == n`), so a repair
  never re-triggers the `start_epoch`/vesting-rebase.
- **DEC-D2.d — `Lost` keeps escrow reserved until `close_deal`.** §D.5.3's Lost
  clause is self-contradictory ("escrow unreserved immediately" AND "providers
  keep claim rights until close"). Chose the consistent reading: on `Lost` we set
  state only; surviving providers can still `claim_earnings`; the remainder is
  unreserved to the client at `close_deal` (which already accepts `Lost`). See
  C-D2.4.
- **DEC-D2.e — Repair pays the WHOLE `repair_pot` to the first repairer** and
  zeroes it. Simple; if multiple shards faulted, the first repairer collects the
  accumulated bounty. Acceptable v1; note if it needs per-shard accounting later.
- **DEC-D2.f — `note_fault` runs twice on an escalating miss** (once in
  `report_missed`, once inside `on_shard_faulted`). Matches both spec clauses;
  `faults` is informational only, so double-count is harmless. Left as-is.
- **DEC-D2.g — Storage pallets omitted from `define_benchmarks!`.** proofs/
  channels `submit_proof`/`settle` benchmarks need live deal/voucher state;
  wrote `create_deal`/`register`/`open`/`close` real benchmarks and stubs for the
  rest. Not wired into the runtime benchmark set to avoid a broken bench build.

## Concerns / open questions (watch these)

- **C-D2.1 — Devnet block production (live-verification, STILL OWED).** ✅ The
  runtime wasm build is green (4m33s, 0 errors) — the D.14 no_std trap is clear.
  ⏳ Still owed per the repo rule: run a devnet and confirm blocks still produce
  with the 4 pallets wired + the `storage-proofs::on_initialize` beacon hook
  live (it now runs every block). Watch for: the beacon snapshot not panicking
  on early blocks (target hash may be genesis/zero — see C-D2.2), and no weight/
  execution regressions. The node binary build was kicked off right after the
  runtime; confirm it too.
- **C-D2.2 — Beacon `on_initialize` is the only always-run hook in D.** It reads
  `block_hash(n - BeaconDelay)` each epoch boundary. On a fresh devnet the first
  `BeaconDelay` blocks' target hash may be genesis/zero — harmless (epoch-0 deals
  start challenges at epoch 1). Confirm on devnet that beacons become non-zero.
- **C-D2.3 — Slash beneficiary dead-account (see B-D2.1).** `report_missed`
  splits the slash 50/50 to reporter + client. If either is a fresh account below
  ED, that half silently fails to move (remainder returned, ignored). Devnet-OK;
  before mainnet, handle the remainder (e.g. redirect to the other party or a
  treasury). The `repair_pot` `FaultSlash` target is the pallet sub-account,
  which `close`/`repair` fund flows assume exists once it first receives money.
- **C-D2.4 — `Lost`/`terminate` leftover `repair_pot`.** A `terminate` or
  `close` on a deal with a non-zero `repair_pot` currently leaves that slashed
  money in the pallet sub-account (not returned to client). Small, devnet-OK;
  note for a sweep at close.
- **C-D2.5 — Faulted provider's unclaimed earnings can be overwritten by a
  repair.** `accept_shard` on a `Faulted` slot overwrites the old assignment, so
  a faulted provider that didn't `claim_earnings` before the slot is repaired
  loses its through-`fault_epoch-1` vesting. Document "claim before you're
  replaced"; or snapshot owed-at-fault into the fault handler later.
- **C-D2.6 — Stake can drop below the 2× invariant after a slash.** `slash`
  reduces `stake` but does not re-check the 2× rule against remaining
  assignments (too expensive to re-scan). The invariant is only enforced at
  `accept`. Acceptable (slashing is already the punishment); note it.

- **C-D1.1 — Node build.** ✅ RESOLVED: `cargo check -p argonaut-node` finished
  clean (0 errors) with `bao_proof::HostFunctions` registered. D1 is a "no chain"
  milestone (spec D.12), so no devnet run is owed for D1 itself — but the FIRST
  D2 change that wires pallets into the runtime must live-verify the devnet still
  produces blocks (repo rule).
- **C-D1.2 — BLAKE3 root == bao root assumption.** `argonaut-store::blob_root`
  uses `blake3::hash(bytes)`. This equals bao-tree's outboard root only because
  BLAKE3's standard hash *is* the tree root and is chunk-group-granularity
  independent (spec §9.0 / D.3.3). Verified indirectly: the primitives bao test
  builds the root via `PreOrderMemOutboard` and `verify_slice` accepts it. **To
  fully close this, D3 must assert `blake3::hash(shard) == iroh_blobs add_bytes
  root`** for a real shard once the daemon exists — add to D3 acceptance.
- **C-D1.3 — reed-solomon-simd shard-size constraints.** RS requires shard bytes
  be a multiple of 64 (and ≥64). Guaranteed today because `shard_size` is a
  multiple of `CHUNK_SIZE = 1024`. If `CHUNK_SIZE` or the round-up ever changes,
  RS will reject. Keep the "multiple of 1024" invariant coupled to RS.
- **C-D1.4 — `verify_slice` accepts `chunk_len > 1` framing but callers must
  pass 1.** The host fn iterates whatever range it's given; D2 pallets MUST
  enforce `chunk_len == 1` and `chunk_start < num_chunks` *before* dispatch
  (D.3.3), since the host fn trusts its inputs for bounds. Not a bug — a caller
  obligation to encode in every pallet check-order.
- **C-D1.5 — `saw_leaf` is the success signal.** `verify_slice` returns true iff
  decoding yields ≥1 validated leaf and no error. For `chunk_len == 1` this is
  exactly right. If a future caller ever requests a 0-length range, it would
  return false (no leaf) — acceptable/defensive.

---

## Decisions (potentially important; recorded so they aren't silently reversed)

- **DEC-D1.a — `argonaut-store` is native/std-only, outside the wasm graph.**
  It's a client SDK + CLI (`clap`, `anyhow`, `reed-solomon-simd`, std). It must
  never become a runtime dependency. Only `primitives/storage` (no_std, with the
  native-only host-fn body gated behind `std`/`bao-tree`, exactly like
  `fn-dsa-vrfy`) crosses into the runtime.
- **DEC-D1.b — Manifest stores `blake3::hash` shard roots, computed without
  iroh.** Encode is pure (no embedded iroh node) per D.10.1. The daemon (D3)
  will re-add shards to a real iroh-blobs store; roots must match (C-D1.2).
- **DEC-D1.c — CLI shard filenames `shard_{i:02}.bin` + `manifest.scale`.** Not
  in the spec; chosen for the encode command's on-disk output. Index `i` ==
  `ShardIndex i` == RS shard i (data `0..k`, parity `k..n`). If D3's `deal
  create` expects a different on-disk layout, reconcile there.
- **DEC-D1.d — `decode` takes `&[Option<Vec<u8>>]` of length n.** A present slot
  is `Some(shard_bytes)`. This is the SDK-internal decode contract; the `fetch`
  path (D4) will build this vector from whatever ≥k shards it retrieves.
- **DEC-D1.e — Kept the spec's frozen signatures.** Per the spec's standing
  rule, where bao-tree 0.16 differed from the sketch (B-D1.1) the *trait/fn
  signatures were kept* and only the internal call adapted.

---

## Carry-forward for D2 (so the next session doesn't re-derive)

- Pallets must call `argonaut_primitives_storage::bao_proof::verify_slice(root,
  size, chunk, 1, slice)` and enforce, in check-order, **before** dispatch:
  `slice.len() <= MAX_PROOF_BYTES`, `chunk_len == 1` (implicit — pass 1),
  `chunk_start < num_chunks(size)` (C-D1.4).
- Selection/beacon: use `is_selected` / `challenged_chunk` / `activation_chunk`
  from `primitives/storage` verbatim — do not re-implement (they're the shared
  source of truth vs the daemon).
- Copy the `pallets/falcon-registry` layout (lib+mock+tests+benchmarking,
  `ReservableCurrency`, `#[pallet::constant]`, placeholder weights).
- Beacon is the epoch-snapshotted `frame_system` block hash in `on_initialize`
  (D.6.1) — **not** a bridge digest (the runtime can't read post-digests).
- D2 tests must use **real bao slices on tiny 4 KiB shards** (dev-dep bao-tree),
  never mocked proofs (spec D.12).

## Carry-forward for D3 (chain-client + provider daemon)

- **Beacon recompute must match the pallet exactly.** The daemon computes the
  beacon itself as `block_hash(epoch_boundary_block − BeaconDelay)` and feeds it
  to `is_selected` / `challenged_chunk` from `primitives/storage`. `EpochDuration`
  / `BeaconDelay` come from the proofs pallet constants (100 / 10 on the default
  chain-spec; lower `EpochDuration` on the compressed devnet chain-spec per D.12).
- **On-chain the beacon is `frame_system` block hash bytes** (H256 → `[u8;32]`,
  little-endian-agnostic — it's a raw 32-byte hash). The daemon reads the same
  block hash via RPC `chain_getBlockHash`.
- **Extrinsic shapes the daemon submits** (call indices):
  provider-registry `register`(0)/`top_up_stake`(1)/`update_terms`(2)/`deregister`(3);
  storage-deals `create_deal`(0)/`accept_shard`(1)/`activate_shard`(2)/
  `claim_earnings`(3)/`expire_assignment`(4)/`cancel_deal`(5)/`terminate`(6)/
  `close_deal`(7); storage-proofs `submit_proof`(0)/`report_missed`(1);
  payment-channels `open`(0)/`settle`(1)/`close`(2).
- `activate_shard` / `submit_proof` take a `BoundedVec<u8, ConstU32<4096>>` bao
  slice = exactly `encode_ranges_validated` output for one chunk (the daemon's
  `blobs.rs::bao_slice` per D.9). `submit_proof` wraps it in
  `ProofPayload::Single`.
- **C-D1.2 still open**: D3 must assert `blake3::hash(shard) == iroh-blobs
  add_bytes root` for a real shard so on-chain `ShardSpec.root` matches what the
  daemon serves.
- Pallet events the daemon watches: `DealCreated`, `ShardAccepted`,
  `ShardActivated`, `DealActive`, `ShardFaulted`, `DealDegraded`, `DealLost`.
- **Runtime pallet indices shifted**: the 4 storage pallets were appended AFTER
  `Sudo` in `construct_runtime!`, so existing pallet indices are unchanged;
  ProviderRegistry/StorageDeals/StorageProofs/PaymentChannels take the next
  indices. Any `chain-client` manual storage-key computation must use the live
  metadata, not hard-coded indices.
