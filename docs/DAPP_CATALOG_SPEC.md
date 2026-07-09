# Argonaut dApp Catalog & Build Specs

**Status: committed catalog (2026-07-09; expanded to full design depth same
day), docs only.** This document answers two questions: *which decentralized
applications is Argonaut uniquely good at*, and *how each one is built* on
the primitives the other specs define. It assumes `IMPLEMENTATION_E_SPEC.md`
(production bootstrap) and `IMPLEMENTATION_D_SPEC.md` (storage platform) as
the platform contract; it adds **no runtime pallets** of its own. Apps live
above the chain: ink! contracts + off-chain daemons + SDKs.

Contract sketches below are design-level Rust (ink! 5.x idiom), not
compile-checked code: storage layouts, message check-orders, and constants
are committed; exact syntax is the implementer's. Where a check order is
listed, it is normative — same contract as the D spec's extrinsic
check-orders.

Naming note: "Implementation F" stays reserved for block rewards/tokenomics
(E spec §E.10); this catalog is deliberately outside the core letter scheme.

---

## 1. Why these apps: build to the differentiators

Argonaut has exactly three properties most chains do not. Every app in this
catalog is scored against them, and each app's section names which ones it
needs.

### PQ1 — Post-quantum signatures end to end

Every account is `blake3(falcon_pk)`; Falcon-512 signs everything. The
threat this defeats is the signature analog of harvest-now-decrypt-later:
**harvest-now-forge-later**. On an ECDSA/sr25519 chain, every transaction
ever published exposes a public key; a future quantum adversary derives the
private key and can then forge *new* statements from *old* identities —
retroactively "signing" documents, certificates, or asset transfers as any
historical party. Any application whose statements must remain
unforgeable for decades (notarization, PKI, custody, inheritance) is
structurally broken on classical chains and structurally sound here. This
is not a marketing gradient — it is a binary property, and P-4 (§3.4)
exists to keep apps from silently forfeiting it.

### PQ2 — Prism ordering: no reorgs, ever

The bridge imports every Substrate block `finalized: true`; the chain has
no fork-choice ambiguity at the Substrate layer. Two consequences apps
build on:

1. **UX:** confirmation is a single deterministic event. Point-of-sale and
   machine-to-machine flows never model "confirmed-but-revertible".
2. **Engineering (underrated):** every off-chain component in this catalog
   — daemons, monitors, indexers — processes chain events with a **plain
   monotone block cursor and zero rollback logic**. On a reorging chain,
   event-driven daemons need invalidation, depth thresholds, and
   compensating actions; here that entire class of code and bugs does not
   exist. §3.1 turns this into a reusable pattern.

One property PQ2 does *not* give: regular cadence. Substrate blocks are
produced one per confirmed Prism leader level, whose arrival times vary
with mining. **Design rule for all apps: durations and deadlines are
expressed in `pallet_timestamp` time, never block counts** (§3.6).

Throughput: Prism's evaluation (Bagaria et al.) demonstrates ~70k tx/s
class ordering; Argonaut devnet numbers are TBD and must never be quoted
until measured (house rule). Apps here are designed to be *correct* at any
throughput and merely *nicer* at high throughput.

### PQ3 — Native verifiable storage with paid retrieval

Implementation D gives apps five primitives no general-purpose chain
exposes natively:

| Primitive | What an app gets |
|---|---|
| Content addressing | a 32-byte BLAKE3 root is simultaneously the on-chain commitment, the transfer-verification id, and the proof target — one hash tree everywhere (guide §9.1.3) |
| Storage deals | erasure-coded availability with on-chain challenge proofs and escrowed payment |
| Retrieval gating | a provider serves a request **only** against a fresh payment voucher, enforced per request at the iroh provider-events layer |
| Payment channels | monotone-cumulative Falcon-signed vouchers (`DS_VOUCHER` domain), no challenge game, `settle` with best voucher |
| bao slice proofs | prove any byte range belongs to a committed root, ~2.5 KB per slice at 1024-byte granularity — *verifiable sampling* |

A subtle but load-bearing consequence of BLAKE3 verified streaming used
throughout §5.1: **a provider physically cannot serve bytes that differ
from the committed root** — corrupted data doesn't arrive as "wrong data",
it arrives as a failed transfer. Integrity failures manifest as
*unavailability*, which the D proof system already prices and punishes.
Apps therefore split "is the data intact" (free, automatic, cryptographic)
from "is the data *good*" (human/economic, needs an inspection window).

### Scoring rule

**Generic DeFi is portable and therefore undifferentiated** — an AMM runs
identically on fifty chains and inherits none of PQ1–PQ3. It appears here
only as Tier-3 plumbing (liquidity for ARG), never as a flagship.
Flagships must need PQ1/PQ2/PQ3 to exist at all.

## 2. Platform prerequisites matrix

| App (tier) | Needs from E | Needs from D | On-chain | Off-chain |
|---|---|---|---|---|
| Anchor (T1) | E0–E1, E4 | optional | 1 contract (immutable) | CLI only |
| Keystone (T1) | E0–E1, E4 | D1 (blobs) | 1 contract (timelock-upgradable) | monitor daemon (optional at MVP) |
| Press (T1) | E0–E1, E4, E6 | **D1–D5** | 1 contract | publisher daemon + reader SDK |
| Bazaar (T2) | E0–E1, E4, E6 | D1–D5 + slices | 1 contract | seller daemon + buyer SDK |
| Vault (T2) | E0–E3, E4 | none | factory + immutable instances | none (CLI) |
| Relic (T2) | E4, E6 | D1–D5 | 1 contract | keeper daemon (permissionless) |
| Meter (T2) | E1, E6 | payment-channels pallet | none (v1) | gate proxy + client SDK |
| Pool (T3) | E4 (+assets) | none | ported AMM | none |
| Endowment (T3) | E2, E4 | D1–D5 | 1 contract | keeper (shared w/ Relic) |

Two platform constants every app inherits:

1. **Contract accounts cannot sign** (E spec rule 3). A contract can hold
   funds, enforce policy, and emit events — it can never initiate an iroh
   transfer or sign a retrieval voucher. Therefore **every storage-touching
   app is a control-plane contract + an off-chain agent** that does the
   data-plane work. This mirrors the chain's own design and is §3.1.
2. **File bytes never go on-chain.** Content lives in D deals; chain state
   holds roots, deals, and money.

## 3. Common build patterns (normative; referenced as P-1…P-7)

### 3.1 P-1 — Control/data split, and the daemon skeleton

The ink! contract owns state and escrow; a daemon (or user-side SDK) owns
all data-plane work. Neither trusts the other beyond the chain: the
contract never blocks on data arriving; the daemon never custodies user
funds.

Every daemon in this catalog is the same skeleton, extracted once into
`apps/common/event-cursor`:

