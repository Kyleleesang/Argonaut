# Argonaut Frontend — Full-Feature dApp on the Three.js Solar System

**Status: F0/F1 done** (`wallet/signer-core` + `wallet/signer-wasm`, node CLI rewired as thin shells; commit `3f9e8fa`). **F2 onward not started.** This is the
implementation guide for turning the solar-system WebGL experience
(`web/`, spec: `FRONTEND_SOLARSYSTEM_SPEC.md`) into a **functional Substrate
frontend** that exposes every feature of the Argonaut chain — accounts and
Falcon-512 signing, balance transfers, key registration, the Implementation-D
storage market (providers, deals, proofs, retrieval, payment channels), and
the Implementation-E surface as it lands (governance, utilities, contracts) —
with the 3D scene as the primary navigation and *live chain state driving the
visuals*.

Relationship to the other docs:
- `FRONTEND_SOLARSYSTEM_SPEC.md` — the visual/UX substrate. This spec layers
  *function* on top of it and defers to it for all rendering, a11y, SSR, and
  performance rules.
- `ARGONAUT_ARCHITECTURE_GUIDE.md` §2 (Falcon), §3 (Prism), §9 (storage) —
  the chain interfaces this frontend consumes.
- `IMPLEMENTATION_E_SPEC.md` E6 — names the tooling gap this spec closes:
  "no existing wallet, explorer, or polkadot-js signer can sign Argonaut
  transactions." The WASM signer in §4 below **is** E6's "JS via wasm"
  stretch goal, promoted to a committed deliverable.
- `DAPP_CATALOG_SPEC.md` — Tier-1 apps (Anchor, Keystone, Press) become moons
  of the Ecosystem planet; their UIs are content panels wired through the
  same chain layer this spec defines.

---

## 0. Feasibility verdict & the one hard problem

**Verdict: fully buildable today.** A Three.js scene is just a view layer;
nothing about WebGL conflicts with `@polkadot/api` or WebSocket
subscriptions. Three properties of Argonaut actually make this *easier* than
the equivalent frontend for a stock Substrate chain:

1. **No reorgs, ever** (guide §3.1: Prism confirms ε-finally; Substrate
   blocks import with `finalized: true`). The frontend needs no
   best-vs-finalized split, no reorg rollback of optimistic UI, no
   "wait for finality" spinner distinct from inclusion. `subscribeNewHeads`
   *is* `subscribeFinalizedHeads`. Every event is exactly-once. This
   collapses the hardest 20% of normal dApp state management to zero.
2. **The signer is pure Rust.** The chain's Falcon implementation is the
   `fn-dsa` crate family — pure Rust, no C — which compiles to
   `wasm32-unknown-unknown`. The browser can run *the same signing code the
   node uses*, byte-for-byte compatible, via wasm-bindgen.
3. **The visual metaphor is load-bearing.** Providers, deals, votes, and
   referenda are countable on-chain objects with natural celestial
   representations (§6). The 3D layer is not decoration over the dApp; it is
   the dApp's index page.

**The one hard problem — signing (E6).** `ArgonautSignature` is a custom
enum, `AccountId = blake3(falcon_pk)`, and the `SignedExtra` chain starts
with the custom `CheckFalconSignature` TransactionExtension
(`runtime/src/lib.rs:314`). polkadot-js can be *taught to read* this chain
with a types bundle, but teaching its JS signing pipeline to produce
Falcon-signed extrinsics means reimplementing Falcon-512 and the exact
SCALE envelope in JavaScript — a cryptographic re-implementation that would
need its own audit and can silently drift from the runtime. **Do not do
this.** The binding decision (§1-D2) is: signed-extrinsic construction
happens in Rust compiled to WASM, sharing the workspace's own crates, and
JavaScript only ever submits opaque hex.

### 0.1 Library inventory (verified 2026-07)

