# Potential Pallet & Package Integrations for Production Readiness

**Status: analysis / roadmap (2026-07-08) — superseded as a plan by
`docs/IMPLEMENTATION_E_SPEC.md` (2026-07-09), the committed, milestone-sequenced
version. Where the two disagree, the E spec wins.** This document remains the
*menu with rationale* — useful for the options that were considered and why —
but nothing here has been implemented, and implementers should work from the
E spec.

Read `docs/ARCHITECTURE.md` first for how the chain actually works today. The
single most important fact for everything below:

> **Prism decides transaction *order* (client-side); Substrate decides
> transaction *effect*.** There is no Aura/BABE/GRANDPA block production. The
> Prism bridge (`node/src/bridge.rs`) produces exactly one Substrate block per
> confirmed leader level, imported `finalized: true`, and the chain never
> reorgs.

That architecture *removes* an entire category of "standard" production pallets
and *reshapes* a few others (fees, block-author rewards). See
[§6 Deliberately NOT to add](#6-deliberately-not-to-add).

---

## 0. What exists today (baseline)

`construct_runtime!` in `runtime/src/lib.rs`:

| Pallet | Role |
|---|---|
| `frame_system` | Core |
| `pallet_insecure_randomness_collective_flip` | Randomness (dev-grade) |
| `pallet_timestamp` | Block time inherent |
| `pallet_balances` | Native token (`Balance = u128`, ED = 500) |
| `pallet_falcon_registry` | Post-quantum key registry (Argonaut-specific) |
| `pallet_transaction_payment` | Fees (`IdentityFee`, fees currently **burned**) |
| `pallet_sudo` | Single-key admin (**devnet-grade**) |

Signatures are Falcon-512 by default (`ArgonautSignature`); sr25519 exists only
behind the `dev-signatures` feature. Anything added below must not reintroduce
elliptic-curve signing assumptions or a second polkadot-sdk version (the whole
workspace is pinned to `stable2412`).

---

## 1. Smart contracts (ink!)

Pick **one** contract VM; do not ship both.

| Package | What it gives you | Notes for Argonaut |
|---|---|---|
| **`pallet-contracts`** | Wasm contracts — the classic **ink!** target | Mature, matches "ink smart contracts" exactly. Also requires the **`ContractsApi` runtime API** in `impl_runtime_apis!` **and** `pallet-contracts-rpc` wired into the node, or ink! tooling (`cargo-contract`, dry-run, gas estimation) cannot call contracts. |
| **`pallet-revive`** | PolkaVM-based contracts, EVM/Solidity-leaning; the direction Parity is steering | More future-proof but younger; ink!'s backend is transitioning toward it. |

For **EVM/Solidity** compatibility instead of/alongside ink!, that is the
**Frontier** stack (`pallet-evm`, `pallet-ethereum`, `fp-*`, plus `fc-rpc` in
the node). Heavier; only if EVM is an explicit goal.

**Recommendation:** `pallet-contracts` for ink! now (it is what "ink smart
contracts" means). Budget for the runtime API + node RPC, not just the pallet.

Argonaut-specific checks:
- Contract `AccountId` is `blake3(falcon_pk)` like every other account — contract
  address derivation must be verified against the Falcon account model.
- Contracts must respect the same weight/fee model (see §4).

---

## 2. Governance & admin — retire `pallet-sudo`

`pallet-sudo` is a single god key: fine for devnet, disqualifying for production.
Replace with the standard governance stack. `set_code` (forkless runtime
upgrades) should move from sudo-gated to governance-gated.

| Package | Role |
|---|---|
| **`pallet-scheduler`** | Delayed/dispatched calls — foundation for the rest |
| **`pallet-preimage`** | Store large call bodies by hash — required by referenda/scheduler |
| **`pallet-referenda`** + **`pallet-conviction-voting`** | Modern OpenGov (preferred). Older path: `pallet-democracy`. |
| **`pallet-collective`** | Council / technical committee |
| **`pallet-membership`** | Manage collective membership sets |
| **`pallet-treasury`** | Fund development from fees/inflation |
| `pallet-bounties` / `pallet-child-bounties` / `pallet-tips` | Optional treasury spending mechanisms |

Keep `pallet-sudo` only behind a chain-spec flag for testnets.

---

## 3. Account & asset utilities (the common utilities)

Near-universal production pallets, all compatible with the Prism/Falcon setup:

| Package | Role |
|---|---|
| **`pallet-utility`** | Batching, `as_derivative` |
| **`pallet-multisig`** | Multi-signature accounts (mitigates single-key custody risk) |
| **`pallet-proxy`** | Delegated / keyless access, cold-wallet patterns |
| **`pallet-identity`** | On-chain identities + registrars |
| **`pallet-assets`** | Fungible tokens beyond the native balance |
| **`pallet-vesting`** | Locked / vesting balances (essential for token distribution & genesis allocations) |
| `pallet-recovery` | Social account recovery (nice-to-have) |

---

## 4. Fee & economic hardening (production-blocking, mostly not new pallets)

Three things in the current runtime are devnet-grade:

1. **Weights are placeholders.** `WeightInfo = ()` on `frame_system`,
   `pallet_timestamp`, `pallet_transaction_payment`, and the Falcon extensions;
   Argonaut's own pallets use `Weight::from_parts(10_000, 0)` stubs. Production
   needs **benchmarked weights** via `frame-benchmarking` +
   `frame-omni-bencher` (the `runtime-benchmarks` feature is already scaffolded —
   it needs to be *run* and the generated `weights.rs` wired in).
2. **Fees are burned.** `OnChargeTransaction = FungibleAdapter<Balances, ()>` —
   the `()` sends fees nowhere. Route them to `pallet-treasury` and/or a
   block-author reward.
   - **Caveat:** there is no `pallet-authorship`, so author rewards cannot come
     from a standard authoring pallet. The Prism bridge is what knows the leader
     (`LeaderDigest`), so any block-reward mechanism must originate from the
     bridge, not FRAME authorship.
3. **Flat fee model.** `IdentityFee` is 1:1 weight→fee with no congestion
   response. A real `WeightToFee` curve plus a `FeeMultiplierUpdate` targeting
   block fullness is standard.

---

## 5. Node / packaging (non-pallet, but part of "live production ready")

- **Contracts/EVM RPC** endpoints in the node if §1 is adopted.
- **`substrate-frame-rpc-system`**, `pallet-transaction-payment-rpc` — the
  runtime APIs already exist; confirm the node exposes the RPCs.
- **Telemetry + Prometheus** (`substrate-telemetry`, `--prometheus`) for
  observability.
- **`try-runtime-cli`** — the `try-runtime` feature exists; the CLI exercises
  upgrade migrations against live state before shipping them.
- Real (non-`//Alice`) genesis: validator/allocation set, chain-spec presets, and
  a **deterministic Wasm build** in CI for on-chain upgrade verification.

---

## 6. Deliberately NOT to add

Because Prism owns ordering and finality client-side, these standard-chain
pallets **must not** be added — they would fight the "Prism decides order,
Substrate decides effect" seam:

- `pallet-session`, `pallet-staking` (+ `nomination-pools`,
  `election-provider-multi-phase`, `bags-list`)
- `pallet-authorship`, `pallet-aura` / `pallet-babe`, `pallet-grandpa`
- `pallet-im-online`, `pallet-offences`

And since Argonaut is a **standalone chain, not a parachain**, the entire
**XCM / Cumulus** stack is out of scope.

---

## 7. Suggested sequencing

Minimal production baseline, in priority order:

1. **Fee/weight hardening** (§4) — benchmarked weights + fee routing. Blocks any
   credible mainnet.
2. **Governance to retire sudo** (§2) — `scheduler`, `preimage`,
   `referenda`/`collective`, `treasury`.
3. **Account utilities** (§3) — `multisig`, `proxy`, `utility`, `identity`,
   `vesting`, `assets`.
4. **Smart contracts** (§1) — `pallet-contracts` + `ContractsApi` + node RPC for
   ink!.

Each tier is independently shippable and does not touch the Prism/Substrate seam.