```
loop:
  events = fetch_contract_events(from = cursor + 1, to = best_block)
  for e in events: handle(e)            # handlers MUST be idempotent
  persist(cursor = best_block)          # fsync before advancing
```

- **At-least-once + idempotent handlers.** Crash between `handle` and
  `persist` replays events; every handler must tolerate replay (check
  on-chain state before acting, use deterministic ids for side effects).
- **No rollback path exists** (PQ2). The cursor only advances. This is the
  single biggest simplification Argonaut hands to app developers; the
  skeleton deliberately has no invalidation hook so nobody reintroduces
  one.
- Daemons authenticate to the chain with their own Falcon key
  (registry-mode after first tx) and are stateless-restartable from
  `(cursor, chain state)` alone — any local cache must be reconstructible.

### 3.2 P-2 — Voucher metering (reuse, don't reinvent)

D's payment channels are the *only* micropayment mechanism in the catalog.
Recap of the contract they provide: channel = on-chain deposit;
voucher = Falcon-signed `(channel_id, cumulative_amount)` over a 32-byte
blake3 payload (`DS_VOUCHER` domain); cumulative and monotone, so the
recipient keeps only the freshest voucher and `settle(best)` at any time —
no challenge game, no timeout race.

To meter any unit (bytes, API calls, seconds):
- define a **stride** (the billing quantum) and a price per stride;
- client sends a fresh voucher raising `cumulative` by
  `price_per_stride` before (or concurrently with) each stride;
- server policy: refuse the next stride if the freshest voucher doesn't
  cover all strides served so far — **serve-then-verify at stride
  granularity**, so the maximum unpaid exposure is exactly one stride.
- **Settlement policy** (committed default): settle when unsettled value
  > 5% of channel deposit *or* 24 h since last settle, whichever first;
  always settle best-voucher on daemon shutdown.
- **Concurrency rule:** one channel = one signer = one monotone counter.
  Parallel requests against one channel must serialize voucher issuance
  behind a single writer (client-side mutex); for true parallelism open
  multiple channels. Gate implementations must not accept out-of-order
  cumulative values as errors — stale vouchers are simply ignored.

### 3.3 P-3 — Verifiable sampling (the math)

A buyer verifying a dataset against its committed root samples N random
1024-byte chunks with slice proofs. If a fraction `f` of chunks would fail
(be unservable — see §1/PQ3: corrupt chunks cannot be served *at all*),
the probability at least one sample hits a bad chunk is:

```
P(detect) = 1 − (1 − f)^N        ⇒        N ≥ ln(1 − P) / ln(1 − f)
```

| f (bad fraction) | P = 95% | P = 99% |
|---|---|---|
| 10% | 29 | 44 |
| 1% | 299 | 459 |
| 0.1% | 2 995 | 4 603 |

Cost: each sample ≈ 2.5 KB proof + 1 KiB data ⇒ **99% assurance against
1% corruption costs ~1.6 MB of transfer**. Sampling is cheap; make N a
buyer-side knob with default `f=1%, P=99%`.

Index selection: buyer-chosen uniform randomness is committed as the v1
default — the buyer has no incentive to grind against themselves, and it
removes the beacon dependency. Beacon-derived indices (D-spec epoch
pattern) are reserved for adversarial-buyer contexts (e.g., third-party
audits where the *sampler* might collude with the seller).

Scope honesty (normative wording for all app docs): sampling proves
**integrity** (bytes match the root) with the stated probability. It
proves nothing about **semantic quality** (the committed bytes could be
well-formed garbage). Semantic protection = inspection of sample *content*
during an escrow window (§5.1), never a cryptographic claim.

### 3.4 P-4 — The PQ boundary

An app's *trust path* is every input that can cause value movement or
status change: signatures it accepts, oracles it reads, attestations it
imports, bridged assets it holds. **Rule: nothing classical
(ECDSA/EdDSA/BLS12-381-signed) may appear anywhere in the trust path**, or
the app forfeits PQ1 entirely — a chain of PQ signatures anchored on a
classical oracle is classical. Each app section below states its PQ
boundary in one line. Display-layer inputs (web PKI for a docs site,
GitHub READMEs) are outside the trust path and unconstrained.

### 3.5 P-5 — Repo layout, toolchain, testing ladder

```
apps/
  common/
    app-sdk/          # chain-client (D.8) + argonaut-signer (E6) + argonaut-store (D.10) glue
    event-cursor/     # §3.1 skeleton
    meter-gate/       # §5.4 reusable voucher-checking reverse proxy
  <name>/
    contracts/        # ink! (own workspace — cargo-contract profile)
    daemon/           # optional; binary crate on app-sdk
    docs/demo.md      # devnet demo script (house rule: live verification)
```

`apps/` is **excluded from the root cargo workspace**. Pin one ink!
5.x version and one `cargo-contract` version across all apps; upgrade at
catalog-wide checkpoints only (same discipline as the polkadot-sdk pin).

Testing ladder, per app: (1) ink! unit tests (mocked env); (2) e2e tests
against a dev node with `dev-signatures` where signer plumbing allows,
Falcon otherwise; (3) the devnet demo from `docs/demo.md` — MVP acceptance
is level 3, executed, not just written.

### 3.6 P-6 — Time semantics

Block production follows Prism leader levels: **roughly steady, not
fixed-interval**. Committed rule: every duration a user experiences
(validity periods, delays, heartbeats, escrow windows, grace periods) is
denominated in **milliseconds of `pallet_timestamp` time** and compared
against `self.env().block_timestamp()`. Block numbers are used only as
event cursors (§3.1) and for "which came first" ordering. No app constant
below is expressed in blocks.

### 3.7 P-7 — Contract upgradeability policy (per-app, committed)

Upgradeable contracts are a custody risk (whoever can upgrade can steal);
immutable contracts can't fix bugs. The catalog commits per-app instead of
one blanket rule:

| Contract | Policy | Rationale |
|---|---|---|
| Anchor | **immutable** (no `set_code_hash` path compiled in) | the product *is* "semantics can never change under your feet" |
| Keystone | upgradable via 3-of-5 multisig + **7-day announced timelock** (announce event carries new code hash; monitors alarm on it) | PKI needs a bug-fix path; the timelock makes upgrades themselves transparency-logged |
| Press, Bazaar, Relic, Endowment | same timelock pattern, 7 days | hold transient escrow, not long-term custody |
| Vault | **factory pattern**: factory upgradable (affects *future* deployments only); each deployed vault instance immutable | decade-horizon custody must not have an admin key, period |
| Meter | n/a (no contract in v1) | — |

Deposits/anti-spam inside contracts mirror E0's shape: contract-level
`deposit(items, bytes)` with the same coefficients, so spam pricing is
uniform chain-wide. Anchor is the one deliberate exception (§4.1.3).

---

## 4. Tier 1 — build first (full design depth)

### 4.1 Anchor — PQ notarization & timestamping