| Need | Library | Status / Notes |
|---|---|---|
| Chain reads: queries, subscriptions, events, metadata | `@polkadot/api` (pjs) | Mature. With a types bundle (§3.3) it reads accounts/storage/events on any custom chain. Its **signing** path is bypassed entirely (§1-D2). |
| Typed alternative for reads | `polkadot-api` (PAPI) | Modern, metadata-typed. Viable substitute for pjs reads; custom-signature chains are equally unsupported for writes, which doesn't matter under D2. Team taste call; default pjs for its ecosystem. |
| Falcon-512 in browser | `fn-dsa`, `fn-dsa-sign`, `fn-dsa-vrfy` (already workspace deps) | Pure Rust, `no_std`-capable → compiles to wasm32. Same crate the node verifies with — zero drift. |
| Rust→WASM toolchain | `wasm-bindgen` + `wasm-pack` | Stable, standard. Output: an npm package `@argonaut/signer` (§4). |
| Extrinsic construction in WASM | depend on `argonaut-primitives-falcon` + a small extracted signer crate (from `node/src/falcon_sign.rs`, per E6) | The signer crate re-uses the runtime's own `UncheckedExtrinsic`/`SignedExtra` types via feature-gated std-less build, or mirrors them behind golden-vector tests (§4.4). |
| Alternative: whole client in Rust | `subxt` (custom `Config`: AccountId, Signature, ExtrinsicParams) — runs in wasm | Legitimate architecture (thin JS facade over a subxt-wasm core). Rejected as default only because the JS read ecosystem (pjs + React hooks + explorers) is worth keeping; noted as the fallback if pjs types-bundle friction exceeds budget (§10). |
| Content verification in browser | `bao-tree` (workspace dep) compiled to wasm | Lets the browser verify streamed shard bytes against BLAKE3 roots — the client side of §D's verified retrieval, trustless even through a gateway. |
| Erasure decode in browser | `reed-solomon-simd` (has portable fallback) or `reed-solomon-erasure`, wasm build | Needed only for in-browser `fetch` reassembly (§7.3); v1 may delegate to the gateway. |
| 3D runtime | three / R3F / drei / zustand | Already running in `web/`. |
| State bridge | `zustand` | One store; chain slice + scene slice (§5). |
| Keystore crypto | WebCrypto (PBKDF2/AES-GCM) or rust `scrypt`+`chacha20poly1305` inside the signer wasm | Prefer **inside the wasm** so the decrypted seed never crosses the JS boundary (§4.3). |

**What does *not* exist and must not be assumed:** a browser extension wallet
(no injected `window.injectedWeb3` will ever hold Falcon keys until we ship
one), and direct browser↔iroh connectivity (QUIC; §7's gateway is the v1
answer, with iroh-in-browser noted as an upgrade path to re-verify against
upstream at build time).

---

## 1. Binding decisions

- **D1 — The scene is the nav; DOM panels are the dApp.** All transactional
  UI (forms, tables, confirmations) is HTML overlay panels per
  `FRONTEND_SOLARSYSTEM_SPEC.md` §7. The canvas never renders text-critical
  or input UI. Everything works with WebGL off (§9 of that spec) — the dApp
  must be *fully operable* through the DOM fallback.
- **D2 — JavaScript never assembles or signs an extrinsic.** Key generation,
  keystore encryption, address derivation (`blake3(pk)` → SS58), signing
  payload construction, SCALE encoding of `UncheckedExtrinsic` (both
  bootstrap `FalconWithKey` and registry-mode `Falcon` flavors) all happen
  inside the `@argonaut/signer` wasm module built from the workspace's own
  crates. JS receives opaque `0x…` bytes and submits them via
  `author_submitAndWatchExtrinsic`. Reads flow through `@polkadot/api`.
- **D3 — One store, unidirectional flow.** Chain subscriptions write into a
  zustand `chain` slice; the R3F scene and DOM panels are both pure
  consumers. The scene never calls the API; panels never touch three.js.
  (Mirrors the solar-system spec's state rule and keeps 60 fps insulated
  from RPC jitter, §8.)
- **D4 — File bytes never touch the chain layer** (guide §9.1 rule 1
  restated for the frontend): uploads/retrievals speak to an
  **argonaut-gateway** (§7) or future in-browser iroh, and only hashes,
  deal ids, and vouchers cross into the chain layer.
- **D5 — The types bundle is CI-pinned to the runtime.** A metadata-drift
  test (§3.3) fails the web build when `spec_version` changes without a
  regenerated bundle. Golden signing vectors (§4.4) pin the wasm signer to
  the node the same way.
- **D6 — Feature flags per milestone.** Governance/contracts panels ship
  dark behind flags keyed on pallet presence in metadata
  (`api.tx.referenda !== undefined`), so the frontend tracks the E-spec
  rollout without forked branches.

## 2. Architecture (layers)

