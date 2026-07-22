# Implementation D — Progress, Bugs, Concerns & Decisions

**Living log.** Tracks what's built, what broke, what was decided, and what to
watch for while implementing Implementation D (decentralized storage platform).
Spec of record: `docs/IMPLEMENTATION_D_SPEC.md`. Update this file as work
proceeds; it is the running counterpart to the spec.

---

## Milestone status

**Current checkpoint (2026-07-22):** **D1, D2, D3 fully done. D4 code-done,
compiles clean, unit-tested, and *partially* live-verified** — deal creation,
on-chain shard acceptance, and the real iroh shard-push protocol (bytes
actually landing in a provider's blob store, `blake3`-verified against the
on-chain root) are all confirmed working against a live multi-process devnet.
Full end-to-end activation + `fetch` + byte-identity was not reached this
session; see D4 detail below for exactly what's proven, what blocked the
rest, and why neither blocker is a code defect. D5 (repair) not started.

| Milestone | State | Notes |
|---|---|---|
| **D1** — `primitives/storage` + `argonaut-store encode` + host-fn reg | ✅ **DONE — 14/14 tests + node compiles** | committed |
| **D2** — pallets (registry/deals/proofs/channels) + runtime wiring | ✅ **DONE — 49 pallet tests + runtime tests + wasm + node binary all green + live devnet check green** | committed |
| **D3** — chain-client + provider daemon | ✅ **DONE — live-verified end to end** (real devnet registration, decoded real on-chain events) | committed |
| **D4** — real iroh retrieval (shard-push + gated retrieval protocols, `deal create`/`fetch` CLI) | 🟡 **CODE-DONE, unit-tested, partially live-verified** (deal creation + shard acceptance + real iroh push confirmed; activation + fetch not reached, see below) | committed |
| D5 — repair | not started | needs D4's `fetch` (done) as a building block |

### C-D2.1/C-D2.2 live devnet verification — DONE 2026-07-16

Live-verified on a single-node `--dev` devnet using a temporary,
never-committed build (`EPOCH_DURATION_BLOCKS` locally edited 100→5, and a
`build-spec`-generated chain-spec JSON with `prismMVoterChains` lowered
100→4 to make first confirmation tractable in minutes rather than the
~10+ minutes a single miner needs at the real m=100 default — both reverted
before committing; the beacon/`on_initialize` logic under test is identical
regardless of either value).

- **16 Substrate blocks imported, crossing 3 full epoch boundaries (5, 10,
  15), zero panics, zero errors** in the node log throughout.
- Queried `StorageProofs::Beacons` directly via `state_getKeysPaged`/
  `state_getStorage` RPC (manual `twox128` prefix key, no chain-client
  needed yet): **3 beacon entries existed** (epochs 1–3, key suffixes
  `01000000`/`02000000`/`03000000`).
- **Epoch 1's beacon == the genesis hash** — exactly the C-D2.2-documented
  edge case (`BeaconDelay=10` exceeds epoch 1's boundary block `5`, so the
  saturating subtraction reads block 0). Confirmed harmless, as designed.
- **Epoch 3's beacon byte-exactly equals `chain_getBlockHash(5)`**
  (`boundary block 15 − BeaconDelay 10 = 5`) — a precise, not just
  plausible, confirmation that the beacon snapshot computes exactly the
  documented formula.

C-D2.1 and C-D2.2 are closed. Nothing else in D2 is owed.

### D4 detail (2026-07-21)

**Real iroh network serving**, not stubbed: the two custom protocols D.9's
`gate.rs` spec calls for, built directly on iroh's raw bidirectional QUIC
streams (`ProtocolHandler`/`Router`, not the stock `iroh-blobs` blob-transfer
protocol, which has no concept of per-stride payment gating). API knowledge
came from reading the actual pinned crate sources and their own examples
(`~/.cargo/registry/.../iroh-blobs-0.103.0/examples/{transfer,custom-protocol}.rs`)
directly, not guessed — this de-risked what was expected to be the highest-risk
part of D4, and the resulting code compiled clean on essentially the first
real attempt (a handful of `Debug`-bound and `anyhow::Error`-vs-`std::error::Error`
fixups, nothing structural).