**What:** hash-anchoring with post-quantum proof of existence: "this exact
content existed at this time, attested by these identities, verifiable in
30 years." Targets: contracts/agreements, research priority claims,
compliance records, software artifacts.

**Why Argonaut:** PQ1 is the entire product (§1: harvest-now-forge-later is
precisely a forged backdated notarization). Cheapest possible flagship:
proves the E4 stack works with Falcon accounts, exercises E1 fees, zero
storage dependency at launch. **Why a contract and not `system.remark`:**
remarks are not queryable, not deduplicated, carry no attestation
semantics, and have no fee/deposit policy; why not a pallet: no consensus
logic needed, permissionless deployment is the point, and Anchor is the E4
acceptance harness.

#### 4.1.1 Contract design

```rust
#[ink(storage)]
pub struct Anchor {
    anchors: Mapping<[u8; 32], AnchorRecord>,        // root → record
    attests: Mapping<([u8; 32], AccountId), Attest>, // co-signatures
    anchor_fee: Balance,                             // set at instantiation, immutable
    fee_beneficiary: AccountId,                      // set at instantiation, immutable;
                                                     // fees forward here directly — the
                                                     // contract itself never accumulates a
                                                     // balance (invariant I-A3, §9.3)
}

pub struct AnchorRecord {
    owner: AccountId,        // first writer; informational, NOT transferable
    timestamp: Timestamp,    // env().block_timestamp() at anchor
    block: BlockNumber,      // ordering/cursor use only (P-6)
    meta: Vec<u8>,           // ≤ 256 bytes, opaque (suggested: SCALE {schema_id, deal_id?, label})
    attest_count: u32,
}
pub struct Attest { timestamp: Timestamp, note_hash: Option<[u8; 32]> }
```

Messages (check orders normative):

- `anchor(root: [u8;32], meta: Vec<u8>)` payable —
  (1) `meta.len() <= 256`; (2) `!anchors.contains(root)` (first-writer-wins,
  else `AlreadyAnchored`); (3) `transferred_value() >= anchor_fee`;
  (4) insert; (5) forward `transferred_value()` to `fee_beneficiary`;
  (6) emit `Anchored { root, owner, timestamp }`.
- `attest(root, note_hash: Option<[u8;32]>)` payable —
  (1) record exists; (2) caller hasn't attested this root;
  (3) fee as above; (4) insert + bump `attest_count`; (5) emit
  `Attested { root, who }`. An attestation means exactly: *this account,
  at this time, vouched for this root* — the optional `note_hash` can
  itself be anchored to say why.
- Queries: `get(root)`, `attestation(root, who)`.

There is deliberately **no** `transfer_owner`, **no** `revoke`, **no**
`remove`: anchors are *facts*, not assets. Facts don't change hands and
can't be unhappened.

#### 4.1.2 Committed constants

| Constant | v1 value | Rationale |
|---|---|---|
| `anchor_fee` | `deposit(1, 300)` (E0 fn) ≈ 1 record + max meta | prices permanent storage honestly |
| max `meta` | 256 B | enough for `{schema, deal_id, label}`; bytes belong in D |

#### 4.1.3 Economics & failure modes

The fee is **non-refundable by design** — a refund requires deletion and
deletion breaks the product; document this loudly (it inverts the
refundable-deposit norm from E). Griefing: front-running an `anchor` to
steal first-writer status gains nothing (the front-runner proves only that
*they* saw the hash — the true author proves authorship by *revealing
pre-image content or signing an `attest`*; the docs must teach the
commit-then-attest idiom for contested contexts). Timestamp accuracy:
block-level granularity; document the trust statement as "minute-scale
accuracy, inherited from the node's timestamp inherent" — never claim
second precision.

Optional storage escrow: caller may store the document via a D deal and
put the `DealId` in `meta`. Anchor verifies nothing about it — root
equality is the link; availability is D's department.

#### 4.1.4 Build plan & acceptance

`A-1` contract + unit tests → `A-2` `argonaut-anchor` CLI
(`anchor file.pdf` → blake3 → call; `verify file.pdf` → recompute, query,
print record + attesters) → `A-3` devnet demo: anchor, co-attest from a
second Falcon account, verify from a *second machine* using only the CLI
and a public RPC; negative test: re-anchor fails `AlreadyAnchored`.
**PQ boundary: pure — no external trust.**

### 4.2 Keystone — key & certificate transparency

**What:** a PQ-PKI layer: namespaces bind controller keys, issue and
revoke PQ certificates over subject keys, and every state change lands in
an append-only, publicly auditable event log. Targets: software-release
signing, organizational PKI, and service identity for the D provider
network itself. Defensive posture throughout: Keystone makes key
compromise *detectable and revocable*; it is not a licensing authority.

**Why Argonaut:** the chain already *is* a PQ key registry
(`pallet-falcon-registry`); Keystone lifts the primitive one level.
Certificate transparency on a no-reorg chain is categorically stronger
than web CT: web CT logs are Merkle trees audited by gossip and can
equivocate until caught; Keystone's log *is* consensus state — inclusion
is deterministic and rewriting is impossible (PQ2).

#### 4.2.1 Trust-anchor design decision (defuses squatting)

The trust anchor is the **namespace id** — `blake3(name ‖ creator ‖
creation_nonce)` — not the name string. Organizations publish their id
out-of-band (website, package metadata, D-stored claims), exactly like a
key fingerprint; the string is a display convenience. Consequence:
name-squatting yields a *different id* and therefore nothing — no dispute
mechanism, no arbitration committee, no rent-seeking surface. Committed:
flat registration deposit, no name governance in v1.

#### 4.2.2 Contract design

```rust
#[ink(storage)]
pub struct Keystone {
    namespaces: Mapping<NamespaceId, Namespace>,
    certs: Mapping<(NamespaceId, CertId), Cert>,       // CertId = per-ns u64 counter
    pending_ops: Mapping<NamespaceId, PendingOp>,      // §4.2.3 timelock
}

pub struct Namespace {
    name: Vec<u8>,               // ≤ 64 B display string
    controller: AccountId,       // Falcon account; upgrade path: E3 multisig
    created: Timestamp,
    cert_count: u64,
    frozen: bool,                // emergency stop: no new issuance (revocation still allowed)
}

pub struct Cert {
    subject_pk_hash: [u8; 32],   // blake3(falcon_pk) of the subject
    not_before: Timestamp,
    not_after: Timestamp,        // ≤ created + MAX_VALIDITY
    claims_root: Option<[u8; 32]>, // BLAKE3 root of SCALE claims blob in a D1 deal
    revoked: Option<(Timestamp, ReasonCode)>,
}
```

Messages:

- `register_namespace(name, nonce)` payable — deposit `deposit(1, 96)`;
  computes and returns the id; emits `NamespaceRegistered`.
- `issue(ns, subject_pk_hash, not_before, not_after, claims_root)` —
  (1) caller == controller; (2) `!frozen`; (3) `not_after − not_before ≤
  MAX_VALIDITY`; (4) store at `cert_count`, increment; (5) emit
  `CertIssued { ns, cert_id, subject_pk_hash, ... }` — **the event is the
  transparency log**.