```
┌────────────────────────────── browser ───────────────────────────────┐
│                                                                       │
│  R3F scene (web/src/…)          DOM panels (wallet, market, gov)      │
│  bodies ← live visual state     forms/tables ← typed view state       │
│        ▲                                ▲            │ user intents    │
│        └────────────┬───────────────────┘            ▼                │
│                zustand store                  action creators          │
│      chain slice │ scene slice │ wallet slice        │                │
│        ▲         ▲          ▲                        ▼                │
│  ┌─────┴─────────┴──────────┴────┐   ┌──────────────────────────┐    │
│  │ chain service (@polkadot/api) │   │ @argonaut/signer (WASM)  │    │
│  │ connect · types bundle ·      │   │ keygen · keystore ·      │    │
│  │ queries · subscriptions ·     │   │ derive SS58 · SCALE ·    │    │
│  │ events · metadata flags       │   │ Falcon sign · extrinsic  │    │
│  └───────────────┬───────────────┘   └───────────┬──────────────┘    │
│                  │ submit raw hex  ◄──────────────┘                   │
│                  ▼                                                    │
│           WebSocket :9944                 ┌───────────────────────┐   │
│                  │                        │ storage client (§7)   │   │
│                  │                        │ upload/fetch/vouchers │   │
└──────────────────┼────────────────────────┴──────────┬─────────────┘
                   ▼                                    ▼ HTTPS
        argonaut node (RPC)                    argonaut-gateway ⇄ iroh
        Prism → bridge → runtime               providers (D daemons)
```

Directory plan (extends the existing `web/`):

```
web/src/
  chain/
    api.ts            # connect, reconnect, health; exposes ApiPromise
    types-bundle.ts   # generated: ArgonautSignature, pallet types, extensions
    balances.ts       # queries + subs: free/reserved, transfers feed
    falconRegistry.ts # Keys map, registration status of current account
    storage.ts        # providers, deals, shard assignments, proof events
    channels.ts       # payment channels for current account
    governance.ts     # (flagged) referenda, council, treasury
    events.ts         # system.events fan-out → store actions
  wallet/
    signer.ts         # thin TS facade over @argonaut/signer wasm
    keystore.ts       # IndexedDB persistence of *encrypted* blobs only
    accounts.ts       # account list, active account, registration mode
  store.ts            # zustand: chain/scene/wallet slices
  scene/              # existing bodies, now data-driven (§6)
  ui/                 # panels: WalletPanel, MarketPanel, DealWizard, …
packages/
  argonaut-signer/    # Rust crate → wasm-pack → npm workspace package
gateway/              # argonaut-gateway daemon (Rust, §7) — optional co-repo
```

## 3. Chain connectivity layer

### 3.1 Connection
`ApiPromise.create({ provider: new WsProvider(endpoints), typesBundle })`
against the node's standard RPC (`:9944`, already exposed in
`docker-compose.yml`). Multi-endpoint array with backoff; connection state
(`connecting/ready/degraded`) lives in the store and is *visualized* — the
Core star dims and its pulse stops when the RPC is lost (honest by design).

### 3.2 Reads & subscriptions (the live feed)
- `rpc.chain.subscribeNewHeads` — block ticker. Drives the Core pulse (§6).
  Because of no-reorg finality, height is monotone; assert it in dev builds.
- `query.system.events` (subscription) — single fan-out point. Route by
  `event.section`: `balances`, `falconRegistry`, `providerRegistry`,
  `storageDeals`, `storageProofs`, `paymentChannels` → store actions.
  Exactly-once by block hash memo (trivial without reorgs).
- `query.*.entries()` + targeted `query.*.multi` — initial hydration of
  provider list, open deals, own channels; then maintained incrementally by
  events (never re-scan per block).
- `derive`-style computed state (registration mode of active account, vested
  escrow claimable, next challenge epochs) computed in service modules, not
  components.