- **`primitives/storage`**: new wire-protocol types shared by both sides —
  `ShardPushRequest`/`Response`, `RetrievalRequest`/`Response`, `Voucher`
  (already existed), `VOUCHER_STRIDE` (1 MiB), `SHARD_PUSH_ALPN`/
  `RETRIEVAL_ALPN` constants. Added `argonaut-primitives-falcon` as a
  (no_std-safe) dependency since `RetrievalRequest` carries a
  `FalconSignature`.
- **`wallet/signer-core`**: new `sign_digest` (raw Falcon sign over an
  already-hashed 32-byte digest, no internal re-hash) — needed because
  `pallet_payment_channels::settle` verifies against
  `voucher_signing_payload(&voucher)`, which is *itself* already a blake3
  hash; the existing `sign_payload` would have hashed it a second time and
  produced a signature the chain rejects. `sign_payload` is now defined in
  terms of `sign_digest`, not duplicated.
- **`client/chain-client`**: new `channels` module (`open_channel`,
  `settle_channel`, `sign_voucher`) — lives here, not in the daemon's own
  `chain.rs`, because both the provider daemon (settles) and the client CLI
  (opens + signs per stride) need it.
- **`client/storage-provider`**:
  - `blobs.rs` gained `add_bytes` (ingest pushed bytes) and `read_range`
    (serve raw byte ranges for retrieval — deliberately not per-stride
    bao-verified slices; see `protocol.rs`'s module doc for why that's an
    honest scope cut, not a silent gap).
  - New `protocol.rs`: `ShardPushHandler` (verifies `blake3(bytes)` against
    the on-chain root before storing, via a small `ShardAuthority` trait so
    this module doesn't need to depend on chain-client's concrete types)
    and `RetrievalHandler` (tracks bytes served per channel in memory,
    requires each stride's voucher to cover everything served on that
    channel so far).
  - `main.rs`: binds a real iroh `Endpoint` (no relay, no DNS discovery —
    `presets::Minimal` + `RelayMode::Disabled`; a devnet's participants
    address each other directly via on-chain `relay_hint`, which now
    carries a real `"ip:port"` instead of the D3 placeholder), persists the
    daemon's iroh identity to `<data_dir>/iroh_secret.key` (stable across
    restarts, same reasoning as the node's own libp2p key), and spawns a
    `Router` serving both protocols. Gained a new one-shot `accept-shard`
    subcommand (`argonaut-provider accept-shard --deal-id --shard-idx`) —
    D3's daemon deliberately doesn't auto-bid on deals (module doc,
    unchanged), so *something* has to submit `accept_shard` before the
    shard-push protocol's `ShardAuthority` check will accept a push; this is
    that something, until a real bidding policy exists.