- `revoke(ns, cert_id, reason)` — controller; idempotent-safe (revoking a
  revoked cert is a no-op, not an error — replay-friendly per §3.1); emits
  `CertRevoked`. **Revocation works even when frozen** (freeze must never
  block damage control).
- `freeze(ns)` — controller, immediate, emits `Frozen`. Unfreeze goes
  through the timelock (a compromised controller must not quietly resume
  issuance).
- `rotate_controller(ns, new_controller)` / `unfreeze(ns)` — **two-phase
  with `ROTATE_DELAY` timelock** (§4.2.3).

#### 4.2.3 The announce-then-execute timelock (the CT teeth)

`rotate_controller` and `unfreeze` are two-phase: `announce_op(ns, op)`
stores `PendingOp { op, execute_after: now + ROTATE_DELAY }` and emits
`OpAnnounced`; `execute_op(ns)` applies it after the delay;
`cancel_op(ns)` by the *current* controller aborts it. Why: the attack
that kills naive on-chain PKI is silent controller-key compromise. With
the timelock, a stolen controller key cannot take over a namespace without
a **7-day public announcement** that every monitor alarms on — the
legitimate owner freezes and cancels. The same pattern guards the
contract's own upgrades (P-7), so "watch Keystone's announce events" is
one monitoring rule covering both layers.

#### 4.2.4 Verifier & monitor

Verifier lib `keystone-verify` — `verify(ns, cert_id, subject_pk, now)`:
cert exists; `blake3(subject_pk) == subject_pk_hash`;
`not_before ≤ now < not_after`; `revoked.is_none()`; namespace exists.
Optionally: fetch claims blob from D by `claims_root`, bao-verify, decode.
Certificate lifecycle: `Issued → Expired | Revoked` (terminal both).

Monitor daemon `keystone-monitor` (event-cursor skeleton, §3.1): watch
lists of namespace ids; alarm on any `CertIssued` not in the operator's
expected set, any `OpAnnounced`, any upgrade announcement. This is the CT
"monitor" role; running one is how a namespace owner detects compromise
within minutes instead of at expiry.

#### 4.2.5 Constants & acceptance

| Constant | v1 value |
|---|---|
| `MAX_VALIDITY` | 2 years (ms) |
| `ROTATE_DELAY` | 7 days (ms) |
| namespace deposit | `deposit(1, 96)` |
| cert deposit | `deposit(1, 128)` per issuance |

Build: `K-1` contract + verifier lib → `K-2` monitor daemon → `K-3` devnet
demo: register (id published out-of-band) → issue → verify → revoke →
verify-fails; rotate with timelock observed by the monitor; **adversarial
leg:** "stolen" controller announces rotation, monitor alarms, owner
freezes + cancels inside the window. **PQ boundary: pure on-chain; claims
blobs verified by root.**

### 4.3 Press — paid content distribution (the D-stack flagship)

**What:** publish once, get paid per read: articles, video, datasets-as-
media, model weights. Readers pay through payment channels; providers
serve only against fresh vouchers; no subscription intermediary.

**Why Argonaut:** this is the app the retrieval gate was designed for
(guide §9.0). PQ3 end to end, PQ2 for instant-final payment UX.

#### 4.3.1 The two-leg revenue design (and why not revenue-split)

A read has two payees and two legs:

- **License leg (on-chain, contract):** reader pays the *publisher* for
  the right to the work — `buy(work_id)` transfers `price_license` to the
  publisher and records a `License`.
- **Bytes leg (off-chain, D verbatim):** reader pays the *provider* for
  delivery — the standard D retrieval flow: open channel, stream vouchers
  per stride, provider settles on-chain.

Rejected alternative, recorded so nobody re-derives it: routing provider
retrieval revenue *through* the contract for a publisher/provider split.
Providers are already compensated twice by D's own economics (deal escrow
for storage, retrieval vouchers for delivery); inserting the contract into
the bytes leg would re-price D's market inside an app, couple the contract
to channel mechanics, and add an escrow hop to every read. The two legs
stay independent: publishers compete on license price, providers on
delivery price. Publisher margin = license revenue − storage deal spend.

#### 4.3.2 Contract design

```rust
#[ink(storage)]
pub struct Press {
    works: Mapping<WorkId, Work>,             // WorkId = u64 counter
    licenses: Mapping<(WorkId, AccountId), License>,
    publisher_index: Mapping<AccountId, Vec<WorkId>>, // bounded page-index
}

pub struct Work {
    publisher: AccountId,
    manifest_root: [u8; 32],       // D manifest root == content identity
    preview_root: Option<[u8; 32]>,// small separate blob, served ungated
    size_bytes: u64,
    price_license: Balance,        // may be 0 (free work, provenance only)
    license_hash: [u8; 32],        // blake3 of human-readable license text (anchor it via Anchor!)
    deals_hint: Vec<DealId>,       // informational; truth lives in D pallets
    status: WorkStatus,            // Listed | Delisted (delist stops sales, never revokes licenses)
}
pub struct License { bought_at: Timestamp, price_paid: Balance }
```