### 3.3 Types bundle & drift protection
Generate a pjs types bundle covering: `ArgonautSignature` (enum + BoundedVec
newtypes `FalconPublic`/`FalconSignature`), `Address = MultiAddress`,
pallet-D types (`DealParams`, `ShardSpec`, deal state enum, …), and the
`signedExtensions` entry for `CheckFalconSignature` (declared with empty
extra/additional payload as implemented — verify against the runtime, this
is exactly the kind of assumption §D5's test exists to catch). Read-only pjs
apps compatibility (E6's deliverable) falls out of this file for free —
publish it as `@argonaut/types-bundle`.

CI: a script starts a dev node, fetches metadata, and diffs
`(spec_version, tx_version, registered type hashes)` against the bundle's
recorded values; mismatch fails the build with "regenerate bundle."

## 4. `@argonaut/signer` — the WASM wallet core (closes E6)

### 4.1 Crate
`packages/argonaut-signer/` (Rust, `crate-type = ["cdylib", "rlib"]`),
depending on `argonaut-primitives-falcon` and the extracted signing logic
from `node/src/falcon_sign.rs` (E6 already mandates that extraction into a
reference SDK — this crate is its wasm face). wasm-bindgen exports:

```rust
#[wasm_bindgen]
pub fn generate_keypair(seed_entropy: &[u8]) -> KeypairHandle;      // in-wasm keygen
pub fn derive_address(pk: &[u8], ss58_prefix: u16) -> String;       // blake3(pk) → SS58
pub fn keystore_seal(handle, passphrase) -> Vec<u8>;                // scrypt+AEAD blob
pub fn keystore_open(blob, passphrase) -> KeypairHandle;            // secrets stay in wasm
pub fn sign_extrinsic(                                              // THE entry point
    handle: &KeypairHandle,
    call_scale: &[u8],        // SCALE RuntimeCall bytes from pjs: api.tx.….method.toU8a()
    ctx: &SigningContext,     // nonce, era+block hash, tip, genesis, spec/tx versions
    mode: Mode,               // Bootstrap (FalconWithKey) | Registry (Falcon)
) -> Vec<u8>;                 // full SCALE UncheckedExtrinsic, ready to submit
```

The JS side gets call bytes from pjs (which knows the metadata) and never
sees key material: `api.tx.balances.transferKeepAlive(dest, amount)` →
`.method.toU8a()` → wasm → `api.rpc.author.submitAndWatchExtrinsic(hex)`.
This split gives pjs's ergonomic call construction *and* Rust's signature
correctness — each side doing only what it is good at.

### 4.2 Two-mode account lifecycle (mirrors guide §2.1)
1. New account: keygen in wasm → SS58 shown → user funds it (faucet/exchange).
2. First transaction signs in **bootstrap mode** (`FalconWithKey`, ~1.7 KB);
   the runtime auto-registers the key (A-4 behavior).
3. Wallet watches for `falconRegistry.KeyRegistered` for its account, then
   flips to **registry mode** (~800 B) permanently. The mode is *displayed*
   (Citadel station docking-clamp lights: amber = bootstrap, green =
   registered — §6) because fee differences are user-visible (E1/E6 docs).

### 4.3 Keystore rules (normative)
- Decrypted seeds live only inside wasm linear memory; JS holds opaque
  handles. Zeroize on lock/timeout. **Honesty note:** wasm memory is a
  hygiene measure against *accidental* leaks (GC copies, heap dumps), NOT a
  confidentiality boundary against code in the same page — wasm-bindgen
  exposes the memory object, and same-page JS can call signer exports while
  unlocked. The real boundary is the custody ladder in §9.1.
- All entropy (keygen seeds, signing randomness, KDF salts) originates from
  `crypto.getRandomValues` fed into wasm. A JS PRNG anywhere in a key path
  is a release blocker.
- Keystore sealing: Argon2id (memory-hard, parameters recorded in a
  versioned keystore format with test vectors) + AEAD. Where the platform
  supports the **WebAuthn PRF extension**, the keystore key is additionally
  wrapped by a platform-authenticator-derived secret (feature-detected,
  passphrase-only fallback) — a stolen IndexedDB blob alone is then
  insufficient for offline brute force.
- IndexedDB stores sealed blobs only; export/import = the blob file
  (documented recovery flow).
- Signing requires explicit per-transaction user approval rendering the
  decoded call (pjs `call.toHuman()`); unknown call indices refuse to render
  → refuse to sign. At custody Tier 1+ (§9.1) the approval surface lives on
  the wallet origin (popup), out of the dApp page's reach. The scene may
  *celebrate* a transaction but never initiates one.
- CSP: `wasm-unsafe-eval` only as needed, no remote script origins; the
  signer package is bundled, never CDN-loaded.
- Falcon's floating-point sampler is a known side-channel research area;
  `fn-dsa` targets constant-time behavior and browsers deny high-resolution
  cross-origin timers, but the sampler is explicitly in external-audit scope
  (§9.2).

### 4.4 Golden vectors (the drift firewall)
The node gains a `falcon-sign --test-vectors` mode emitting
`(seed, call_scale, ctx, expected_extrinsic_hex)` tuples for both modes;
`argonaut-signer`'s CI (native *and* wasm via `wasm-pack test --headless`)
must reproduce them bit-exactly. Runtime bumps regenerate vectors. This is
the frontend's equivalent of the guide's byte-identical-chains test.

## 5. Store & data flow

```ts
interface ChainSlice {   // written ONLY by chain services
  connection: 'connecting'|'ready'|'degraded';
  head: { number: number; hash: string; timestamp: number };
  providers: Map<AccountId, ProviderVis>;   // stake, capacity, faults, status
  deals: Map<DealId, DealVis>;              // state machine position, shards
  proofsSeen: RingBuffer<ProofEvent>;       // last N, feeds ring shimmer
  referenda?: ReferendumVis[];              // flag-gated (E2)
  txVolume: EMA;                            // events/block, feeds city lights
}
interface WalletSlice {  // written by wallet actions
  accounts: AccountMeta[]; active?: AccountId;
  mode: 'none'|'bootstrap'|'registered';
  balance: { free: bigint; reserved: bigint };
  channels: ChannelVis[];
  pending: TxStatus[];                      // broadcast→inBlock (== final)
}
```

Rules: services **throttle** high-frequency writes (provider set diffs and
proof events batch per block, not per event); scene components subscribe via
narrow selectors; DOM panels use the same selectors — one truth, two views
(D3). Transactions are store actions: `actions.transfer(dest, amount)` →
signer wasm → submit → `pending` entry → resolved by its `system.events`
inclusion (no separate finality stage — no-reorg dividend again).

## 6. Chain state → celestial visualization (the feature map)

The point of doing this in Three.js. Every mapping below is driven by the
`ChainSlice` — remove the RPC and the system visibly goes still.

| Chain feature | Data source | Visualization |
|---|---|---|
| Block production | `subscribeNewHeads` | **Core neutron-star pulse**: one corona pulse + beam sweep per Substrate block (i.e., per confirmed Prism leader level). Steady rhythm = healthy chain; stall = visibly dimming core. |
| Prism structure (education) | static params + head | **Prism planet**: m voter-chain light-bands orbiting it; a vote streak animation per block. Panel explains no-reorg confirmation (guide §3). |
| Transaction volume | events EMA | **Ecosystem city lights** emissive intensity scales with recent tx rate; dApp moons (Anchor/Keystone/Press) brighten when their contracts emit events (catalog integration). |
| Storage providers | `providerRegistry.Providers` | **Aegis ring asteroids, one rock per provider** (the existing `AsteroidField` becomes data-driven): size ∝ capacity, brightness ∝ stake, red flash on slash event, tumble-out-of-ring animation on deregistration. Hover → provider card (terms, faults, uptime). |
| Deals lifecycle | `storageDeals.Deals` + events | **Ring bands**: each active deal renders as a faint arc segment; state machine position = color (Assigning amber → Active gold → Degraded red-pulsing → Expired fade). Click → deal panel with shard table. |
| Proof heartbeat | `storageProofs` events | Per-proof **glint** on the corresponding provider rock; `report_missed` = red streak from ring toward the Core (slash flows to escrow/reporter). |
| Wallet / security | wallet slice | **Citadel station** is the account hub: docking clamps amber/green for bootstrap/registered mode (§4.2); habitat-ring speed ∝ pending txs; the station panel is the wallet UI (balances, send, register, history). |
| Payment channels | `paymentChannels` + retrieval client | **Photon stream** from Citadel to the provider rock currently serving a fetch; stream density ∝ voucher rate; channel close = stream collapse + settle event flash. |
| Governance (E2, flagged) | `referenda`, `council`, `treasury` | **Assembly planet** (from the solar-system spec table): ring segments light per active referendum, fill level = tally; treasury balance = planet's aurora intensity. Panels: vote (conviction UI), proposals. |
| Contracts (E4, flagged) | `contracts` events | **Forge planet** grid-lights flicker on contract instantiation/calls; developer panel links docs + code-hash explorer. |
| Chain health | connection state | Global: RPC loss desaturates the whole system and freezes orbital motion (reduced-motion-compatible), with a DOM banner — never a silently stale UI. |

Performance guardrails (inherits solar-system spec §8): provider rocks are
already an `InstancedMesh` — per-instance color/scale updates only on diff;
event-driven effects pooled (max N concurrent glints); all visual state
changes lerped in the render loop from store snapshots, never per-event
React re-renders of scene graph.

## 7. Storage market end-to-end (Implementation D in the browser)

### 7.1 The gateway (v1 data plane)
Browsers cannot speak iroh QUIC. Ship **`argonaut-gateway`** — a small Rust
daemon (reuses `client/argonaut-store` + the §D.7 wrapper traits) exposing:

```
POST /upload           multipart file → RS-encode → serve shards to providers
                       → returns { manifest_root, shard_roots[], deal_params_draft }
GET  /fetch/{manifest} → streams reassembled file
GET  /shard/{root}?range= → bao-encoded verified slices (pass-through)
WS   /retrieve/{manifest} → §D.6 voucher loop: server streams verified chunk
                       groups, client returns Falcon-signed vouchers per stride
```

Trust model, explicitly documented: the gateway is **untrusted for
integrity** — the browser verifies bao slices against the manifest root
in wasm (`bao-tree`; §0.1) — but *is* trusted for availability/privacy of
the specific session. Anyone can run one (providers will, as a service);
the frontend has a gateway picker + self-host docs. Upgrade path: replace
the gateway with iroh-in-browser when upstream support is production-ready
(re-verify at F3; the storage client interface in `web/src/chain/storage.ts`
is written against a `DataPlane` interface so the swap is one adapter).

### 7.2 Deal creation wizard (client flow, mirrors §D.8 `deal create`)
1. Select file → gateway `/upload` → manifest + shard roots + size/cost quote
   (n, k, duration, price from provider terms in the store).
2. Panel shows provider auto-selection (rank: stake, faults, price; user can
   pin/exclude — rocks highlight in the ring as they're chosen).
3. Sign `create_deal` (escrow shown explicitly) → watch `Assigning`: shard
   acceptance events light shards one by one on the deal arc → `Active`.
4. Deal detail panel tracks: per-shard `last_proven_epoch` heartbeat, vested
   vs claimed escrow, degraded/repair events; `terminate` button with
   pro-rata refund preview (formulas from the deals pallet, computed by a
   shared TS lib generated from `parameters.rs` constants — do not hardcode).

### 7.3 Retrieval with vouchers
Fetch flow: open channel (`paymentChannels.open`) if none → WS retrieve →
per-stride voucher signing via the **same wasm signer** (vouchers are
`(channel_id, cumulative) + ArgonautSignature` — off-chain, so signing is
silent-approved within a user-set per-session budget, with the running total
live in the panel and the photon stream visual) → gateway settles the final
voucher on-chain (or client does). v1 delegates RS-decode to the gateway
with bao verification in-browser; a later flag moves decode client-side.

### 7.4 Provider operator view
A dashboard panel (their rock, enlarged): registration/terms management
(`register`, `update_terms`, `top_up_stake`), assigned shards with challenge
calendar (§D.5 selection formula reimplemented in TS for *display only* —
consensus stays with the daemon), earnings claims, fault history. This makes
the frontend double as the provider ops UI the D-spec daemon lacks.

## 8. Milestones (each shippable; feature-flag the rest)

- **F0 — Read-only live system.** Types bundle + chain service + store;
  Core pulses per block, provider rocks & deal arcs render from real devnet
  state; connection-loss desaturation. No keys, no signing.
  *Acceptance:* against a 4-node devnet, kill the RPC node → visual
  degradation + banner within 5 s; restart → full recovery, no refresh.
- **F1 — WASM signer.** `packages/argonaut-signer` with §4.1 API; golden
  vectors green native+wasm; bundle ≤ 1.5 MB gz (measure; fn-dsa keygen is
  the fattest path — if over budget, lazy-load keygen separately from sign).
  *Acceptance:* E6's external-party test executed with this signer in a
  browser: create key → fund → bootstrap tx → registry-mode transfer, on
  public devnet, docs only.
- **F2 — Wallet UX (custody Tier 0, beta-capped).** Citadel = wallet:
  create/import/lock, send flow with human-readable call preview, mode
  indicator, tx history from events; §4.3 keystore rules incl. entropy
  mandate and Argon2id format (PRF wrapping may land in F6).
  *Acceptance:* Playwright e2e: fresh browser → funded account → transfer →
  balance updates without reload; keystore blob survives export/import;
  beta badge + soft caps verifiably active.
- **F3 — Storage market.** §7.1 gateway + §7.2 wizard + §7.4 operator panel;
  Aegis fully data-driven.
  *Acceptance:* D3-style compressed-epoch devnet: create deal from browser,
  watch activation glints, kill a provider daemon → rock reddens on
  `report_missed`, repair recolors it; escrow math in panel matches chain
  events to the unit.
- **F4 — Retrieval + channels.** §7.3 flow with live voucher stream visual.
  *Acceptance:* 100 MiB fetch in-browser with bao verification; stopping
  vouchers halts the stream within one stride (mirrors D4).
- **F5 — Governance & contracts (tracks E2/E4).** Assembly + Forge panels
  behind metadata flags; conviction voting UI; contract event lights.
  *Acceptance:* referendum lifecycle visible end-to-end on an E2 devnet.
- **F6 — Production hardening.** Astro island wrap (SSR content per
  solar-system spec §2/§9), a11y pass for every panel (they're DOM — hold
  the line), Lighthouse ≥ 90, bundle gates, gateway self-host docs, and the
  security graduation: **custody Tier 1** (wallet origin + popup approvals,
  §9.1), WebAuthn-PRF keystore wrapping, dual-endpoint head sanity check,
  and the §9.2 external audit engaged with findings triaged.
  *Acceptance:* solar-system spec §15 definition-of-done plus: full dApp
  operable keyboard-only and with WebGL disabled; a scripted XSS payload
  injected into the dApp page cannot read key material or produce a
  signature without a user click in the wallet-origin popup.

Dependency spine: F0 → F1 → F2 → {F3 → F4} ∥ F5 → F6. F0 needs only the
existing devnet; F3+ want the D-milestone daemons (D3+); F5 is gated on E2/E4
landing — ship dark until then (D6).

## 9. Security posture (frontend-specific)

### 9.1 Custody isolation ladder (normative)

In-page key custody cannot be made safe against page compromise by
cryptographic hygiene alone (§4.3 honesty note). The wallet therefore climbs
an explicit ladder; each tier reuses the same `@argonaut/signer` core:

| Tier | Where keys live | Boundary | Status |
|---|---|---|---|
| 0 — in-page | dApp page (wasm) | none vs page compromise | F2 deliverable. **Beta/testnet only**: UI-enforced soft value caps, visible "beta custody" badge. |
| 1 — wallet origin | `wallet.<domain>` context; dApp ⇄ wallet via a narrow, versioned `postMessage` protocol; **approvals in a popup window on the wallet origin** (popup, not iframe — overlay/clickjack-proof) | browser same-origin policy | **F6 requirement** — production default before mainnet-value use. |
| 2 — extension | MV3 service worker embedding the signer crate; injected provider; extension-owned approval UI | process + origin isolation | Post-1.0 roadmap; the crate API (§4.1) is designed to embed unchanged. |
| HW | secure element | physical | Not currently feasible for Falcon (floating-point sampling on constrained secure elements is an open problem). Documented ecosystem limitation; CLI/offline signer (`argonaut-sign`) is the interim high-value path. |

### 9.2 Process gates (release blockers, in order)

1. Golden vectors (§4.4) green native+wasm — every commit.
2. Reproducible wasm build: pinned toolchain, artifact hash asserted in CI
   and published; npm provenance attestation on `@argonaut/signer` and
   `@argonaut/types-bundle`.
3. **External security audit** before Tier-1 launch / any mainnet-value
   default. Scope: signer crate (key lifecycle, zeroization, entropy,
   Falcon sampler side channels), keystore format & PRF wrapping, approval
   flow, the Tier-1 postMessage protocol.
4. Public bug bounty from Tier-1 launch; beta value caps removed only after
   audit findings closed.

### 9.3 Threat table

| Threat | Mitigation |
|---|---|
| Key exfiltration via XSS | Custody ladder §9.1 (Tier 1+ is the real defense); strict CSP; no third-party script origins; signer bundled+hashed; wasm hygiene (§4.3) against accidental leaks. |
| Malicious RPC endpoint | Reads are integrity-light by nature (v1 accepts trusted-RPC like every dApp; document it). Signing payload is constructed client-side from user-approved calls, so a hostile RPC cannot alter intent — worst case is lying reads → banner on cross-endpoint head mismatch (cheap dual-endpoint sanity check, ship in F6). |
| Malicious gateway | bao verification in-browser (integrity); voucher budget caps exposure to one stride (already the D-design); gateway picker + self-host. |
| Call-data confusion (user signs wrong thing) | Mandatory human-readable decode in the approval modal from *local* metadata; unknown call indices refuse to render → refuse to sign. |
| Types-bundle / signer drift after runtime upgrade | §3.3 CI metadata diff + §4.4 golden vectors; both keyed to `spec_version`. |
| Supply chain (npm) | Signer + types bundle are workspace-internal packages; lockfile-pinned; no wallet deps from npm at all. |

## 10. Risk register

| Risk | Severity | Mitigation |
|---|---|---|
| **fn-dsa is pre-standard by its author's own declaration** (pinned 0.2.0; README: keys/signatures "may cease to be accepted by ulterior versions"; stability only at 1.0, after FIPS 206 final — expected late 2026/early 2027). Applies equally to every Falcon impl anywhere; not a crate-specific defect. | H if mainnet launches before FIPS final; L before that | Chain-level bet already taken by Implementation A; frontend adds no new exposure (same pinned crate, golden vectors keep wallet/node in lockstep). **Gates:** mainnet-value default requires FIPS 206 final + fn-dsa 1.0 + §9.2 audit. Migration plan is *more* than the guide's "one-file fix": legacy verify host fn kept forever (historical re-sync), `verify_v2` host fn + new `ArgonautSignature` variant, dual-scheme window with wallet key-rotation UX. Track upstream (0.3.0 exists) and bump while pre-mainnet keys are still disposable. |
| pjs types-bundle friction with the custom signature/extension (reads or `.method.toU8a()` breaking) | M | D2 already minimizes pjs's role to reads+call-encoding; fallback documented in §0.1: subxt-wasm core with custom `Config`, JS facade unchanged (the store/service interfaces are the stable seam). |
| WASM signer bundle size / keygen latency on low-end mobile | M | Lazy-load; keygen in a worker; measure at F1 gate, not after. |
| Gateway becomes a centralization/abuse chokepoint | M | Anyone-can-run + picker (§7.1); iroh-in-browser upgrade path re-checked each milestone; retrieval remains verifiable regardless. |
| Scene perf collapse under busy-chain event storms | M | §5 throttling rules + pooled effects; F0 acceptance includes a synthetic 50-events/block soak. |
| E-spec churn (governance shape changes) | L | D6 metadata flags; F5 panels are additive. |
| In-browser key custody is inherently weaker than the CLI keystore | Managed via §9.1 ladder | Tier 0 is beta-capped by design; Tier 1 (origin isolation + popup approvals) is the production bar, gated by external audit (§9.2); CLI/offline signer (`argonaut-sign`, guide §2.4) documented for high-value ops; Tier-2 extension embeds the same audited crate. |

## 11. Handoff prompts (§8-of-the-guide style)

- **P-F0:** "Implement §3 (chain service, types bundle, CI drift test) and
  §5–§6 read-only visual bindings in `web/`, per D1/D3/D5. Deliver F0
  acceptance against docker-compose devnet."
- **P-F1:** "Extract signing from `node/src/falcon_sign.rs` into
  `packages/argonaut-signer` per §4 (wasm-bindgen API verbatim), add node
  `--test-vectors` mode, deliver golden-vector CI native+wasm and the E6
  external-party walkthrough doc."
- **P-F2:** "Implement §4.2/§4.3 wallet UX in the Citadel panel with the D2
  submission path; Playwright e2e per F2."
- **P-F3/F4:** "Implement `gateway/` per §7.1 (reuse client/argonaut-store
  behind the DataPlane trait), the deal wizard, operator dashboard, and
  voucher retrieval per §7.2–7.4; deliver F3/F4 acceptance on a D3 devnet."
- **P-F5/F6:** as milestones land upstream; F6 additionally executes the
  solar-system spec's M5 checklist against the now-functional site.

Rules to restate every prompt: JS never signs (D2); scene reads the store
only (D3); every milestone lands with its acceptance test; visual mappings
must degrade honestly on RPC loss (§6 last row).

## 12. Definition of done

Every runtime feature reachable in ≤ 3 interactions from its celestial body;
all of it operable with WebGL off, keyboard-only, and screen-reader
navigable; no signature bytes ever produced outside the wasm signer; golden
vectors and metadata CI green against the pinned runtime; F0–F4 acceptance
suites in CI against dockerized devnets; the Aegis ring showing real
providers within 5 s of connect on a cold cache; and an external party
completing the E6 walkthrough (key → fund → register → transact → store →
retrieve) using only the deployed site and public docs.

---

### Appendix — why this is *more* than a skin

A stock Substrate dApp shows tables of what the chain did. Because Prism
confirmation is ε-final and the D-platform's actors (providers, deals,
proofs, channels) are countable objects with continuous lifecycles, Argonaut's
state *is* a dynamical system — bodies appearing, orbiting, glinting,
faulting, being repaired. The solar system isn't a metaphor bolted onto an
explorer; it is a truthful real-time render of consensus and storage economics.
That is the demo nobody else can copy without also building the chain.
