# Implementation E — Production Bootstrap (0→1): Pallets & Utilities

**Status: committed spec (2026-07-09).** This document turns
`docs/POTENTIAL_PALLET_INTEGRATIONS.md` (the *menu*, 2026-07-08) into a
committed, sequenced implementation plan. Where the two disagree, **this spec
wins**. It is written to be implementable milestone-by-milestone without
re-deriving decisions ("Sonnet-ready", same contract as
`IMPLEMENTATION_D_SPEC.md`).

Scope: everything needed to take the chain from the current devnet runtime to
a credible production mainnet, **excluding** the Prism consensus work (done,
Implementation B), the STARK aggregation layer (optional, Implementation C),
and the storage platform (Implementation D, separate spec). E is orthogonal to
D and can proceed in parallel; E2 (governance) should land before D's pallets
ship to a real network so their parameters are governable.

---

## E.0 Binding rules

1. **One polkadot-sdk pin.** Every pallet added here comes from the same
   `stable2412` branch already pinned in the root `Cargo.toml`. No version
   mixing, no crates-io FRAME crates alongside the git pin.
2. **The Prism seam is inviolable.** Prism decides transaction *order*
   (client-side); Substrate decides transaction *effect*. Therefore the
   following are **permanently out of scope**, not deferred:
   `pallet-session`, `pallet-staking` (+ `nomination-pools`,
   `election-provider-multi-phase`, `bags-list`), `pallet-authorship`,
   `pallet-aura`/`pallet-babe`, `pallet-grandpa`, `pallet-im-online`,
   `pallet-offences`, and the entire XCM/Cumulus stack (standalone chain).
   Any "block author reward" mechanism must originate from the Prism bridge
   (`node/src/bridge.rs`, which knows the `LeaderDigest`), never from FRAME
   authorship. Reward design is deliberately **not** in E — see E.8.
3. **No new signature assumptions.** All accounts are
   `blake3(falcon_pk)`-derived; `ArgonautSignature` is the only production
   signature type (`dev-signatures` gates sr25519 for tests only). Every
   pallet added must be audited for one thing: does it ever *verify a
   signature* or *derive an account that must later sign*? Pallets whose
   derived accounts never sign (multisig, proxy-delegated, treasury PalletId,
   contract accounts) are fine — record the audit result per pallet in the
   milestone notes.
4. **Fee/byte reality: Falcon transactions are big.** A bootstrap-mode
   extrinsic carries ~1.65 KB of signature+key; registry-mode ~0.75 KB.
   `LengthToFee` therefore dominates the fee for ordinary transfers and must
   be priced deliberately (E1), not left at `IdentityFee`.
5. **Milestones are independently shippable.** Each bumps `spec_version` by 1
   (currently 100). Each ends with a **live devnet run** — a runtime built,
   a chain launched (or upgraded via `set_code`), and the milestone's
   acceptance transactions executed against it. Code-complete without the
   devnet run is not "done".
6. **Weights:** new pallets ship with their upstream
   `pallet_*::weights::SubstrateWeight<Runtime>` immediately (never `()`),
   and are re-benchmarked on Argonaut reference hardware in E1 (or at the
   next E1 re-run after they land). Argonaut's own pallets keep placeholder
   weights until E1.
7. **Constants pattern:** runtime-tunable parameters go through
   `parameter_types!`/`#[pallet::constant]`; magic numbers get a named
   constant with a comment stating its unit and rationale.
8. **Docs discipline:** each milestone updates `docs/ARCHITECTURE.md` §2
   (crate map) and the baseline table in `POTENTIAL_PALLET_INTEGRATIONS.md`
   §0 when the runtime pallet set changes.