Messages: `publish(...)` (deposit `deposit(1, 256)`; emits `Published`);
`buy(work_id)` payable — (1) work `Listed`; (2) no existing license
(rebuy = no-op refund-free error); (3) `transferred_value() ==
price_license`; (4) transfer to publisher; (5) record license, emit
`Bought`. `set_price`, `delist` (publisher only; licenses survive
delisting). `has_license(work, who)` — the query providers *may* consult
(v2 gate integration; v1 doesn't need it, see below).

**v1 access model (committed): license = receipt, not gate.** The bytes
leg is already paid per-stride, so "unlicensed" readers who fetch anyway
have merely… bought the bytes without the license — economically
equivalent to piracy of a receipt. v1 accepts this; the license's value is
provenance + supporting the publisher + lawful access. v2 (optional
per-publisher) has providers check `has_license` before opening gated
works' streams — one extra RPC in the gate path, already supported by D's
per-NodeId intercept.

#### 4.3.3 Flows

```
PUBLISH (publisher daemon, P-1):
  encode (argonaut-store, RS shards)──► deal create ──► DealActive
        └► publish(manifest_root, …) ──► Published event

READ (reader SDK):
  buy(work_id)                      # license leg — one extrinsic, final on inclusion (PQ2)
  discover providers (D registry)   # via deals_hint or provider registry scan
  open/reuse payment channel        # D payment-channels pallet
  fetch manifest → shards           # vouchers per stride (§3.2); bao-verified streaming
  reassemble + final verify against manifest_root
```

Worked example (documents the fee shape, E1 units): 1 GiB work, stride
16 MiB = 64 strides; provider price 5 MILLICENTS/MiB ⇒ bytes leg
5 120 MILLICENTS ≈ 5.1 CENTS; license 50 CENTS ⇒ reader total ≈ 55 CENTS,
of which publisher gets 50, provider ~5.1, chain fees per E1's worked
examples. (Illustrative prices; markets set real ones.)

#### 4.3.4 Failure modes

| Failure | Behavior | Design position |
|---|---|---|
| Publisher stops renewing deals | work unavailable; reader SDK surfaces D deal state before `buy` (client-side check — no oracle needed, it's all on-chain) | licenses are *not* refunded: a license is a perpetual right to fetch **while available**; the SDK must show availability prominently pre-purchase |
| Provider withholds despite vouchers | reader's exposure ≤ 1 stride (§3.2); switch provider (RS shards exist at n providers) | D's economics, not Press's |
| Reader re-shares bytes | possible; see honesty rule | Press sells **payments + provenance + availability — never DRM**; this sentence is normative for all Press marketing/docs |
| Corrupt shard | cannot be served at all (verified streaming) → manifests as unavailability | D challenge system slashes; reader retries other providers |

#### 4.3.5 Build plan & acceptance

`P-1` contract + unit tests → `P-2` publisher daemon (event-cursor +
argonaut-store) → `P-3` reader SDK/CLI (`press buy`, `press fetch`) →
`P-4` devnet demo on ≥2 storage providers: publish → buy → fetch with
voucher streaming → offline re-verify against root; provider `settle`s
vouchers on-chain; negative legs: fetch a delisted work with an old
license (succeeds), buy a delisted work (fails), kill one provider
mid-fetch and finish from the other (RS recovery visible).
**PQ boundary: pure.**

---

## 5. Tier 2 — build on Tier-1 rails

### 5.1 Bazaar — data exchange with verifiable sampling

**What:** a dataset marketplace where buyers verify before they buy:
sample slices against the committed root (P-3), inspect sample *content*,
then release escrow for bulk retrieval. Targets: training data, market
data, scientific datasets.

**Two-layer protection (from §1/PQ3's integrity-vs-quality split):**

1. **Integrity — cryptographic, automatic.** A seller whose stored bytes
   don't match the advertised root *cannot serve valid slices at all*;
   sampling detects this as failed/timed-out sample fetches with P-3
   probability. Detection = absence of valid data, not verification of bad
   data (a corrupted slice never verifies — it never *arrives*).
2. **Quality — economic, human/scripted.** Samples that arrive are genuine
   bytes of the committed dataset; whether they're *worth buying* is
   judged by inspecting them during the escrow window (schema checks, spot
   checks, statistics). Bazaar's docs must never conflate the two layers.

**Escrow state machine (contract):**

```
Listed ──buy(escrow locked)──► Sampling(deadline = now + SAMPLE_WINDOW)
   Sampling ──accept()────────► Released (funds → seller)
   Sampling ──abort()─────────► Refunded (minus ABORT_FEE → seller)
   Sampling ──deadline passes─► Refunded (same; buyer inaction = abort)
```

Committed decisions with rationale:
- **Timeout ⇒ refund**, not release: auto-release on silence would let
  sellers profit from buyer-side failures (lost keys, crashed tooling).
  The seller is compensated for a wasted window by `ABORT_FEE`
  (flat, small — e.g. `deposit(1, 0)`-scale) plus the fact that…
- **…samples are paid retrieval at normal stride prices** (P-2), so
  sample-leeching ("sample repeatedly, never buy") is self-financing for
  the seller at their own advertised rates. No free-rider hole, no
  reputation system needed in v1.
- No on-chain "fraud proof" of corruption exists or is needed:
  non-serving is not provable, but it *is* refundable (buyer aborts) and
  it *is* punished (the seller's D deals fail availability challenges
  independently). The escrow never has to adjudicate data quality.

Dependencies: Press rails (catalog+SDK patterns), D slice proofs.
Acceptance adds the two adversarial legs explicitly: (a) corrupted-shard
seller — samples fail to arrive, buyer aborts, refund lands; (b) honest
bytes, garbage content — samples arrive and verify, buyer inspects and
aborts within the window. **PQ boundary: pure.**

### 5.2 Vault — quantum-safe custody & inheritance

**What:** decade-horizon custody: m-of-n Falcon multisig ownership,
spending policy (limits, delays, allowlists), and dead-man-switch
inheritance.

**Why Argonaut:** the purest PQ1 product — "assets that stay yours for 30
years" is exactly the promise classical chains cannot make (§1). And
per P-7, each vault instance is **immutable**: long-horizon custody with
an upgrade key is custody by whoever holds the upgrade key.

**Design:**

```rust
pub struct VaultConfig {                  // fixed at instantiation (immutable instance)
    owners: AccountId,                    // an E3 multisig account (m-of-n happens there)
    daily_limit: Balance,                 // rolling 24h (timestamp-based, P-6)
    large_threshold: Balance,
    large_delay: Timestamp,               // e.g. 48h announce-before-execute
    allowlist: Option<Vec<AccountId>>,    // if Some, only these destinations
    heartbeat_period: Timestamp,          // e.g. 90 days
    grace: Timestamp,                     // e.g. 30 days
    heirs: Vec<(AccountId, Permill)>,     // shares sum ≤ 1; remainder → first heir
}
```

- **Spending:** `spend(dest, amount)` — under `daily_limit` and (if set)
  allowlisted ⇒ immediate; over `large_threshold` ⇒ enters a pending queue
  executable after `large_delay`, cancellable by owners during the window.
  The delay window is the theft response: a compromised multisig quorum
  still telegraphs large exfiltration 48 h in advance to any watching
  owner. (Same announce-then-execute shape as Keystone §4.2.3 — the
  catalog uses one idiom for "dangerous ops go through a public delay".)
- **Inheritance state machine:**
  `Active ──(no heartbeat for heartbeat_period)──► Lapsed(announced)
  ──(grace passes)──► Claimable ──heirs claim shares──► Drained`.
  Any owner heartbeat during `Lapsed` returns to `Active` (resurrection);
  heirs claim independently (no coordination required among heirs). All
  windows in timestamp ms (P-6) — block-count windows would drift with
  Prism cadence.
- **Factory:** deploys instances from a pinned code hash; factory upgrades
  affect future deployments only (P-7). No storage dependency anywhere.

Acceptance: devnet demo of 2-of-3 spend; policy-blocked overspend; pending
large-spend cancelled during delay; heartbeat lapse → heir claim (devnet
with shortened test constants); resurrection during Lapsed.
**PQ boundary: pure.**

### 5.3 Relic — content-backed NFTs

**What:** NFTs whose media provably exists and stays available: token ↔
`manifest_root` ↔ live D deal, with an on-chain `Backed/Unbacked` flag
maintained by permissionless keepers. Kills the dead-IPFS-link problem
honestly: content is either provably stored or the token *visibly* says it
isn't.

**Design highlights:**
- ink! NFT (PSP34-lineage) + per-token
  `{ manifest_root, deal: DealId, backing: Backed | Grace(since) | Unbacked }`.
- **Keeper economics (the interesting part):** `mark_unbacked(token)` is
  permissionless; the contract *checks the claim against D's deal state
  on-chain* (cross-contract/runtime read of the storage-deals pallet — no
  oracle, the truth is local) and pays the caller a small bounty from a
  pool funded by a slice of mint fees. False claims cost the caller the
  call fee and pay nothing — verification is on-chain, so keepers can't
  lie, only race each other. `Grace` (e.g. 14 days) between deal lapse and
  `Unbacked` gives owners a renewal window.
- **Re-backing is patronage:** *anyone* may fund a new deal for the same
  root and call `mark_backed(token, new_deal)` (contract re-verifies
  against D). A community can keep art alive without owning it.
- Royalties: enforced only inside this contract's own transfer/marketplace
  functions; same honesty rule as Press — economic, not cryptographic.

Acceptance: mint → fetch media by root → let deal expire (shortened devnet
deal) → keeper flips `Grace→Unbacked`, collects bounty → third party
re-funds → `Backed`. **PQ boundary: pure.**

### 5.4 Meter — machine-to-machine streaming payments

**What:** SDK-first productization of P-2 for non-storage metering:
pay-per-call APIs, per-second compute/GPU, bandwidth. No contract in v1 —
**the payment-channels pallet is the product**; Meter is packaging.

**Components:**
- `meter-gate` (in `apps/common/`, reused by Press v2 and Bazaar sample
  serving): a reverse proxy in front of any HTTP/gRPC service. Per
  request: map peer → channel (the `Hello { channel_id }` handshake,
  mirroring D's `/argonaut/retrieval/1`), check freshest voucher covers
  `strides_served + 1`, forward or 402. Implements the §3.2 settlement
  policy and the single-writer counter rule; exposes Prometheus metrics
  (strides served, unsettled balance, settle events) per E5 conventions.
- Client SDK: channel lifecycle (open, top-up when remaining < 2×
  expected burst, close), voucher signer (wraps `argonaut-signer`),
  automatic voucher-ahead pipelining so metering adds no request latency
  (issue voucher for stride N+1 while N is in flight — safe because
  vouchers are cumulative: over-issuing by one stride risks exactly one
  stride).
- v2 (optional): `meter-hub` contract for service discovery/reputation —
  only if organic usage demands it; discovery is not a launch requirement.

Acceptance: devnet demo metering a real HTTP API per-request through
channels, including a mid-stream `settle`, a channel top-up under load,
and a client that stops paying (gate returns 402 after ≤1 unpaid stride).
**PQ boundary: pure (vouchers are Falcon-signed by construction).**

---

## 6. Tier 3 — ecosystem plumbing (undifferentiated, still needed)

- **Pool — AMM/DEX (ink!).** Constant-product pool for ARG↔asset
  liquidity. Trigger: E3's optional `pallet-assets` gaining a product
  reason (stable-unit pricing for Press/Bazaar is the likely one). Port a
  mature audited ink! AMM; write nothing novel.
- **Endowment — archive public goods.** Treasury-seedable contract that
  auto-renews D deals for designated public-good archives: explicit
  endowment → explicit renewal horizon (the contract computes and
  publishes "funded through <date>" from its balance and current deal
  prices) — permaweb economics without perpetuity hand-waving. Reuses
  Relic's keeper infrastructure and the P-7 timelock. Depends on E2
  treasury + D.
- **Indexer profile.** Not an app: a documented SQL schema + decoder for
  the event streams of §4–5 (Anchor records, the Keystone log, Press
  catalog/licenses, Relic backing changes), built on the §3.1 cursor
  (no-reorg indexing is just an INSERT loop). Lands with E6's types-bundle
  work so explorers and monitors share one decoding.

## 7. Anti-catalog — deliberately not building

- **Oracle-fed DeFi** (lending, perps, oracle-stablecoins): no oracle
  story exists, and any classically-signed oracle breaks P-4 wholesale —
  a PQ chain reading an ECDSA price feed is classical at the exact point
  where the money moves. Revisit only with a PQ-signed oracle design.
- **Bridges / wrapped assets:** imports the counterpart chain's quantum
  exposure *and* its validator-set risk; structurally off-message before
  the security review even starts.
- **On-chain data apps** (social feeds, high-rate game state): violates
  "file bytes never go on-chain" (§2); if it doesn't fit the control/data
  split it doesn't fit Argonaut.
- **Randomness-critical apps** (lotteries, casino): chain randomness is
  dev-grade (E spec §E.7); *blocked, not deferred* until a beacon
  graduates (D's epoch beacon in production would reopen this).
- **DRM claims:** Press/Relic/Bazaar sell payments + provenance +
  availability; never confidentiality or copy-protection the design
  doesn't have. This sentence is normative for every app's public docs.

## 8. Sequencing & why this order

```
Anchor ──────────► proves E4 + Falcon-signed contract calls + E1 fees
Keystone ────────► proves contract↔D1 split, timelock idiom, monitor pattern
Press ───────────► proves the full D stack: deals, gate, vouchers, settle   ◄ flagship
Bazaar, Relic ───► reuse Press rails + slice proofs + keeper economics      (T2)
Vault ───────────► needs E3 only; parallel track any time after E3+E4       (T2)
Meter ───────────► needs payment-channels only; parallel track              (T2)
Pool, Endowment ─► product-triggered plumbing                               (T3)
```

Three idioms recur across the catalog and should be built once, in
`apps/common/`, at first use: the **event-cursor daemon** (§3.1, first
used by Keystone), the **announce-then-execute delay** (§4.2.3, reused by
Vault and every P-7 upgrade), and the **voucher gate** (§3.2/§5.4, reused
by Press v2 and Bazaar). Each Tier-1 app doubles as the *acceptance
harness* for a platform layer — Anchor for E4, Keystone for the
contract/storage split, Press for all of D. Build them in that order and
the platform gets verified as a side effect — which is the real reason
this catalog exists.

---

## 9. Production-readiness plan (MVP → mainnet)

Everything in §4–§5 ends at the MVP devnet demo. This section is the rest
of the road: a shared gate ladder every app climbs, the cross-cutting
production requirements built once, and per-app plans (invariants, audit
focus, monitoring, launch caps, residual risks). An app is "production
ready" when — and only when — it has passed **G3** below with its per-app
plan executed; nothing ships to mainnet on the strength of a demo.

### 9.1 The gate ladder (normative for every app)

- **G0 — MVP.** The §4/§5 acceptance: code-complete, unit + e2e tests,
  live devnet demo per `docs/demo.md` (house rule). Where §4–5 stop.
- **G1 — Testnet soak, ≥ 4 weeks.** Deployed on the public testnet (E0
  preset) with monitors and dashboards live from day one. Must include
  the **chaos drill set** (§9.2.5) and, for priced apps, real
  economic-parameter exercise: at least one parameter (fee, window,
  stride price) deliberately mis-set at soak start and corrected via the
  app's own governance/upgrade path — proving the tuning loop works, not
  just the happy path.
- **G2 — Security review.** (a) External audit of every contract
  touching value (scope table in §9.3); (b) internal adversarial review
  written against each app's failure-mode table — every row gets a "we
  attempted this" note; (c) property tests/fuzzing green on the §9.3
  invariants (ink! message fuzzing + property harness); (d) all
  audit findings remediated or explicitly risk-accepted in writing.
- **G3 — Guarded mainnet launch.** Deploy with **launch caps** (per-app
  values in §9.3), admin keys in ceremony custody (§9.2.2), runbook +
  on-call rotation live (§9.2.6), public status/monitoring page, and the
  published code hash verified on-chain by a third party (§9.2.1).
- **G4 — Caps lifted.** After ≥ 6 incident-free weeks at G3 and all G2
  remediations closed: raise/remove caps **through the app's own
  announce-then-execute path** (§4.2.3 idiom), so the cap change itself
  is telegraphed. G4 is per-cap, not all-or-nothing.

Regression rule: a P-7 timelocked upgrade re-enters the ladder at G2 for
the changed surface (delta audit), and at G1 if it changes economics.
Immutable contracts (Anchor, Vault instances) never re-enter — that is
the point of immutability; their fixes are *new deployments* with
explicit migration docs.

### 9.2 Cross-cutting production requirements (build once)

**9.2.1 Reproducible contract builds & code-hash transparency.** Every
contract release is built with the pinned `cargo-contract` in a
containerized deterministic build (the contract-layer analog of E5's
srtool job); the resulting **code hash is published** in the app's
`docs/` and announced via Anchor (the catalog's own notary — dogfooding).
Verification instruction for third parties: fetch on-chain code hash,
compare. No contract goes to G3 without an independent reproduction.

**9.2.2 Admin-key ceremony.** Every P-7 upgrade key and every
`fee_beneficiary`/treasury-ish account is an E3 multisig (≥ 2-of-3;
3-of-5 for Keystone) whose member keys come from the E0 key-ceremony
procedure — no developer laptops. Ceremony records (account ids, member
count, threshold — never key material) are checked into `apps/<name>/docs/`.

**9.2.3 The pause idiom — "pause entries, never exits" (PR-1).**
Timelock-upgradable contracts (Keystone, Press, Bazaar, Relic, Endowment)
gain a `paused` flag settable **immediately** by the admin multisig
(pausing is the one op that must not wait out a timelock) with these
committed semantics: pause blocks *entry* operations (new buys, new
issuance, new escrows, mints) and never blocks *exit* operations
(refunds, `abort`, revocation, license queries, heir claims, escrow
resolution). Unpause goes through the announce-then-execute delay
(a compromised admin must not quietly resume). Deliberate exceptions,
documented: **Anchor has no pause** (nothing to protect — it holds no
funds after the §4.1 fee-forwarding design) and **Vault instances have no
pause** (a pause switch on custody *is* an admin backdoor; the large-spend
delay window is Vault's whole emergency story).

**9.2.4 Observability baseline.** Every daemon and gate ships Prometheus
metrics per E5 conventions. Minimum set, all apps: event-cursor lag
(blocks behind best), handler error rate, restarts; plus the per-app
metrics in §9.3. Alert wiring is part of G1 entry, not an afterthought —
a monitor that isn't alarming during soak is untested.

**9.2.5 Chaos drill set (run during G1, scripted, repeatable).**
(1) kill/restart every daemon mid-event-batch — verify idempotent replay
(§3.1) leaves state correct; (2) kill a storage provider mid-fetch —
verify RS failover; (3) exhaust a payment channel mid-stream — verify
≤ 1-stride exposure and clean top-up; (4) full timelock rehearsal:
announce → monitor alarm fires → cancel; announce → execute; (5) upgrade
rehearsal end-to-end on testnet including code-hash reproduction;
(6) clock-skew tolerance: daemons behind a node whose timestamps drift
minutes — no window logic breaks (P-6 windows are generous by design).

**9.2.6 Runbooks & support.** Per app, a one-page runbook: alert →
diagnosis → action, including the pause decision tree, the "settle all
vouchers now" procedure, and contact/escalation. Plus user-facing docs
debt paid before G3: the Anchor commit-then-attest idiom, the Keystone
id-out-of-band pattern, Press's availability-before-buy check — each of
these is a *safety behavior* users must know, so docs are a launch gate,
not marketing.

**9.2.7 Dependency & constants discipline.** ink!/`cargo-contract` pins
upgrade only at catalog-wide checkpoints (§3.5) and force re-entry at G2
for rebuilt contracts. Test deployments may shorten time windows **only
via instantiation parameters**, never code forks; every non-production
constant used in a demo/soak is logged in `demo.md`. Production constants
are the §4–5 tables — any change is a spec change first.

### 9.3 Per-app production plans

Notation: **I-x** invariants are property-test/fuzz targets (G2c);
**caps** are G3 launch limits enforced in-contract (removable per G4).

#### Anchor
- **Invariants:** I-A1 records are write-once (no message mutates an
  existing `AnchorRecord`); I-A2 `attest_count` == number of `attests`
  entries for that root; I-A3 contract balance is always 0 after any
  message (fees forward, nothing accumulates — no funds to steal is the
  audit headline).
- **Audit focus:** small — first-writer race semantics, fee forwarding,
  meta bounds. Cheapest possible external audit; do it anyway (it is also
  the auditors' warm-up on ink!-on-Argonaut).
- **Monitoring:** anchors/day, attest/day, fee-forward failures (must be
  zero). No daemon → no cursor metrics.
- **Caps:** none needed (holds no funds). G3 = G0+G1+G2 complete.
- **Residual risk:** users misunderstanding first-writer semantics →
  mitigated by docs gate (§9.2.6), not code.

#### Keystone
- **Invariants:** I-K1 `cert_count` strictly monotone per namespace;
  I-K2 `revoked` is one-way (never un-set); I-K3 no `CertIssued` while
  `frozen`; I-K4 no pending op executes before `execute_after`; I-K5
  `revoke` succeeds even when frozen or paused (PR-1 exit rule).
- **Audit focus:** the timelock state machine (announce/execute/cancel
  races), freeze/unfreeze asymmetry, controller-rotation edge cases.
- **Monitoring:** `OpAnnounced` (page immediately — every announcement is
  either expected or an incident), issuance rate per watched namespace,
  monitor-daemon cursor lag.
- **Caps:** max namespaces per account (anti-spam, e.g. 16); max
  issuance rate per namespace per day (e.g. 1 000) — both instantiation
  params.
- **Residual risk:** controller key loss (≠ compromise) with no rotation
  announced in time → namespace orphaned; positioned honestly as key
  hygiene, mitigated by multisig controllers.

#### Press
- **Invariants:** I-P1 conservation: contract balance == sum of listing
  deposits (license payments transfer straight through, the contract
  never holds reader funds); I-P2 a `License` exists ⇔ a `Bought` event
  with matching payment exists; I-P3 delist never removes a license;
  I-P4 `price_license` changes never affect already-bought licenses.
- **Audit focus:** payable-message value handling (exact-value check,
  refund on error paths), publisher-index bounds, delist semantics.
- **Monitoring (contract + daemons):** buys/day, license revenue,
  publisher-daemon cursor lag, deal-state check failures in the reader
  SDK (spike = publishers letting deals lapse), provider voucher-settle
  events.
- **Caps:** `price_license` ≤ 100 ARG at launch; total works per
  publisher ≤ 100; (bytes-leg exposure is already structurally capped at
  one stride by §3.2 — state this in the audit brief).
- **Residual risks:** re-sharing (accepted, normative honesty rule §7);
  publisher deal-lapse UX (mitigated: SDK availability check is a
  *blocking* pre-buy step at G3, relaxable at G4).

#### Bazaar
- **Invariants:** I-B1 escrow conservation: contract balance == listing
  deposits + Σ(escrows in `Sampling`); I-B2 every escrow terminates in
  exactly one of {Released, Refunded}; I-B3 no release before `accept()`
  and no double-release; I-B4 timeout always refunds (never releases);
  I-B5 `abort`/refund works while paused (PR-1).
- **Audit focus:** the escrow state machine under reentrancy and
  cross-contract-call failure (the transfer-out on Released/Refunded is
  the classic target); timeout/accept race at the deadline boundary.
- **Monitoring:** escrows opened/resolved, abort rate (quality signal),
  sample-fetch failure rate (integrity signal — a spike means a seller is
  serving corrupt listings), mean sample N chosen by buyers.
- **Caps:** max escrow 500 ARG; max concurrent escrows per seller 10.
- **Residual risk:** semantic-garbage sellers churning listings →
  abort-rate metric feeds a v2 reputation decision; not solved in v1,
  said out loud.

#### Vault (highest bar in the catalog)
- **Invariants:** I-V1 conservation: instance balance ≥ Σ(pending spends)
  at all times; I-V2 no spend executes that violates
  `daily_limit`/allowlist/delay at *execution* time (not just announce
  time); I-V3 heir shares sum ≤ 1 enforced at instantiation; I-V4 heirs
  can never claim while `Active`; I-V5 any owner heartbeat during
  `Lapsed` restores `Active` and voids claims; I-V6 no admin/upgrade path
  exists in a deployed instance (verified by code inspection — it's the
  product).
- **Audit focus:** the full state machine (`Active/Lapsed/Claimable`)
  under timestamp adversariality; rolling 24 h limit accounting; the
  factory (correct code-hash deployment, param validation). **Vault
  additionally gets a second independent audit** — it is the one app
  whose failure is unrecoverable by design (no upgrade path), so the bar
  is two clean external reviews plus the property suite, and G1 soak runs
  with *production* time constants at least once (90-day heartbeat means
  G1 for Vault is ≥ one full heartbeat period on testnet — plan the
  calendar accordingly).
- **Monitoring:** none server-side (no daemon) — instead a *user-side*
  watch tool ships with the CLI: alerts owners on any `PendingSpend` or
  `Lapsed` transition for their vaults. Shipping this watcher is a G3
  gate: the delay window is only protective if someone is watching it.
- **Caps:** factory enforces per-instance TVL cap 10 000 ARG and global
  factory TVL cap 100 000 ARG at G3; both raised at G4 through factory
  redeployment (instances are immutable, the *factory* gates new size).
- **Residual risks:** all-owners key loss (assets locked until heartbeat
  lapse → heirs — which is the designed recovery); user-chosen bad
  parameters (mitigation: CLI refuses obviously dangerous configs, e.g.
  grace < 7 days, unless `--i-understand` flag).

#### Relic
- **Invariants:** I-R1 `backing` transitions only along
  `Backed ↔ Grace → Unbacked → Backed`; I-R2 every `mark_*` call's claim
  matches D deal state read in the same call (no stale flips); I-R3
  bounty pool never overdrawn; I-R4 token ownership never affected by
  backing state.
- **Audit focus:** the cross-contract/runtime read of storage-deals state
  (the one place an app reads a pallet — get the interface reviewed by
  whoever owns D), keeper bounty race (two keepers, one bounty —
  first-in-block wins, second is a cheap no-op).
- **Monitoring:** keeper daemon cursor lag, `Grace` population (early
  warning of mass deal lapse), bounty pool balance.
- **Caps:** mint fee floor (funds the bounty pool), max supply per
  contract instance at launch (e.g. 10 000).
- **Residual risk:** nobody runs keepers → mitigated by bounty + the
  Endowment keeper (§6) as backstop operator at G3.

#### Meter
- No contract in v1 ⇒ no contract audit; production work is **gate
  hardening**: (a) property test — unpaid exposure never exceeds one
  stride under concurrent load, malformed vouchers, and replayed stale
  vouchers; (b) DoS posture: Falcon voucher verification is cheap
  (tens of µs) but not free — per-IP/per-NodeId rate limits *before*
  signature checks, connection caps, and a `Hello`-flood test in the G1
  drills; (c) settlement-policy fault injection (chain RPC down at
  settle time → gate keeps serving within exposure bounds, retries,
  alarms).
- **Monitoring:** strides served, unsettled balance per channel
  (alert at 2× settle threshold), 402 rate, settle failures.
- **Caps:** operator-side config defaults: max channel deposit accepted
  10 ARG, max concurrent channels per NodeId 4.
- **Residual risk:** operator misconfigure (serve-before-verify beyond
  one stride) → ship the gate with safe defaults and a config linter,
  and the property test in CI runs against the *shipped defaults*.

### 9.4 Readiness summary

| App | Gate pacing note | The hard part | External audit |
|---|---|---|---|
| Anchor | fastest through ladder; no funds at rest | docs (semantics) | 1 (small) |
| Keystone | monitor coverage is the launch gate | timelock races | 1 |
| Press | G1 needs ≥ 2 independent testnet providers | value-handling paths | 1 |
| Bazaar | after Press G3 | escrow reentrancy | 1 (+ shared findings w/ Press) |
| Vault | **G1 ≥ one full heartbeat period** | irrecoverability by design | **2 independent** |
| Relic | after D stable on mainnet | pallet-read interface | 1 |
| Meter | gate hardening, not contracts | DoS + exposure bound | security review, not audit |

Order of first mainnet arrivals, implied: Anchor → Keystone → Meter →
Press → (Bazaar, Relic, Vault as their gates clear). Vault starts its
long G1 clock early and lands late — start the testnet soak the moment E3
ships rather than waiting for the storage apps.