- **`client/argonaut-store`**: new `deal` module — `create_deal` (submit +
  poll until mined) and `push_shards_for_deal` (poll each shard's
  `ShardAssignments` entry until `Accepted`, then push to whichever provider
  actually holds it) — see "Bugs found & fixed (D4)" below for why this is
  two functions, not one, as of 2026-07-22 — and `fetch` module (`fetch_shard`
  — opens a payment channel sized to the shard, retrieves stride by stride
  with an ever-larger voucher, verifies the reassembled shard's root). New
  CLI subcommands `deal-create` and `fetch` wired into `main.rs`.
  `discover_active_providers` (`state_getKeysPaged` over `Active` providers,
  no ranking beyond "first n") is now only an upfront informational check,
  not the push target — see below for why.

**Known gap, documented in `protocol.rs`'s own module doc, not silent:
`RetrievalHandler` never verifies `RetrievalRequest::sig`.** It checks that
the claimed `voucher.cumulative` covers strides served so far (the metering
logic works), but never confirms the signature is real or belongs to the
channel's actual `from` account — so as implemented, anyone who knows a
`channel_id` and sends a garbage signature with a big enough `cumulative`
gets served for free. The metering mechanism is real; the authentication of
*who* is paying is not, yet. Fix needs the provider to resolve the channel's
`from` account + Falcon key on-chain (mirroring `settle`'s own inline-pk /
registry-lookup resolution) before calling `falcon_crypto::verify` — a
second small chain-backed authority trait, parallel to `ShardAuthority`,
not done in this pass. **Do not treat this protocol as production-safe
until this is closed.**

**Other documented scope cuts, matching this session's established
pattern of stating them rather than hiding them:**
- Manifest distribution isn't automated — `fetch` requires the caller to
  already have the local `manifest.scale`, rather than fetching it from a
  provider first (the spec's own "obtain manifest from any provider" step).
- `price_per_stride` is an out-of-band-agreed rate passed identically to
  both the daemon and the fetching client — nothing in D prices retrieval
  bandwidth on-chain.
- One payment channel per shard fetched, not one reused across a session's
  shards.
- Provider selection in `deal create` is "first n active", no ranking.

**Live verification: partial, 2026-07-22.** Every unit-level claim above is
tested against the real thing, not a mock (bao slices against the real host
function, from earlier D3 work; voucher signatures against the real
`falcon_crypto::verify`; the exact same live-devnet-derived storage prefix
used in D3's chain-client tests). The full multi-process scenario (a devnet +
2 provider daemons + the client CLI pushing a shard, accepting it, activating
it, and fetching it back byte-identical) was run live, using the same
temporarily-lowered-`EpochDuration`-and-`BeaconDelay` technique (never
committed) as D2's and D3's verifications.

**Confirmed working, live, against a real 2-provider devnet:**
- Both providers' `register_provider` (bootstrap mode) mined and visible via
  `ProviderRegistry::Providers` — same mechanism D3 already verified, redone
  here as part of the full run.
- `argonaut-store deal-create` → `create_deal` mined, `Deals` storage
  readable back with the correct `client`/`params` (3 separate deals created
  across the session's retries, all mined correctly).
- `argonaut-provider accept-shard` → `accept_shard` mined and
  `ShardAssignments` shows `state: Accepted` with the correct provider
  (deal 1, shard 0, provider1).
- **The real iroh shard-push protocol, end to end**: `push_shard_to_provider`
  opened a live QUIC connection, sent the shard bytes, and
  `provider1_data/shards/<blake3-root>` exists on disk with the *exact* root
  computed at `encode` time — i.e. the client → provider byte transfer and
  the provider's own `blake3(bytes) == expected_root` verification both ran
  for real, not mocked. This was the highest-risk untested path in D4 and it
  works.

**Not reached this session: `activate_shard` and `fetch`.** Two independent,
non-code causes, both worth recording so a future session doesn't have to
re-derive them:

1. **Assignment-window vs. this devnet's per-extrinsic inclusion latency.**
   This single-node Prism PoW devnet mines blocks continuously but individual
   extrinsics sometimes take anywhere from under a minute to 20+ minutes to
   actually land in a block (observed directly: two `accept_shard` calls
   submitted in the same instant, one mined within the window, the other
   didn't land until ~20 minutes later). With the test-only
   `EPOCH_DURATION_BLOCKS=4` and the pallet default
   `ASSIGN_TIMEOUT_EPOCHS=4`, the assignment window is only 16 blocks —
   narrow enough that a second, unluckily-delayed `accept_shard` call
   reliably missed it (`Error::AssignWindowClosed`, confirmed by reading
   `ShardAssignments` back as `None` after the extrinsic's nonce had already
   been consumed on-chain). Reproduced twice (deal 1 and deal 3). **Not a
   concern at production parameters**: the real default
   `EPOCH_DURATION_BLOCKS=100` gives a 400-block window, comfortably wider
   than any inclusion delay observed here. A future rerun of this exact test
   should either accept slower iteration at production epoch length, or use
   a still-short-but-less-extreme test value (e.g. 20–40 blocks/epoch) so
   the window has slack for this devnet's latency variance.
2. **Provider daemons don't auto-discover new assignments.** `responder.rs`'s
   `held: Vec<HeldAssignment>` is populated once at daemon startup from
   `--assignment deal_id:shard_idx` CLI flags (module doc in
   `client/storage-provider/src/main.rs`, an intentional D3/D4 scope
   boundary, not a gap found this session) — a daemon started *before* a
   deal exists has no way to learn about it later without a restart. This
   session's provider daemons were started before any deal existed, so even
   the one successfully-accepted-and-pushed shard (deal 1, shard 0) was
   never a candidate for `try_activate` — confirmed by its `ShardAssignments`
   entry still reading `state: Accepted` (never `Activated`) after several
   hours of the daemon running. A future live run needs to either restart
   the provider daemon with `--assignment <deal_id>:<shard_idx>` right after
   `accept-shard` mines, or (a real future improvement, not attempted here)
   give the daemon a way to pick up newly-accepted assignments without a
   restart.

Net effect: `activate_shard` and `argonaut-store fetch` remain unverified
against a live chain (they're exercised by unit/integration tests only).
That's the concrete next step for closing out D4 live verification.

### D3 detail (2026-07-17)

**Two new crates**, both compiling clean and green on `cargo test --workspace`
(50/50 suites):

- **`client/chain-client`** (`argonaut-chain-client`, D.11) — thin native
  RPC client depending on `argonaut-runtime` directly (no subxt). Three
  modules: `keys` (manual storage-key computation via the real
  `frame_support::StorageHasher` impls, not hand-rolled hashing — one test
  cross-checks the computed `StorageProofs::Beacons` prefix against the
  literal value a live devnet returned in the D2 verification run above),
  `extrinsic` (builds + signs `General`-preamble extrinsics — bootstrap and
  registry mode turned out to be exactly the same construction, differing
  only in which `CheckFalconSignature` variant fills `extra`'s first slot,
  once traced through `runtime/src/extensions.rs`'s `validate`), and
  `subscribe` (`chain_subscribeFinalizedHeads`, implemented per spec but not
  used by D3's own daemon — see below).
- **`client/storage-provider`** (`argonaut-storage-provider`, D.9) — the
  `argonaut-provider` daemon binary. `blobs.rs` (`DiskBlobStore`, bao proof
  generation via the exact `PreOrderMemOutboard`/`encode_ranges_validated`
  pattern already proven in `primitives/storage/src/tests.rs` — its own test
  asserts a generated slice verifies against the *real* `bao_proof::verify_slice`
  host function, not a re-implementation), `chain.rs` (typed wrapper: register,
  accept/activate\_shard, submit\_proof, storage reads — using the pallets'
  own `Encode`/`Decode` types via direct crate deps, not hand-mirrored
  structs), `responder.rs` (epoch-driven challenge loop, polling
  `finalized_head()` rather than the subscription stream -- see its module
  doc for why), `main.rs` (CLI, keystore loading, startup registration).

**Runtime change required and made:** `runtime/src/lib.rs` only re-exported
`Call` types for `System`/`Balances`/`Timestamp` (needed by existing
examples); added the same re-export for the four D pallets
(`ProviderRegistryCall`/`StorageDealsCall`/`StorageProofsCall`/
`PaymentChannelsCall`) since an out-of-process client needs the concrete
`Call` type, not just the pallet's `Config` impl — matches the existing
pattern exactly, not a new one.

**Scope cut, explicit and spec-consistent:** `BlobStore::fetch` (pulling a
blob from a peer over iroh) is stubbed. D3's own acceptance bar (D.12) is
the challenge-response loop, not retrieval — that's D4's "100 MiB fetch...
per-stride vouchers." Proof generation needs only the shard bytes +
`bao-tree`, no iroh at all, so `DiskBlobStore` is fully real for everything
D3 actually needs; iroh/iroh-blobs (`=1.0.2`/`=0.103.0`, D.2's pins) are
confirmed to resolve and compile cleanly together (de-risked before writing
any code against them), but real network serving is D4 work. **C-D1.2
("`blake3::hash(shard) == iroh-blobs add_bytes root`") is therefore still
open** — nothing yet exercises the real iroh-blobs store.

**Live-verified (2026-07-17), the chain-interaction half:** ran the actual
`argonaut-provider` binary against a real single-node devnet (temporary
`build-spec`-generated chain spec funding a fresh Falcon keystore account
and lowering `prismMVoterChains` for tractable confirmation time — same
never-committed-artifact technique as the B-10/D2 verifications). Observed,
by decoding the real `System::Events` for the block that included the
daemon's extrinsic (via a throwaway decoder linking the actual
`argonaut_runtime::RuntimeEvent` type, not manual SCALE-byte reading):
`Balances::Reserved` (FalconRegistry key deposit) → `FalconRegistry::KeyRegistered`
(A-4's auto-register-on-bootstrap firing correctly) → `Balances::Withdraw`
(fee) → `Balances::Reserved` (the exact `1_000_000_000_000_000`-planck stake
the daemon's default `--stake` requested) → **`ProviderRegistry::Registered`**
→ fee-refund/payment events → `ExtrinsicSuccess`. Then independently
recomputed `chain-client`'s own storage prefix for `ProviderRegistry::Providers`
and confirmed a real entry exists at exactly that key. This proves the full
path -- keystore load, bootstrap-mode Falcon signing, `General`-preamble
extrinsic construction, WS submission, chain-side validation and dispatch,
and manual storage-key reads -- works end to end against a real node, not
just in unit tests.

**Not yet live-verified:** the responder's actual challenge-response loop
across real epoch boundaries (the registration test above didn't run long
enough to reach one), and the full D.12 D3 acceptance scenario (3 provider
daemons + 1 client, 50 compressed epochs, kill-one-daemon →
`report_missed` → slash → `Degraded`) -- a substantially larger multi-daemon
integration exercise than a single sitting's live-verification pass. The
unit-level proof-generation correctness (bao slices the real host function
accepts) and the chain-interaction correctness (this section) are both
solid; the remaining gap is specifically the multi-daemon, multi-epoch
choreography.

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

## Bugs found & fixed (D4)

- **B-D4.1** `create_deal_and_push_shards` pushed shard bytes immediately
  after submitting `create_deal`, in the same call. This is wrong on two
  counts, both found by actually running it against a live node rather than
  trusting a code read: (1) `submit_extrinsic` (`author_submitExtrinsic`) is
  fire-and-forget — it returns as soon as the pool accepts the extrinsic, not
  once it's mined — so the push fired before the deal even existed on-chain;
  (2) `accept_shard` is first-come-first-served among *any* active provider
  (`pallets/storage-deals::accept_shard`), so the client has no on-chain say
  in who ends up holding a given `shard_idx` — pushing to a client-side
  `discover_active_providers` guess can target the wrong provider even once
  timing is fixed. Every live push attempt failed with `NotAssigned`
  (`protocol.rs`'s `ShardAuthority` check correctly requires an `Accepted`
  assignment to already exist — this was the push protocol working as
  designed against a client that was calling it too early). **Fixed** by
  splitting into `create_deal` (submit, then poll `Deals` until mined) and
  `push_shards_for_deal` (poll each shard's `ShardAssignments` entry until
  `Accepted`, then read back the *real* assignee and push to them). Live
  re-verified afterward: `deal-create` now correctly waits, and the push that
  does fire lands real bytes in the real assignee's blob store (see D4 live
  verification above).

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

- **C-D2.1 — Devnet block production (live-verification).** ✅ **RESOLVED
  2026-07-16** — see the checkpoint section above. 16 blocks, 3 epoch
  boundaries, zero panics/errors.
- **C-D2.2 — Beacon `on_initialize` is the only always-run hook in D.** ✅
  **RESOLVED 2026-07-16** — confirmed live: epoch 1's beacon reads genesis
  (harmless, as designed) and epoch 3's beacon byte-exactly matches
  `chain_getBlockHash(epoch_3_boundary - BeaconDelay)`.
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

**Superseded by what D3 actually did (2026-07-17): the daemon reads the
beacon directly from `StorageProofs::Beacons` on-chain rather than
recomputing it from `block_hash(epoch_boundary − BeaconDelay)` itself.**
Simpler and strictly safer than the plan below -- it's the same value the
pallet already computed and stored, so there's no way for the daemon's copy
of the formula to drift from the pallet's (e.g. if `BeaconDelay` or the
saturating-subtraction edge case ever changed on one side and not the
other). The original plan is kept below for context, not as the current
approach.

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
