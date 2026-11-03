# Argonaut Architecture

This document explains **how Argonaut actually works, as built** — not just what was planned. It
covers the two subsystems that exist and run together today: post-quantum signatures with an
on-chain key registry, and the Prism consensus engine that orders every transaction before
Substrate ever executes one. Argonaut is one machine; these are its two working parts, not
separate products. For the original design intent (and the parts still to come), see
[`ARGONAUT_ARCHITECTURE_GUIDE.md`](./ARGONAUT_ARCHITECTURE_GUIDE.md).

Both subsystems are functionally complete, unit-tested, and — as of 2026-07-07 — live-verified on
a real two-node devnet: byte-identical chains across multiple confirmed levels, a node restart,
and DAG backfill, all in one session (see §4.9 for a real bug that session found and fixed along
the way).

## Table of contents

1. [Design philosophy: the one binding rule](#1-design-philosophy-the-one-binding-rule)
2. [Crate map](#2-crate-map)
3. [Signatures & accounts: Falcon-512 + registry](#3-signatures--accounts-falcon-512--registry)
4. [Consensus: Prism](#4-consensus-prism)
5. [End-to-end transaction lifecycle](#5-end-to-end-transaction-lifecycle)
6. [Known limitations and deliberate compromises](#6-known-limitations-and-deliberate-compromises)
7. [Where to go next](#7-where-to-go-next)

---

## 1. Design philosophy: the one binding rule

Argonaut's ordering layer and execution layer meet at exactly one seam, and the entire
architecture is designed to keep that seam thin and one-directional:

```
   ┌─────────────────────────────┐              ┌─────────────────────────────┐
   │        Consensus              │              │        Execution              │
   │        Prism                  │              │   Falcon-512 + registry       │
   │                               │              │                               │
   │   decides transaction ORDER  │              │  decides transaction EFFECT   │
   │   (client-side, no Substrate │──confirmed──▶│  (Substrate runtime, FRAME,   │
   │    block production at all)  │   leader      │   pallet-balances, etc.)      │
   │                               │   sequence    │                               │
   └─────────────────────────────┘              └─────────────────────────────┘
```

**Prism decides order. Substrate decides effect.** Nothing about a transaction's validity or
state effect is ever decided client-side by Prism (Prism doesn't even parse transaction
contents beyond raw bytes); nothing about *ordering* is ever decided by Aura/GRANDPA-style block
production — there is none. `node/src/bridge.rs` is the **only** place the two layers touch, and
the data only ever flows one way: a confirmed Prism leader level produces exactly one Substrate
block. Substrate never reorgs on this chain (every bridge-built block is imported
`finalized: true`), because Prism's own confirmation rule is the actual consensus — Substrate's
fork-choice bookkeeping here is just recording an already-decided sequence.

This split is why the two layers can be described (and were built) almost independently: the
signing/execution side only cares that *something* signs and validates extrinsics; Prism only
cares that it eventually hands the bridge an ordered, deduplicated list of opaque transaction
bytes. They are one machine with a clean internal seam, not two separate machines bolted together.

---

## 2. Crate map

```
                              ┌───────────────────────┐
                              │   node (argonaut-node)  │
                              │   binary crate           │
                              └───────────┬─────────────┘
                                          │ wires everything together
                    ┌─────────────────────┼─────────────────────┐
                    ▼                                            ▼
      ┌───────────────────────────┐                ┌───────────────────────────┐
      │   runtime (argonaut-runtime) │             │   client/prism-*             │
      │   FRAME pallets + extensions │             │   entirely client-side       │
      └───────────┬───────────────┘                └───────────┬───────────────┘
                  │                                            │
    ┌─────────────┼─────────────┐              ┌───────────────┼────────────────┐
    ▼             ▼             ▼              ▼                ▼                ▼
 primitives/   pallets/      (frame       prism-db          prism-chain      prism-miner
  falcon      falcon-       pallets:      (RocksDB           (DAG state       (mining loop,
  (sig types,  registry     balances,      column            machine)         sortition,
  host fns)    (on-chain    timestamp,     families,                          template
               registry)    sudo, ...)     checkpoints)                       assembly)
                                                                    │
                                              ┌─────────────────────┼─────────────┐
                                              ▼                                    ▼
                                        prism-network                       prism-ledger
                                        (gossip + sync                      (confirmation
                                         protocol over                       stages 1 & 2,
                                         sc-network)                         BuildLedger,
                                                                             bridge glue)
```

Dependency direction is strictly downward and never crosses back: `prism-chain` never imports
`prism-ledger` (confirmation *policy* lives in the ledger crate; the chain crate only exposes raw
DAG facts — votes, tips, levels). `prism-miner` and `prism-network` both depend on `prism-chain`
but not on each other or on `prism-ledger`. Nothing in `client/prism-*` depends on
`argonaut-runtime` or any Substrate block-authoring type — the whole DAG state machine, mining
loop, and confirmation logic can be (and is, in its own test suites) exercised with zero Substrate
runtime involvement at all.

---

## 3. Signatures & accounts: Falcon-512 + registry

### 3.1 Why Falcon-512

Argonaut's entire premise is post-quantum security. sr25519/ed25519 (the default in most
Substrate chains) are elliptic-curve schemes, broken by a sufficiently large quantum computer via
Shor's algorithm. [Falcon-512](https://falcon-sign.info/) is a lattice-based signature scheme
(NIST PQC standardization round finalist) with no known efficient quantum attack. Every account
in Argonaut signs with Falcon-512 by default; classical signatures exist only as an explicitly
gated developer convenience (§3.6).

The tradeoff: Falcon-512 keys and signatures are much larger than elliptic-curve ones —

| | sr25519 | Falcon-512 |
|---|---|---|
| Public key | 32 bytes | 897 bytes |
| Signature | 64 bytes | up to 752 bytes |

This size difference is *why* there are two transaction modes, not one (§3.3).

### 3.2 Core types and host functions (`primitives/falcon`)

```rust
pub struct FalconPublic(pub BoundedVec<u8, ConstU32<897>>);
pub struct FalconSignature(pub BoundedVec<u8, ConstU32<752>>);

pub enum ArgonautSignature {
    FalconWithKey { sig: FalconSignature, pk: FalconPublic },  // bootstrap mode
    Falcon { sig: FalconSignature },                           // registry mode
    #[cfg(feature = "dev-signatures")]
    Sr25519(sp_core::sr25519::Signature),                      // dev-only escape hatch
}
```

`AccountId` derivation is `blake3(pk)` — there is no separate "address" concept layered on top;
the account *is* the hash of the Falcon public key (`ArgonautSigner::into_account`).

The actual lattice-crypto verification never runs in wasm. It's exposed via a
`#[runtime_interface]` **host function** (`falcon_crypto::verify`, `falcon_crypto::pk_hash`),
which compiles to a host-function dispatch inside the runtime and to real native code
(`fn-dsa` crate) on the client side:

```
   wasm runtime                              native client
  ┌─────────────────┐                      ┌─────────────────────┐
  │ falcon_crypto::   │── host call trap ──▶│ falcon_crypto::verify │
  │  verify(sig,msg,pk)│                     │  (fn_dsa_vrfy crate,  │
  │  (just a stub in   │◀── bool result ────│   real math)          │
  │   wasm)             │                     └─────────────────────┘
  └─────────────────┘
```

This is the same pattern Substrate itself uses for sr25519/BLS host calls — it keeps expensive,
architecture-specific cryptography out of the wasm blob entirely.

### 3.3 Bootstrap mode vs registry mode

**Bootstrap mode** — a classic `Signed`-preamble extrinsic. The signature carries its own public
key inline, so the runtime can check `blake3(pk) == signer` and then verify, with no storage
access needed at all:

```
 UncheckedExtrinsic {
   preamble: Signed(
       address:   AccountId,
       signature: ArgonautSignature::FalconWithKey { sig, pk },   ~1.65 KB (897B pk + up to 752B sig)
       extra:     SignedExtra
   ),
   function: RuntimeCall
 }
        │
        ▼  Checkable::check()  (sp_runtime's generic UncheckedExtrinsic, unmodified)
   Verify::verify(&ArgonautSignature, payload, &address)
        │
        ├─ blake3(pk) == address?  ── No ──▶ reject
        │
        ▼ Yes
   falcon_crypto::verify(sig, blake3(payload), pk)   -- host function
        │
        ▼
   CheckedExtrinsic { ExtrinsicFormat::Signed(address, extra), function }
```

**Registry mode** — the public key is never sent at all. The signature and claimed account live
inside a `TransactionExtension` (`CheckFalconSignature`, §3.5) instead of the preamble, because a
`General`-preamble extrinsic (the only preamble shape with *no* `Verify`-checked signature) starts
with an **unauthorized** origin — something in the extension pipeline has to authorize it:

```
 UncheckedExtrinsic {
   preamble: General(
       extension_version: 0,
       extra: SignedExtra   -- CheckFalconSignature::RegistryMode { sig, account }
   ),
   function: RuntimeCall
 }
        │
        ▼  Checkable::check()  -- NO Verify:: check happens here; origin starts as RawOrigin::None
   CheckedExtrinsic { ExtrinsicFormat::General(0, extra), function }
        │
        ▼  SignedExtra tuple's validate(), position 0 = CheckFalconSignature
   pk = FalconRegistry::key_of(account)?          -- on-chain lookup; ~850 bytes saved on the wire
   msg = (extension_version, call).encode()
           ++ tail_extra.encode()        -- explicit data of extensions AFTER position 0
           ++ tail_implicit.encode()     -- implicit data of extensions AFTER position 0
   falcon_crypto::verify(sig, blake3(msg), pk)
        │
        ▼
   origin.set_caller_from_signed(account)          -- only NOW is the origin authorized
```

The registry-mode signing payload is **not** the old bootstrap-mode `SignedPayload` shape — it's
derived directly from `sp_runtime`'s tuple `TransactionExtension::validate`/
`DispatchTransaction::validate_only` behavior for an extension at tuple position 0 (worked out
from that source directly, not guessed; see `node/examples/submit_transfer_registry.rs`'s module
doc for the exact derivation). This is why registry-mode tooling needed its own transaction
builder rather than reusing bootstrap mode's.

### 3.4 The registry pallet (`pallets/falcon-registry`)

A single storage map, a registration deposit, and a lookup trait:

```rust
pub type Keys<T: Config> = StorageMap<_, Blake2_128Concat, T::AccountId, FalconPublic, OptionQuery>;

pub trait KeyProvider<AccountId> {
    fn key_of(who: &AccountId) -> Option<FalconPublic>;
}
```

`register_key(pk)` is itself a **bootstrap-mode** call (it must be — there's no registry entry
yet to check registry-mode against): it verifies `blake3(pk) == caller`, reserves `KeyDeposit`
from the caller's balance, and stores the key. `deregister_key()` removes the entry and refunds
the deposit. The deposit exists purely to deter on-chain state spam (~900 bytes of permanent
storage per registrant).

### 3.5 The `CheckFalconSignature` transaction extension (`runtime/src/extensions.rs`)

This is the piece that makes registry mode possible without changing the extrinsic preamble
format Substrate itself understands. It sits **first** in the runtime's `SignedExtra` tuple:

```rust
pub type SignedExtra = (
    CheckFalconSignature,                          // ← must run first
    frame_system::CheckNonZeroSender<Runtime>,
    frame_system::CheckSpecVersion<Runtime>,
    frame_system::CheckTxVersion<Runtime>,
    frame_system::CheckGenesis<Runtime>,
    frame_system::CheckEra<Runtime>,
    frame_system::CheckNonce<Runtime>,
    frame_system::CheckWeight<Runtime>,
    pallet_transaction_payment::ChargeTransactionPayment<Runtime>,
);
```

It must run first because every extension *after* it (`CheckNonce`, `CheckWeight`, ...) assumes
an already-authorized signer — for a `General`-preamble extrinsic, nothing else in the pipeline
is in a position to authorize the origin.

```rust
pub enum CheckFalconSignature {
    RegistryMode { sig: FalconSignature, account: AccountId },
    Disabled,   // no-op: used for bootstrap-mode (Signed-preamble) extrinsics
}
```

`Disabled` is a pure no-op — bootstrap mode's signature was already checked by the generic
`Checkable::check` before any extension runs at all, so this extension has nothing left to do for
that path. This asymmetry matters (§4.9): **only** registry-mode extrinsics ever pass through
`CheckFalconSignature::validate`'s real logic; bootstrap-mode ones never do.

### 3.6 Security model: the `dev-signatures` gate

`ArgonautSignature::Sr25519(..)` exists purely for developer convenience (well-known `//Alice`
style dev accounts, benchmarking, etc.) and is gated behind a real Cargo feature,
**`dev-signatures`**, off by default at every layer — including the chain specs and CLI tooling
that used to require it just to compile:

```
  dev-signatures = off (default)                dev-signatures = on (opt-in)
 ┌─────────────────────────────────┐          ┌─────────────────────────────────┐
 │ ArgonautSignature::               │          │ ArgonautSignature::               │
 │   FalconWithKey { .. }             │          │   FalconWithKey { .. }             │
 │   Falcon { .. }                    │          │   Falcon { .. }                    │
 │   (Sr25519 variant does not exist) │          │   Sr25519(..)                      │
 └─────────────────────────────────┘          └─────────────────────────────────┘
        ▲                                              ▲
        │ default node build + production wasm         │ `cargo build --features
        │ (chain_spec dev accounts derived directly     │  dev-signatures` -- native
        │  via sp_core, no feature dependency)          │  benchmarking/example tooling
```

This wasn't the original design, and it took two separate live-discovered bugs to actually get
right. First: an earlier iteration gated the variant on `#[cfg(feature = "std")]` to fix a
wasm/native SCALE mismatch, which overshot and left sr25519 permanently accepted by the
*production* wasm runtime. Second, found later (2026-07-07): `node`'s own `Cargo.toml` was
unconditionally activating `dev-signatures` on the runtime crate (needed at the time because
`chain_spec.rs`'s dev-account funding went through the feature-gated signer type) — and
`substrate-wasm-builder` mirrors whatever features are active on the runtime crate's *native*
build into the separate wasm build it spawns, so the feature leaked straight back into the
"production" wasm blob despite the gate existing in the type. The fix was to also decouple
`chain_spec.rs`'s dev-account derivation from the feature entirely (it only ever needed
`sp_core::AccountId32: From<sr25519::Public>`, a conversion with no dependency on Argonaut's own
signer types at all), so `dev-signatures` could finally become genuinely off by default with
nothing else silently requiring it.

### 3.7 Tooling

- `argonaut-node falcon-key generate|inspect` (`node/src/keygen.rs`) — keystore generation and
  inspection, optionally passphrase-encrypted.
- `argonaut-node falcon-sign` (`node/src/falcon_sign.rs`) — offline bootstrap-mode signing.
- `node/examples/submit_transfer.rs` — full bootstrap-mode transaction builder (genesis hash,
  nonce, era, tip, RPC submission). Requires `--features dev-signatures` (it signs via sr25519 for
  convenience against well-known dev accounts).
- `node/examples/submit_transfer_registry.rs` — the registry-mode counterpart, signed with a real
  Falcon-512 keystore; requires the signer already registered on-chain.

### 3.8 Tests and acceptance

| Acceptance criterion | Status |
|---|---|
| Sign/verify round-trip; corrupted sig/pk/msg each fail; `pk_hash` matches independent blake3 | ✅ |
| Bootstrap tx succeeds and key appears in storage | ✅ |
| Registry-mode tx smaller; rejected in pool if unregistered | ✅ |
| Weight/size assertion test (registry-mode ≤ 850 bytes) | ✅ real Falcon-512 keypairs, `runtime/src/tests.rs` |
| wasm build unaffected | ✅ live-verified 2026-07-07 (see §3.6) |
| Auto-register-on-first-bootstrap-tx (optional) | Not implemented — see §6 |

---

## 4. Consensus: Prism

### 4.1 Why Prism

A conventional single chain forces a tradeoff between block size/frequency (throughput) and safe
confirmation depth (latency/security) — you cannot turn either knob without moving the other.
[Prism](https://arxiv.org/abs/1810.08092) (Bagaria et al.) decomposes "blockchain" into separate
concerns running on **separate, parallel proof-of-work chains**, so throughput and confirmation
latency can each be tuned independently:

- **m voter chains** — pure Nakamoto longest-chain PoW, one per chain, whose only job is to vote
  on which proposer chain block is canonical at each level. Confirmation security comes from
  having *many* independent chains agree, not from depth on any single one.
- **1 proposer tree** — orders content by referencing transaction blocks and other proposer
  blocks. The proposer block with the most votes at a level is that level's **leader**.
- **1 transaction chain** — pure throughput: just carries opaque transaction bytes, referenced by
  proposer blocks. Never itself the target of consensus.

All of this runs **entirely client-side**, independent of and prior to any Substrate block
production (§1). Argonaut's runtime never runs Aura, GRANDPA, or any other Substrate consensus
engine — Substrate blocks are produced only as a downstream artifact of Prism confirmation
(§4.9).

### 4.2 The super-block structure

Every mined Prism block is one of `m + 2` "slots" (m voter chains + 1 tx chain + 1 proposer
tree), but they all share one PoW header, committing to every slot's parent and content via two
Merkle roots:

```
                         SuperHeader
                 ┌─────────────────────────┐
                 │ parents_root  (Merkle)    │──▶ merkle_root([voter0_parent, ..., voterM-1_parent,
                 │ content_root  (Merkle)    │              tx_parent, proposer_parent])
                 │ timestamp_ms              │
                 │ nonce                      │──▶ merkle_root([voter0_content, ..., voterM-1_content,
                 │ difficulty                 │              tx_content, proposer_content])
                 └─────────────────────────┘
                              │
              block_hash = blake3(SCALE(header))
                              │
              sortition(block_hash, params) decides WHICH of the
              m+2 slots this successful PoW hash actually mines
```

A miner assembles **one** candidate template covering all `m + 2` slots (`MinerTemplate`), then
searches nonces; whichever slot the winning hash's sortition range falls into determines what
block actually gets emitted (§4.4). Only that one slot's content, parent, and a Merkle
*membership proof* against the two roots are ever transmitted — not all `m + 2` slots' data.

### 4.3 Sortition — mapping a PoW hash to a block type

```
 U256 hash space (big-endian interpretation of block_hash), m = params.m_voter_chains:

 0                                                                              2^256 - 1
 ├──────────────── m · f_v ─────────────────┼── f_t ──┼── f_p ──┼──── Fail ────┤
 │  Voter(0) │ Voter(1) │ ... │ Voter(m-1)  │Transaction│Proposer│  (no valid    │
 │◄──f_v────►│◄──f_v───►│     │◄──f_v──────►│           │        │   block)      │
 0        f_v        2·f_v            m·f_v      +f_t      +f_p              max
```

`f_v`, `f_t`, `f_p` are equal fractions of one shared combined `difficulty` (testnet: an equal
`1/(m+2)` split). A hash landing in the voter range further divides by `f_v` to pick *which*
voter chain (`0..m-1`) it mined for. This single shared difficulty threshold is what
[difficulty retargeting](#410-difficulty-retargeting) adjusts — the three-way split stays fixed
so retargeting the combined rate never disturbs the intended slot-type ratio.

### 4.4 Block validation

Every incoming block is checked structurally before it ever reaches the DAG state machine
(`validate_block` in `primitives/prism`):

1. **PoW check** — `block_hash <= header.difficulty`.
2. **Sortition consistency** — the hash's sortition result must match the block's claimed content
   type (a block can't claim to be a voter block if its hash sorts into the proposer range).
3. **Content sanity** — voter votes strictly ascending; tx blocks ≤ 32 KB; chain id `< m`.
4. **Merkle proofs** — the claimed parent and content each verify against the header's two roots.

This is pure, stateless, `no_std` logic (`primitives/prism` has zero Substrate/DB dependencies) —
it doesn't need to know anything about the current DAG to reject a structurally-malformed block.

### 4.5 The DAG state machine (`client/prism-chain`)

This is where all the interesting consensus logic lives. A small example DAG, mid-flight:

```
 Voter chain 0:  G0 ── v0_1 ── v0_2 ── v0_3  (tip, depth 3)
 Voter chain 1:  G1 ── v1_1 ── v1_2          (tip, depth 2)
 Voter chain 2:  G2 ── v2_1 ── v2_2 ── v2_3  (tip, depth 3)
                          │        │        │
                     votes:[P1] votes:[P1] votes:[P1,P2]

 Proposer tree:  Pgen ── P1 (level 1) ── P2 (level 2, best)
                          │                │
                    tx_refs:[T1,T2]    tx_refs:[T3]
                    prop_refs:[]        prop_refs:[]

 Tx blocks:  T1  T2  T3   (all referenced)   T4  (still unreferenced)
```

Each **voter chain** is an independent Nakamoto longest-chain: a new block extends the current
tip or starts/extends a competing fork. If a competing fork overtakes the tip, `reorg_voter_chain`
walks both branches back to their lowest common ancestor, **unwinds** votes cast on the abandoned
segment, and **re-applies** votes cast on the newly-adopted one:

```
 old_tip ── ... ── LCA ── ... ── new_tip
    │                              │
    └─ unwind these blocks'         └─ re-apply these blocks'
       votes (walk old_tip → LCA)      votes (walk LCA → new_tip,
                                         oldest-first)
```

`recount_votes()` independently rebuilds every vote total from scratch by replaying each chain's
*current* canonical history — this must always agree with the incrementally-maintained `votes`
map, and is exercised as a property test on every insertion sequence the test suite throws at it.

Every accepted block re-drains an **orphan pool** afterward (`drain_orphans`) — a block whose
parent/refs weren't yet known is held, not rejected, and retried every time *any* new block is
accepted, resolving dependency chains to a fixed point regardless of arrival order. This same
fixed-point property is what makes restart-time replay order-independent (§4.8).

Per-chain **genesis hashes** (not a single shared sentinel) domain-separate every voter chain and
the proposer tree from every other Argonaut network:

```rust
genesis_hash(chain_spec_genesis, slot) = blake3(chain_spec_genesis ++ slot_le_bytes)
// slot 0..m-1 → voter chain i's genesis
// slot m      → the proposer tree's genesis
// (tx blocks parent onto proposers -- no genesis slot of their own)
```

A block claiming chain `i` whose parent is *some other* chain's genesis is rejected outright, not
orphaned — that ancestry can never resolve into anything legitimate.

### 4.6 Mining (`client/prism-miner`)

Mining runs on a plain OS thread per the design spec (not async — nonce search is CPU-bound):

```
  build_template(chain, mempool, queue, q, params)         ── pure, testable function
        │
        ├─ voter[i] parent = chain.voter_tips[i]; content = chain.unvoted_levels[i] (ascending)
        ├─ tx parent = deepest unreferenced proposer; content = mempool txs from `queue`
        │              (queue = blake3(tx) % q -- "queue coloring", bounds duplication
        │               across concurrently-mined tx blocks)
        └─ proposer parent = same deepest unreferenced proposer;
                     content = every other unreferenced tx block + unreferenced proposer
        │
        ▼
  try_mine(template, params, start_nonce, max_attempts)     ── pure, synchronous nonce search
        │
        └─ carries next_nonce forward across calls -- callers MUST resume from it, never
           restart at 0, or they re-search an already-exhausted range forever
        │
        ▼
  spawn_mining_thread: loop { tmpl = template.load(); params = params.load(); try_mine(...) }
                       (ArcSwap -- wait-free template/params hand-off from the main thread)
```

### 4.7 Networking (`client/prism-network`)

Two protocols, layered over `sc-network`:

```
  /argonaut/prism/blocks/1   (notifications, push)
     ── full PrismBlock (or a hash-only announcement if it's too large) ──▶ peer
     peer: validate_block → chain.insert_block → re-gossip if newly accepted

  /argonaut/prism/sync/1     (request/response)
     GetBlocks(hashes)  ──▶  Blocks(prism_blocks)          -- orphan/ref resolution
     GetStatus          ──▶  Status { proposer_best_level, voter_tips,
                                        unreferenced_proposers, unreferenced_tx_blocks }
```

`GetStatus`/`Status` drive **initial sync / backfill**: on connect (and every 30s per peer), a
node asks for the peer's DAG frontier, filters it through `PrismChain::has_block` (a genuine DB
check, not just an in-memory "have I seen this" cache — the latter is empty after every restart),
and feeds unknown hashes into a bounded, batched (64/round) work queue with a per-hash retry
limit. Backfilled blocks skip re-gossip but are otherwise validated and inserted identically to
the live path.

`has_block` is load-bearing beyond backfill too: re-`insert_block`-ing an already-accepted
proposer is **not idempotent** (it would wrongly re-add it to `unreferenced_proposers`), so
anything that needs "do I already have this" must consult it, not a network-worker-local cache.

### 4.8 Confirmation (`client/prism-ledger`)

Two independent confirmation rules, layered:

```
 STAGE 1 — conservative k-deep (Bitcoin-like latency, obviously safe)

    voter chain i:  ... ── vote-casting block (depth d) ── ... ── tip (depth D)

    proposer P at level ℓ is STAGE-1 CONFIRMED when:
      1. EVERY voter chain (0..m-1) has cast a vote for P            (unanimous)
      2. for every such vote:  tip_depth - cast_depth >= k           (K_DEEP_CONFIRMATION = 6)

 STAGE 2 — §4.5 confidence-interval fast confirmation (paper Def. 4.5)
    a probabilistic rule that can confirm a *uniquely determined* leader well before
    every chain has voted -- see confirmation.rs. On by default (B-15; --prism-no-stage2
    opts out). Its margin is single_check_margin, re-derived from the paper's own Chernoff
    bound for a single check at the chain spec's prismBetaPercent / prismEpsExponent (the
    paper's literal Appendix E.1 k_min is thousands of blocks deep -- an asymptotic-proof
    artifact). At m=100 that margin is vacuous below ~400-deep votes, so stage 2 is a
    safety backstop for a stalled (silent-chain) stage 1, not a latency win.

 PrismLedger::advance() tries stage 2 first, falls back to stage 1 -- whichever fires
 first in real time is what confirms the level. Stage 2 is provably never LESS safe
 than stage 1 alone (its candidate list is guaranteed to contain the eventual leader).
```

Once a level is confirmed, **BuildLedger** (paper Alg. 2) turns the leader into a flat, ordered,
deduplicated list of transaction bytes:

```
 confirmed leader P2 (level 2)
        │
        ├─ prop_refs → P1                              (DFS, ref order)
        │                ├─ tx_refs → T1, T2
        │                └─ prop_refs → (none)
        └─ tx_refs → T3

 visitation order:  T1, T2, T3
 emitted:  each tx's blake3 hash checked against a durable, ACROSS-EVERY-CONFIRMED-LEVEL
           dedup set -- a tx referenced again by a later proposer is silently skipped,
           not re-emitted
```

This dedup set (`emitted_tx_hashes`) is what makes the *input* to Substrate block-building a
pure, deterministic function of the confirmed DAG — every honest node that agrees on the leader
sequence agrees on this list bit-for-bit, independent of each node's own local mempool (which
only ever influences what that node *mines*, never what any node *confirms*).

### 4.9 The Substrate bridge (`node/src/bridge.rs`)

The one-way funnel from confirmed Prism state into Substrate blocks:

```
 PrismLedger::advance() returns a ConfirmedLevel { level, leader, txs, timestamp_ms }
        │
        ▼
 build_and_import_block:
   1. BlockBuilderBuilder::on_parent_block(current best) .with_inherent_digests([
        DigestItem::PreRuntime(*b"prsm", LeaderDigest{level, leader}.encode())
      ])
   2. push the timestamp inherent -- the LEADER's own timestamp_ms (baked into its hash,
      so every node agrees bit-for-bit), clamped forward against on-chain pallet_timestamp::Now
      to respect pallet_timestamp's monotonicity invariant. NEVER SystemTime::now() --
      two honest nodes confirming the same level moments apart would otherwise produce
      different block hashes.
   3. push each tx (decode failure or apply failure -- SKIP, never abort the whole block:
      Prism itself never validates tx content, only Substrate's execution does)
   4. builder.build() → (header, body)
        │
        ▼
   BlockImportParams { finalized: true, fork_choice: Custom(true),
                        state_action: ApplyChanges(pre-computed storage changes) }
        │
        ▼
   self-import (this node, via block_import.import_block directly -- never through
   the import queue/verifier below)
```

Every **other** node receives the identical block via ordinary Substrate block-sync and imports
it through `PrismBridgeVerifier` (always finalizes, always best — Prism already decided):

```
   peer import path:  PrismBridgeVerifier::verify → import queue
                       → BlockImportParams::new's DEFAULT state_action = Execute
                       → runtime re-executes the block end to end (Core_execute_block)
                         to verify state root / extrinsics root, same as any Substrate import
```

> **A real, live-discovered lesson worth preserving:** the leader digest was originally attached
> via `BlockImportParams::post_digests` — a mechanism that only survives the *authoring* node's
> own trusted self-import (`StateAction::ApplyChanges` never calls back into the runtime at all).
> Any other node importing the identical block via ordinary sync re-executes it
> (`StateAction::Execute`), and `frame_executive::Executive::final_checks`' digest-count
> `assert_eq!` **panicked** on every such import — `Executive::initialize_block`'s
> `extract_pre_digest` only preserves `DigestItem::PreRuntime` entries, silently dropping the
> `Other`-tagged leader digest during re-verification. In practice: **any node that ever fell
> behind the Substrate chain could not safely rejoin the network at all.** This was found live
> on a real two-node devnet run (restarting a node that had missed a confirmed level), not by any
> unit test — the fix is to attach the digest as `DigestItem::PreRuntime`, set *before*
> `builder.build()` via `.with_inherent_digests(...)`, which `initialize_block` preserves
> identically whether the block is being authored or re-verified. If you ever add another custom
> digest item to a bridge-built block, it must be `PreRuntime`, attached before building — not
> `Other`, and not `post_digests` — unless you have specifically verified it never needs to
> survive a peer's re-execution.

### 4.10 Difficulty retargeting

A Bitcoin-style rolling-window retarget, entirely client-side:

```
  every RETARGET_WINDOW (=20) accepted blocks (any slot type -- the COMBINED rate is
  what's retargeted; sortition only splits an already-successful hash afterward):

    actual_elapsed   = newest_timestamp - oldest_timestamp (over the window)
    expected_elapsed = target_interval_ms * RETARGET_WINDOW
    ratio            = clamp(actual_elapsed / expected_elapsed, 1/4, 4)    -- Bitcoin-style clamp
    new_target       = current_target * ratio

  new_target is then re-split EQUALLY across all m+2 slots (Params::set_combined_target)
  so the voter:tx:proposer ratio is invariant under retargeting.
```

The current target persists across restarts (`CURRENT_DIFFICULTY_TARGET` column) — a restarted
node resumes from wherever retargeting last left off, not `Params::testnet()`'s initial guess.

### 4.11 Storage, checkpoints, and restart recovery (`client/prism-db`)

Five live RocksDB column families (an earlier 13-CF schema copied from the reference
implementation was deleted — never populated, no encoding ever designed for it, and this port's
actual data model has no equivalent to be "compatible" with):

```
 BLOCKS                      full block bodies
 BLOCK_STATUS                per-block status (incl. BlockStatus::Pruned tombstones)
 CURRENT_DIFFICULTY_TARGET   §4.10's persisted retarget state
 EMITTED_TX_HASH             durable BuildLedger dedup set (survives pruning of tx bodies)
 CHAIN_CHECKPOINT            periodic snapshot of every in-memory DAG index (§below)
```

On restart, `PrismChain::with_db` loads the most recent `ChainCheckpoint` (if any), then replays
only the blocks in `BLOCKS` that `has_block` doesn't already recognize — bounded by how long ago
the last checkpoint was written (every 256 accepted blocks), not by total chain history. This
replaced an earlier, much worse bug: without any restart recovery, a node didn't just lack fast
recovery, it silently **re-confirmed every already-settled level from 1** and double-built
Substrate blocks on top of the real chain — found live, not a hypothetical. `PrismLedger::recover`
independently rebuilds its own confirmed-level cursor from the Substrate client's own best-block
number (the only value that can never be ahead of what's truly committed) and its tx-dedup set
directly from `EMITTED_TX_HASH`.

### 4.12 Pruning

Off by default (`--prism-prune`), because a pruning node cannot serve full historical backfill to
a fresh peer — every devnet should keep at least one non-pruning archive node. Two independent
things are trimmed, bounded by two independent inputs:

```
  voter blocks:        bounded by --prism-k-prune alone (default 60, i.e. 10x stage-1's k).
                        NEVER touches vote accounting (both confirmation stages only ever
                        read `votes`/`voter_tips`, never voter-block history) -- purely a
                        reorg-processability bound, since a live reorg's LCA walk would
                        panic if it ever needed to cross the pruned frontier.

  proposer/tx bodies:  bounded by the ledger's own confirmed level (supplied by the bridge,
                        which owns both PrismChain and PrismLedger -- prism-chain itself
                        never depends on prism-ledger). Only the CONFIRMED LEADER's own
                        subtree is swept; proposer_meta/known_tx_blocks are kept forever
                        (every confirmation query only ever needs level/existence, not body
                        content).
```

Pruned hashes are tombstoned (`BlockStatus::Pruned`), not deleted from the index — so `has_block`
and backfill both correctly treat a pruned hash as "known, don't re-request" rather than
mistaking it for genuinely missing.

### 4.13 Parameters (testnet defaults)

| Parameter | Value | Meaning |
|---|---|---|
| `m_voter_chains` | 100 | number of parallel voter chains |
| `target_interval_ms` | 1000 | target combined interval between any of the m+2 slots |
| `K_DEEP_CONFIRMATION` | 6 | stage-1's k-deep threshold |
| `MAX_TX_BLOCK_BYTES` | 32 KiB | per tx-block size cap |
| `RETARGET_WINDOW` | 20 | blocks averaged per retarget decision |

---

## 5. End-to-end transaction lifecycle

Tying the whole machine together, from a signed transaction to a settled balance:

```
 1. Wallet signs an extrinsic (bootstrap or registry mode, §3.3) and submits it via RPC.
                                     │
 2. The node's Substrate transaction pool validates it: CheckFalconSignature (registry mode
    only) + the standard nonce/era/weight extensions all run against the pool's view of state.
                                     │
 3. A LOCAL MINING THREAD (client/prism-miner) pulls the raw tx bytes into its next tx-block
    template -- this is the FIRST and ONLY place a node's own mempool matters. Different
    nodes may include the same tx in different tx blocks, or at different times -- harmless,
    because confirmation dedups later (step 6).
                                     │
 4. The tx block is mined, gossiped (client/prism-network), and referenced by some proposer
    block (possibly on a different node) -- this is what makes it PART OF THE DAG, not yet
    part of any confirmed order.
                                     │
 5. Voter chains across the network vote for that proposer (or a competing one) at its level.
    Fork-choice/reorg reconciles competing votes as voter chains grow (client/prism-chain).
                                     │
 6. Once EITHER stage-1 (unanimous, k-deep) or stage-2 (confidence-interval) confirms a
    unique leader at that level (client/prism-ledger), BuildLedger expands the leader's
    tx_refs/prop_refs into a flat, deduped list -- THIS is the first point the tx's ORDER
    is decided, and it's identical on every honest node.
                                     │
 7. node/src/bridge.rs builds exactly one Substrate block from that list: pushes the
    deterministic timestamp inherent, applies each tx (skipping decode/apply failures,
    never aborting the whole block), and imports it `finalized: true`.
                                     │
 8. The Substrate runtime (pallet-balances, etc.) is what actually decides EFFECT: nonce
    checks, balance checks, and the actual balance mutation all happen here, for the first
    and only time -- this is exactly the "Prism orders, Substrate executes" split from §1.
```

---

## 6. Known limitations and deliberate compromises

These are documented in code comments in full detail; summarized here so this document doesn't
read as more finished than it is:

- **Stage 2 is a backstop, not a fast path, at `m = 100`.** Its margin (`single_check_margin`,
  B-3/B-15) is derived rather than a placeholder, but it is vacuous below ~400-deep votes. So
  in practice stage 1 confirms everything, and stage 2 only rescues a level that stage 1's
  unanimity would stall on forever (a silent voter chain). Faster stage 2 needs a larger `m` or
  a smaller assumed β. The bridge finalizes with no reorg path, so anything holding real value
  should set `prismEpsExponent` ≥ 9.
- **Two honest miners can, in the exact-tie case, stall stage-1 confirmation forever** — proven
  structural (not a numerical shortfall) at small `m`; vanishingly unlikely at the paper's
  intended `m` (100+). Don't scale `m` down to ~4 expecting fast guaranteed confirmation.
- **A single global `Mutex<PrismChain>`** serializes the miner, network insert path, and bridge —
  fine for a devnet, a real scalability wall at `m = 100` and high throughput. Deliberately not
  optimized until measured to matter.
- **DAG index CFs are unpopulated beyond the periodic checkpoint** — restart replay cost is
  bounded by the checkpoint interval, not fully O(1).
- **No docker-compose devnet artifact yet** — the repo's `docker-compose.yml`/`scripts/*.sh` are
  still unmodified `substrate-node-template` boilerplate.
- **Auto-register-on-first-bootstrap-tx** turned out to be more involved than originally scoped:
  `sp_runtime`'s generic `Checkable::check` verifies and then **discards** the bootstrap-mode
  public key before any `TransactionExtension` runs, so there is no point in the current extension
  pipeline that ever sees it. Implementing this for real needs a custom
  `Checkable`/`UncheckedExtrinsic`, not a branch inside `CheckFalconSignature`.

## 7. Where to go next

- **[`ARGONAUT_ARCHITECTURE_GUIDE.md`](./ARGONAUT_ARCHITECTURE_GUIDE.md)** — original design
  intent for the whole system, including the parts of the machine not built yet: succinct proof
  aggregation and decentralized storage.
- **[`IMPLEMENTATION_D_SPEC.md`](./IMPLEMENTATION_D_SPEC.md)** — the decentralized storage
  subsystem's detailed build spec, ready to build from when its turn comes.