9. **(Added 2026-07-16) Mainnet-value launch is gated on FIPS 206 final +
   `fn-dsa` 1.0.** The chain's Falcon verifier (`fn-dsa`, pinned 0.2.0) is
   pre-standard by its author's own declaration; keys/signatures may cease to
   be accepted by later versions until 1.0 (expected after FIPS 206 final,
   late 2026/early 2027). E0–E2 may take a chain to *testnet* freely, but no
   milestone in this spec authorizes launching real value before that gate —
   and any post-launch scheme migration requires keeping the legacy verify
   host function forever (historical re-sync), a `verify_v2` host fn, and a
   dual-scheme window. This was project policy before this spec was written
   (see `FRONTEND_DAPP_SPEC.md` §10's risk row) and belongs here as a rule,
   not buried in a frontend risk table.

## E.1 Baseline (what exists today, verified against `runtime/src/lib.rs` 2026-07-09)

| Component | State | Production verdict |
|---|---|---|
| `frame_system`, `pallet_timestamp` | `WeightInfo = ()` | needs E1 |
| `pallet_balances` | ED = 500, upstream weights | ED needs an E0 decision |
| `pallet_transaction_payment` | `IdentityFee` both axes, `FeeMultiplierUpdate = ()`, fees **burned** (`FungibleAdapter<Balances, ()>`) | needs E1 + E2.4 |
| `pallet_sudo` | single key, root of everything incl. `set_code` | retire via E2 |
| `pallet_insecure_randomness_collective_flip` | dev-grade randomness | policy decision in E4; never for economic security |
| `pallet_falcon_registry` | `KeyDeposit = 1×ED = 500` | re-price in E0/E1 |
| `pallets/agg-verify` | in workspace, **not wired** into `construct_runtime!` | Implementation C concern, not E |
| Chain specs | `development_config` / `local_testnet_config` from `//Alice`-style seeds; `GenesisBuilder::preset_names() == []` | needs E0 |
| Node RPC | `substrate-frame-rpc-system` + `pallet-transaction-payment-rpc` wired (`node/src/rpc.rs`) | baseline OK; E4 adds contracts RPC, E5 hardens |
| SS58 prefix | 42 (generic Substrate) | needs E0 registration |
| Packaging | B-10 **done 2026-07-14**: real multi-stage `Dockerfile` + 3-node compose devnet, live-verified | E5 extends (testnet-in-a-box profile), does not duplicate |

## E.2 Milestone map

```
E0  Genesis, token identity & chain-spec hardening        (+ pallet-vesting)
E1  Weights & fee hardening                               (no new pallets)
E2  Governance: retire sudo                               (+ scheduler, preimage,
                                                            collective, membership,
                                                            treasury, referenda,
                                                            conviction-voting,
                                                            tx-pause, safe-mode)
E3  Account utilities                                     (+ utility, multisig, proxy;
                                                            optional: identity, assets,
                                                            recovery — see E.6)
E4  Smart contracts (ink!)                                (+ contracts + runtime API
                                                            + node RPC)
E5  Node, ops & release engineering                       (no pallets)
E6  Ecosystem tooling: signing SDK, explorer/wallet story (no pallets)
```

Dependencies: E1 before E4 (contracts need a sane fee model); E2.4 (fee
routing) needs E2.2 (treasury); everything else is parallelizable. E0 and E1
together are the **minimum credible testnet**; E0–E2 the minimum credible
mainnet; E3–E6 complete the production story.

---

## E.3 Milestone E0 — Genesis, token identity & chain-spec hardening

No behavioral consensus changes; this milestone makes the chain *nameable and
launchable* with real keys and real allocations.

**Decisions (committed here so nobody re-derives them):**

- **Token:** symbol `ARG`, 12 decimals. `pub const UNITS: Balance =
  1_000_000_000_000; pub const CENTS: Balance = UNITS / 100; pub const
  MILLICENTS: Balance = CENTS / 1_000;`
- **Existential deposit:** `1 * CENTS` (= 10^10 plank). The current `500` is
  a dev value; changing ED post-genesis is disruptive, so this is decided
  now. Update `EXISTENTIAL_DEPOSIT` and every constant expressed in ED.
- **Standard deposit function** (used by every deposit-taking pallet from
  here on, including re-priced `FALCON_KEY_DEPOSIT`):
  ```rust
  /// 20 CENTS per storage item + 1 MILLICENT per byte. Same shape as the
  /// polkadot-sdk convention; tune the two coefficients via governance later.
  pub const fn deposit(items: u32, bytes: u32) -> Balance {
      items as Balance * 20 * CENTS + bytes as Balance * MILLICENTS
  }
  ```
  `FALCON_KEY_DEPOSIT` becomes `deposit(1, 917)` (897 B pk + entry overhead).
- **SS58 prefix:** register a dedicated prefix in the ss58-registry (PR to
  `paritytech/ss58-registry`) and change `SS58Prefix` from 42. Until the
  registry PR merges, pick the value and hard-code it; registration is
  bookkeeping, not a blocker.
- **`pallet-vesting` joins the runtime now**, not in E3: vested genesis
  allocations must exist *at* genesis. Config: `MinVestedTransfer =
  1 * UNITS`, upstream weights, `WithdrawReasons` excluding
  `TRANSACTION_PAYMENT` (vested accounts can pay fees).

**Chain-spec work (`node/src/chain_spec.rs` + runtime `GenesisBuilder`):**

1. Implement named genesis presets in the runtime's
   `sp_genesis_builder::GenesisBuilder` (`preset_names()` currently returns
   `[]`): `dev`, `local`, `testnet`, `mainnet-stub`. Dev/local keep derived
   accounts; `testnet`/`mainnet-stub` take real Falcon public keys.
   **(Added 2026-07-16, postdates this spec:** B-16 moved the Prism consensus
   parameters into chain-spec `properties` — `prismMVoterChains`,
   `prismDifficultyShift`, `prismTargetIntervalMs`, `prismKDeepConfirmation`,
   set via `chain_spec.rs`'s `prism_properties()`. Every new preset/spec MUST
   carry these keys: `prism_params_from_properties` falls back to compiled
   defaults *silently, per-field* if a key is missing, so an omission pins the
   network to defaults with no error. Add a test asserting every preset's
   properties contain all prism keys.**)**
2. **Key ceremony tooling:** `node/src/keygen.rs` already generates Falcon
   keys; add a documented offline procedure (docs, not code, until the tool
   gap is proven): generate N root/treasury/council keys air-gapped, record
   `blake3(pk)` account IDs, feed accounts into the `mainnet-stub` preset.
   Production genesis must contain **no seed-derived accounts**.
3. Genesis allocation table lives in a checked-in `genesis/*.json` consumed
   by the preset — auditable, diffable, no numbers buried in Rust.
4. Sudo key at genesis = a **multisig-controlled account** (E3 lands
   multisig; until then, a dedicated ceremony key documented as interim).

**Tests & acceptance:**
- `chain-spec-builder` (or `argonaut build-spec`) emits specs from each preset;
  `preset_names()` lists all four.
- Unit test: `deposit(1, 917)` and ED constants match documented values.
- **Devnet run:** launch from the `testnet` preset with ceremony-generated
  Falcon keys; execute a bootstrap-mode transfer, a registry-mode transfer,
  and a `vesting.vested_transfer`; confirm vesting schedule enforcement.

## E.4 Milestone E1 — Weights & fee hardening

No new pallets. Three workstreams:

**1. Benchmarked weights.** The `runtime-benchmarks` feature is scaffolded
(`define_benchmarks!` covers system/balances/timestamp/falcon-registry).
Work: write real benchmark bodies for `pallet-falcon-registry` (register /
deregister / update, worst-case key sizes), run `frame-omni-bencher` against
the runtime Wasm on a documented reference machine, commit generated
`runtime/src/weights/*.rs`, and replace every `WeightInfo = ()`
(`frame_system`, `pallet_timestamp`, `pallet_transaction_payment`, Falcon
extensions — including `CheckFalconSignature`'s verification cost, which is
the chain's hottest path). Record the reference-hardware spec in
`docs/ARCHITECTURE.md`.

**2. Real fee curve.**
```rust
// WeightToFee: polynomial mapping ExtrinsicBaseWeight → 1 CENT base fee
// (polkadot-sdk convention, coefficients per Argonaut units from E0).
type WeightToFee = WeightToFeePolynomial-impl;   // runtime/src/fees.rs
// LengthToFee: ConstantMultiplier<Balance, TransactionByteFee>.
// Rule 4: pick TransactionByteFee so a REGISTRY-mode transfer (~0.9 KB total)
// costs a sane fee and a BOOTSTRAP-mode transfer (~1.8 KB) roughly 2×. Byte
// fees are the dominant axis on this chain — document the two worked examples
// in runtime/src/fees.rs. Start: TransactionByteFee = 10 * MILLICENTS.
// FeeMultiplierUpdate: TargetedFeeAdjustment targeting 25% block fullness
// (standard polkadot-sdk parameters; congestion response replaces flat fees).
```

**3. Fee destination placeholder.** Fees keep burning in E1 (the `()` slot);
E2.4 reroutes them. Do not build a custom fee splitter before treasury
exists.

**Tests & acceptance:**
- `cargo test -p argonaut-runtime` passes with generated weights.
- Golden test: fee for a registry-mode `balances.transfer_keep_alive` and a
  bootstrap-mode one, asserting the documented worked examples.
- **Devnet run:** submit both transfer forms; `payment.queryInfo` RPC returns
  fees matching the golden test; saturate a devnet with spam txs and observe
  the multiplier rising (congestion response live).

## E.5 Milestone E2 — Governance: retire sudo

New pallets (all `stable2412`, upstream `SubstrateWeight` initially):
`pallet-scheduler`, `pallet-preimage`, `pallet-collective` (one instance:
`Council`), `pallet-membership` (managing Council), `pallet-treasury`,
`pallet-referenda`, `pallet-conviction-voting`, `pallet-tx-pause`,
`pallet-safe-mode`.

Deliberately **not** added: `pallet-democracy` (superseded),
`pallet-bounties`/`child-bounties`/`tips` (deferred until treasury throughput
demands them).

**Sub-milestones (each shippable):**

- **E2.1 scheduler + preimage.** Preimage uses **holds**
  (`RuntimeHoldReason`) — already aggregated by `construct_runtime!`; deposit
  = `deposit(1, bytes)` via E0's function. Scheduler origin: root only, until
  E2.3.
- **E2.2 council + membership + treasury.** Council = `pallet-collective`
  instance, 5 seats at genesis (ceremony keys from E0), membership managed by
  root. Treasury: `PalletId(*b"py/trsry")`, `SpendOrigin` = council
  supermajority or root, `Burn = 0` initially, spend period 7 days
  (`7 * DAYS`). *Rule-3 audit: treasury/collective derived accounts never
  sign — pass.*
- **E2.3 referenda + conviction-voting (minimal OpenGov).** **Two tracks
  only**: `root` (runtime upgrades, parameter changes; long enactment,
  high deposit) and `treasury` (spends; shorter). Resist track sprawl — more
  tracks are a governance-era decision, not a bootstrap one. `set_code`
  moves from sudo-gated to `root`-track-gated.
- **E2.4 fee routing.** `OnChargeTransaction = FungibleAdapter<Balances,
  DealWithFees>` where `DealWithFees` sends **80% to treasury, burns 20%**.
  The block-author-reward slot stays empty per rule 2 (bridge hook, E.8).
- **E2.5 emergency controls + sudo retirement.** `pallet-tx-pause`
  (pause specific calls; pause origin = council fast-track) and
  `pallet-safe-mode` (blanket filter with whitelist; enter origin = council
  unanimous or root). Then: testnets keep sudo (chain-spec choice); mainnet
  executes `sudo.remove_key` via referendum after ≥2 successful root-track
  referenda have demonstrably executed (one being a no-op runtime upgrade).

**Tests & acceptance:**
- Mock-runtime tests: referendum lifecycle (submit → decide → confirm →
  scheduler-enact), treasury spend via council origin, tx-pause blocks a
  paused call, safe-mode filters non-whitelisted calls.
- **Devnet run:** full governance rehearsal — submit a preimage, run a
  root-track referendum that performs a real `set_code` upgrade
  (spec_version +1), vote with conviction from Falcon accounts, observe
  enactment; then `sudo.remove_key` and verify sudo calls fail thereafter.

## E.6 Milestone E3 — Account utilities

**Committed:** `pallet-utility` (batch, `as_derivative`), `pallet-multisig`,
`pallet-proxy`.
**Optional (decision point at milestone start, default *defer*):**
`pallet-identity`, `pallet-assets`, `pallet-recovery` — none blocks
production; add when a product need names them.

Config notes:
- All deposits via E0's `deposit()` (e.g. `DepositBase = deposit(1, 88)`,
  `DepositFactor = deposit(0, 32)` for multisig; proxy equivalents).
- Proxy types: `Any`, `NonTransfer`, `Governance` — three variants, no more.
- *Rule-3 audit:* multisig accounts and `as_derivative` sub-accounts never
  sign (approvals/calls come from member Falcon accounts) — pass. Proxied
  accounts never sign (the proxy signs) — pass. Document in each pallet's
  runtime-config comment that these derived accounts **cannot register a
  Falcon key and must never need to**.
- Wire the E0 sudo-key plan: root key becomes a 3-of-5 council multisig.

**Tests & acceptance:**
- Mock tests: 2-of-3 multisig dispatch, proxy add/remove/proxied-transfer,
  `utility.batch_all` atomicity with Falcon-signed outer call.
- **Devnet run:** create a 2-of-3 multisig from ceremony keys, execute a
  treasury proposal through it; rotate the genesis sudo/root arrangement to
  the multisig (if E2 not yet landed, sudo key → multisig account).

## E.7 Milestone E4 — Smart contracts (ink!)

**Committed: `pallet-contracts`** (Wasm/ink!), not `pallet-revive` — matches
current ink! tooling; revisit only if ink!'s revive backend becomes the
stable2412-era default. This is the largest single milestone; keep it last
among runtime milestones.

Three inseparable parts (menu §1's warning stands): the pallet, the
`ContractsApi` runtime API in `impl_runtime_apis!`, and the contracts RPC
wired into `node/src/rpc.rs` — without all three, `cargo-contract` dry-runs
and gas estimation don't work.

Config decisions:
- Deposits: `DepositPerItem = deposit(1, 0)`, `DepositPerByte = deposit(0, 1)`
  (E0 function), `DefaultDepositLimit` generous but bounded.
- `Randomness = RandomnessCollectiveFlip` **with a documented hazard**: it is
  dev-grade; contracts must not build economic security on it. The honest
  beacon story is Implementation D's epoch-snapshot pattern (D spec §D.13) —
  if/when D lands, revisit. This is a *known limitation*, stated in docs, not
  silently shipped.
- `CallFilter = Everything` initially; tighten via tx-pause if needed.
- Weights: contracts is the one pallet where upstream weights are least
  transferable — schedule an E1-style re-benchmark on reference hardware in
  the same milestone.
- *Rule-3 audit:* contract accounts are storage-derived and never sign — pass.
  Verify `cargo-contract`'s default address derivation produces valid
  `AccountId`s under the Falcon account model (it hashes; no curve
  assumption) — one integration test.

**Tests & acceptance:**
- Integration test: upload + instantiate + call flipper via extrinsics in a
  mock runtime.
- **Devnet run:** `cargo contract build && cargo contract instantiate` a
  flipper against the devnet through the RPC (proves the API/RPC wiring, not
  just the pallet); call it from a Falcon account; query state via dry-run.

## E.8 Milestone E5 — Node, ops & release engineering (no pallets)

- **Observability:** verify/enable Prometheus metrics and `--telemetry-url`
  in `node/src/service.rs` (spawn_tasks provides both; confirm nothing was
  stripped). Add Prism-bridge metrics (levels confirmed, bridge lag, emitted
  tx-hash count) — the bridge is Argonaut's novel component and currently
  observable only via logs.
- **Packaging:** fixplan **B-10** is done (2026-07-14: multi-stage
  `Dockerfile` + 3-node compose devnet, live-verified). E5's work is the
  extension only: a `testnet-in-a-box` compose profile using the E0
  `testnet` preset.
- **Upgrade discipline:** CI job running `try-runtime-cli
  on-runtime-upgrade` against a live devnet/testnet snapshot before any
  `set_code` referendum; the `try-runtime` runtime API already exists.
- **Deterministic Wasm:** srtool (or srtool-like docker) build in CI,
  publishing the Wasm hash so referendum voters can verify `set_code`
  payloads. Confirm srtool tolerates the git-pinned `stable2412` workspace
  (it builds the workspace as-is; no crates-io assumption — verify once).
- **RPC hardening:** document/enforce `--rpc-methods safe` for public
  endpoints; unsafe RPCs (incl. any Prism-specific ones) bound to localhost.
- **Release policy (docs):** spec_version bump rules (rule 5),
  `transaction_version` bump criteria (extrinsic-breaking changes — note the
  `SignedExtra` order is consensus-critical with `CheckFalconSignature`
  first), branch/tag scheme, and a signed-release checklist.

**Acceptance:** CI green on try-runtime + deterministic-build jobs; a
docker-compose testnet with Prometheus scraping bridge metrics; a rehearsed
`set_code` upgrade whose Wasm hash was independently reproduced.

## E.9 Milestone E6 — Ecosystem tooling: the Falcon signing gap (no pallets)

Falcon-512 signatures mean **no existing wallet, explorer, or polkadot-js
signer can sign Argonaut transactions**. This is the single biggest
production-adoption gap and it is tooling, not runtime:

- **`argonaut-signer` reference SDK:** extract the signing logic from
  `node/src/falcon_sign.rs` into a documented library + CLI flow covering
  both bootstrap-mode (`FalconWithKey`) and registry-mode extrinsics.
  Deliverable of this milestone; language bindings beyond Rust (JS via wasm)
  are a stretch goal.
- **polkadot-js types bundle:** chain metadata + a types bundle so
  polkadot-js **apps can read** the chain (accounts, events, storage) even
  though it cannot sign; document the read-only limitation.
- **Explorer story (decision, not code):** Subscan/Statescan-class explorers
  assume MultiSignature — a stock deployment will index blocks/events but
  mis-render signatures. Decide: fork-and-patch one, or ship a minimal
  first-party explorer reading via the types bundle. Default: document
  read-only polkadot-js apps as the interim explorer.
- **Registry-mode onboarding doc:** the two-step account story (bootstrap tx
  registers key → registry-mode thereafter) written for end users/exchanges,
  including fee difference from E1's worked examples.

**Acceptance:** an external party (someone not on the core team) follows the
docs to create a key, fund it, register it, and transact in registry mode
against the public testnet, using only released tooling.

---

## E.10 What E deliberately does not decide

- **Block/leader rewards & issuance schedule.** Requires bridge-originated
  crediting (rule 2) and tokenomics sign-off; spec it as "Implementation F"
  when the economics are decided. E2.4's fee split is designed to not
  prejudge it.
- **Implementation C (agg-verify) wiring.** Its runtime integration is C's
  own acceptance work.
- **Replacing dev-grade randomness chain-wide.** The honest path is the
  D-spec beacon pattern; E only documents the hazard (E.7).
- **Multi-track OpenGov, bounties, fellowship-style bodies.** Post-launch
  governance evolution.

## E.11 Summary table (committed pallet end-state after E)

| Pallet | Milestone | Deposit basis | Rule-3 audit |
|---|---|---|---|
| vesting | E0 | n/a (locks) | pass |
| scheduler, preimage | E2.1 | `deposit()` (preimage holds) | pass |
| collective(Council), membership | E2.2 | n/a | pass |
| treasury | E2.2 | n/a (PalletId account) | pass |
| referenda, conviction-voting | E2.3 | submission deposit | pass |
| tx-pause, safe-mode | E2.5 | safe-mode stake | pass |
| utility, multisig, proxy | E3 | `deposit()` | pass (derived accts never sign) |
| contracts (+API+RPC) | E4 | `deposit()` per item/byte | pass (verify address derivation test) |
| identity / assets / recovery | E3-optional, default defer | `deposit()` | pass |

Removed/retired: `pallet-sudo` (mainnet, E2.5). Unchanged but re-parameterized:
balances (ED), transaction-payment (curve + routing), falcon-registry
(deposit re-price). Permanently excluded: rule 2 list.
