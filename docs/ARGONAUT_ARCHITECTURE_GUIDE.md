# Argonaut Architecture & Implementation Guide

**Purpose of this document.** This is a handoff specification for implementing three subsystems in the Argonaut repository (https://github.com/Kyleleesang/Argonaut): (A) Falcon-512 transaction signatures with an on-chain public-key registry, (B) the Prism consensus protocol (Bagaria et al., "Prism: Deconstructing the Blockchain to Approach Physical Limits"), and (C) an optional STARK-based hash-signature aggregation layer. It is written so that a capable coding model can implement each phase with minimal additional research. Every section states: what to build, which existing libraries to use, the exact interfaces, and acceptance criteria.

**Read this first — three global rules:**
1. **The runtime is only the state transition function (STF).** Prism block types must NOT implement Substrate's `BlockT`/`HeaderT` traits. All Prism machinery lives client-side. The existing code in `runtime/src/Consensus/` should be treated as design notes and deleted/rewritten, not fixed in place.
2. **Everything on the wire or in storage uses SCALE codec** (`parity-scale-codec` `Encode`/`Decode` + `scale_info::TypeInfo`), never serde, never `std::hash::Hash` for content addressing. Content hashes are computed over SCALE encodings with BLAKE3.
3. **Native-only crypto goes behind host functions.** Anything that cannot compile to `wasm32-unknown-unknown` (PQClean C code, RocksDB, provers) is exposed to the runtime via `sp_runtime_interface` or kept entirely in the node.

---

## 0. Feasibility & Library Inventory (verified June 2026)

| Need | Library | Status / Notes |
|---|---|---|
| Substrate framework | `polkadot-sdk` (paritytech/polkadot-sdk), stable release branch (e.g. `stable2412` or newer) | The old `paritytech/substrate` repo and its `polkadot-v0.9.29` branch (what Argonaut currently pins) are **archived**. Migration is mandatory. Start from `polkadot-sdk-solochain-template`. |
| PoW consensus scaffolding | `sc-consensus-pow`, `sp-consensus-pow` (in polkadot-sdk) | Maintained. Used only for the interim single-chain PoW milestone; full Prism replaces it. |
| Falcon-512 | Option 1: `fn-dsa` crate family (`fn-dsa`, `fn-dsa-sign`, `fn-dsa-vrfy`) — pure Rust, by Thomas Pornin, no C toolchain needed. Option 2: `pqcrypto-falcon` (wraps PQClean C). | Both exist on crates.io. **Prefer `fn-dsa`**: pure Rust removes cross-compilation headaches and its verify half is small; a 2026-07-14 survey found no production-ready Falcon crate anywhere (PQClean itself is being archived, pointing to fn-dsa as the replacement) — this is standard-immaturity, not a crate-quality gap, so don't crate-shop later. Caveat: FIPS 206 (FN-DSA) is at Initial Public Draft; final standard expected late 2026/early 2027; the crate's own README states no backward compatibility until 1.0. Pin the crate version. **Correction (2026-07-16): migration at the final standard is NOT a one-file fix** — see §7's risk register, this row was wrong until now. |
| BLAKE3 | `blake3` crate | Stable. Use for all content addressing and for `AccountId = blake3(pk)`. |
| Prism reference logic | `prism-rust` (github.com/yangl1996/prism-rust) — the paper authors' full implementation: miner, blockdb, blockchain (DAG state), ledger-manager, network | MIT-ish licensed research code from 2019. Does not compile against modern deps out of the box; **port module logic, do not depend on the crate**. The Argonaut repo already copied its RocksDB column-family names — keep them. |
| Embedded DB for Prism DAG | `rocksdb` crate (or `parity-db` if preferred) | Stable. Prism data lives in its own DB, separate from Substrate's client DB. |
| Networking | `sc-network` custom protocols (notifications + request/response), or raw `rust-libp2p` | Both viable. Recommended: `sc-network` so Prism gossip shares the node's existing swarm. |
| XMSS hash-based signatures | `hash-sig` (github.com/leanEthereum/hash-sig) — actively maintained reference impl of the "Hash-Based Multi-Signatures for Post-Quantum Ethereum" scheme (XMSS over Poseidon2/SHA-3) | Research-grade, active. Pin a commit. |
| STARK aggregation prover | `leanMultisig` / leanVM (github.com/leanEthereum/leanMultisig) — minimal zkVM purpose-built for XMSS aggregation; benchmarked ≈1,000 XMSS verifications proven/sec on laptop-class hardware; recursion supported. Alternatives: hand-written AIRs in `winterfell` (the 2022 ASIA CCS paper's approach) or `Plonky3`. | Pre-1.0, unaudited, API churn expected. **Wrap it behind an internal `trait AggregationBackend` so it can be swapped.** |
| Poseidon2 (native + in-circuit) | Provided inside `Plonky3` / leanMultisig stack | Fine for prototype. Note: algebraic hashes are under ongoing cryptanalysis; design for hash agility (see Risk Register). |

**Feasibility verdicts:**
- (A) Falcon + registry: **fully buildable today**, low risk. ~1–2 weeks of work.
- (B) Prism: **buildable today**; no library gap, but it is a substantial port (~8–12k lines including tests). The reference implementation removes algorithmic risk.
- (C) STARK aggregation: **buildable today at prototype quality**; production quality is gated on upstream (leanVM audits/stabilization), not on missing functionality.

---

## 1. Phase 0 — Foundation: Migrate & Restructure the Repo

### 1.1 Target workspace layout

```
argonaut/
├── Cargo.toml                  # workspace
├── node/                       # service wiring, CLI, RPC
├── runtime/                    # FRAME runtime (STF only — no consensus types)
├── pallets/
│   ├── falcon-registry/        # Implementation A
│   └── agg-verify/             # Implementation C (runtime side)
├── primitives/
│   ├── falcon/                 # A: types + host functions (no_std compatible interface)
│   ├── prism/                  # B: block types, sortition, constants (no_std, SCALE)
│   └── agg/                    # C: bundle formats, host functions
├── client/
│   ├── prism-db/               # B: RocksDB block store
│   ├── prism-chain/            # B: DAG state machine (blocktree, votes, leaders)
│   ├── prism-miner/            # B: superblock assembly + PoW loop + sortition
│   ├── prism-network/          # B: gossip + missing-block fetch protocols
│   ├── prism-ledger/           # B: confirmation rules + BuildLedger + Substrate bridge
│   └── aggregator/             # C: batch collector + prover driver (binary target too)
└── docs/                       # this file + per-crate READMEs
```

### 1.2 Migration steps

1. Regenerate the project from the current `polkadot-sdk-solochain-template`; copy over `chain_spec` customizations. Do NOT try to bump the `polkadot-v0.9.29` pins in place — the API distance is too large.
2. Delete `runtime/src/Consensus/*` and `node/src/Mining/*` (superseded by `primitives/prism` and `client/prism-*`). Keep the RocksDB column-family constant list; move it to `client/prism-db/src/columns.rs`.
3. Fix `primitives` version pinning: every `sp-*`/`sc-*`/`frame-*` dependency in every crate must come from the same polkadot-sdk release. The current repo mixes `sp-core 6.0.0` (git branch) with `sp-core 27.0.0` (crates.io) — this cannot link.
4. Runtime hygiene: remove Aura + GRANDPA pallets and their runtime APIs **at the Prism milestone** (Phase B4), not before. Until then the chain must keep running under Aura so every other phase is testable on a live chain.
5. Naming: rustify everything (snake_case fields, no `AccountId`-typed fields inside header structs, no serde derives).

**Acceptance criteria:** `cargo build --release` green; `--dev` node produces blocks under Aura; `cargo test` green; CI added for `wasm32-unknown-unknown` runtime build.

---

## 2. Implementation A — Falcon-512 Signatures + Public-Key Registry

### 2.1 Design summary

- `AccountId32 = blake3(falcon512_public_key_bytes)`.
- Runtime `Signature` type is a custom enum supporting Falcon (and optionally sr25519 for dev accounts during bring-up).
- Verification runs **natively** via a `#[runtime_interface]` host function (Falcon verify won't/shouldn't run inside wasm).
- Two transaction modes:
  - **Bootstrap mode:** transaction carries `(signature, full_public_key)`; runtime checks `blake3(pk) == signer` then verifies.
  - **Registry mode:** transaction carries only `(signature)`; runtime loads pk from `pallet-falcon-registry` storage. ~2x tx/GB improvement (897 B saved per tx).

### 2.2 `primitives/falcon` — types and host functions

```rust
// no_std-friendly public API
pub const FALCON512_PK_LEN: usize = 897;
pub const FALCON512_SIG_MAX_LEN: usize = 752; // padded/varlen; store actual len

#[derive(Encode, Decode, TypeInfo, Clone, PartialEq, Eq, Debug)]
pub struct FalconPublic(pub BoundedVec<u8, ConstU32<897>>);

#[derive(Encode, Decode, TypeInfo, Clone, PartialEq, Eq, Debug)]
pub struct FalconSignature(pub BoundedVec<u8, ConstU32<752>>);

/// The runtime-facing signature envelope.
#[derive(Encode, Decode, TypeInfo, Clone, PartialEq, Eq, Debug)]
pub enum ArgonautSignature {
    /// signature + inline public key (bootstrap mode)
    FalconWithKey { sig: FalconSignature, pk: FalconPublic },
    /// signature only; pk resolved from registry (registry mode)
    Falcon { sig: FalconSignature },
    /// dev-only escape hatch, feature-gated out for release builds
    Sr25519(sp_core::sr25519::Signature),
}

#[runtime_interface]
pub trait FalconCrypto {
    /// Verify a Falcon-512 detached signature. Pure function; native-only impl.
    fn verify(sig: &[u8], msg: &[u8], pk: &[u8]) -> bool;
    /// blake3 of pk → 32 bytes (account id derivation helper)
    fn pk_hash(pk: &[u8]) -> [u8; 32];
}
```

- Native implementation of `FalconCrypto::verify` calls `fn_dsa_vrfy` (or `pqcrypto_falcon::falcon512::verify_detached_signature`). **Important:** sign over `blake3(scale_encoded_payload)` (32 bytes), not the raw payload — keeps host-call payloads small and matches the account-id hash function.
- Implementing `sp_runtime::traits::Verify` for `ArgonautSignature`:
  - `FalconWithKey`: check `falcon_crypto::pk_hash(&pk.0) == *signer` AND `falcon_crypto::verify(...)`. Additionally, as a side effect it does NOT write storage (Verify is stateless) — registration happens via the pallet extrinsic or a `TransactionExtension` (below).
  - `Falcon`: cannot resolve pk inside `Verify` (no storage access). Therefore **registry-mode signature checking is done in a `TransactionExtension`** (polkadot-sdk's successor to `SignedExtension`), which has storage access, not in `Verify`. `Verify` for this variant returns false; the extension bypasses the default check via an unsigned-with-extension or `General` transaction flavor. (Alternative simpler design if extension plumbing fights you: keep ALL transactions in `FalconWithKey` form on the wire but have the tx-pool/runtime strip-and-dedupe — do NOT do this; the extension design is the correct one. Note this decision explicitly for the implementer.)
- Register the host functions in the node executor: add `falcon_crypto::HostFunctions` to the `ExtendHostFunctions` tuple in `node/src/service.rs` executor construction.

### 2.3 `pallets/falcon-registry`

Storage:
```rust
#[pallet::storage]
pub type Keys<T: Config> = StorageMap<_, Blake2_128Concat, T::AccountId, FalconPublic, OptionQuery>;
```

Extrinsics:
```rust
/// Register the caller's public key. Origin must be signed in bootstrap mode
/// (i.e., the pk was already proven by the enclosing signature check).
pub fn register_key(origin, pk: FalconPublic) -> DispatchResult {
    let who = ensure_signed(origin)?;
    ensure!(falcon_crypto::pk_hash(&pk.0) == who.as_ref(), Error::<T>::KeyMismatch);
    ensure!(!Keys::<T>::contains_key(&who), Error::<T>::AlreadyRegistered);
    Keys::<T>::insert(&who, pk);
    Self::deposit_event(Event::KeyRegistered { who });
    Ok(())
}
```

Also provide: `Config` with a storage-deposit constant (charge ~900 bytes worth of deposit to discourage state spam; refundable on `deregister` if you add one), benchmarking, and a `KeyProvider` trait (`fn key_of(who) -> Option<FalconPublic>`) consumed by the TransactionExtension.

TransactionExtension `CheckFalconSignature` (runtime crate):
- Pipeline: decode `ArgonautSignature` → if `Falcon{sig}`, load pk via `KeyProvider`, reject with `InvalidTransaction::BadSigner` if absent → `falcon_crypto::verify(sig, blake3(signed_payload), pk)` → on `FalconWithKey`, verify inline and (optionally, config-gated) auto-register the key so the first transaction doubles as registration.
- Weight: charge extra weight for one storage read (registry mode) or ~900 bytes proof size (bootstrap mode).

### 2.4 Tooling

- `keygen` subcommand in the node CLI: generate Falcon keypair from a 32-byte seed (use the crate's seeded keygen), print SS58 address (`blake3(pk)` with your network prefix), write keystore file `{address}.falcon.json` (encrypted with a passphrase; scrypt + xsalsa/chacha via `sp-core` utilities is fine).
- A tiny `argonaut-sign` offline signer binary (payload in → SCALE `ArgonautSignature` out) so integration tests and scripts don't need a wallet.
- Substrate's standard keystore/session-key machinery does NOT know Falcon — that's fine because Falcon keys here are user transaction keys, not consensus session keys. Do not attempt `KeyTypeId` integration.

### 2.5 Tests & acceptance

1. Unit: sign/verify round-trip; corrupted sig/pk/msg each fail; `pk_hash` matches independent blake3.
2. Runtime-level: bootstrap tx succeeds and (if auto-register on) key appears in storage; second tx in registry mode succeeds with ~897 fewer bytes; registry-mode tx from unregistered account rejected in the pool (not in a block).
3. Weight/size assertion test: encoded registry-mode extrinsic ≤ 850 bytes for a balance transfer.
4. wasm build unaffected (host fn interface compiles to no_std).

**Dependencies:** none on B or C. Ship this first.

---

## 3. Implementation B — Prism Consensus

### 3.1 Architecture decision (binding)

Prism runs **entirely client-side**. The Substrate chain becomes a deterministic *output* of Prism:

```
                    ┌──────────────────────────── node process ────────────────────────────┐
 user txs ──────►  tx-pool ──► prism-miner ──► [tx blocks, proposer blocks, m voter blocks]
                                    │                       │ gossip (prism-network)
                                    ▼                       ▼
                              prism-db  ◄────────────  peers' blocks
                                    │
                                    ▼
                              prism-chain (DAG state: longest chains, votes, leaders)
                                    │  confirmed leader levels
                                    ▼
                              prism-ledger: BuildLedger → ordered, sanitized tx list
                                    │
                                    ▼
                    Substrate BlockBuilder → import with instant finality → runtime (STF)
```

Consequences the implementer must respect:
- One Substrate block per confirmed leader level (or per batch of levels). Its extrinsics are the deduplicated transactions from all transaction blocks referenced (directly or transitively) by that leader. Because Prism confirmation is ε-final, this chain **never reorgs**: import with `finalized: true`, `fork_choice: Custom(always best)`. Delete GRANDPA at this milestone.
- Transactions that fail runtime execution (e.g., double-spends that survived structural dedupe, bad nonces) are **skipped, not fatal** — Prism's paper sanitizes double spends at ledger build; account-model conflicts surface at execution. Use `apply_extrinsic` results to drop failures and continue. Determinism requirement: every honest node must derive the identical Substrate block from the same leader sequence — so the skip rule must be purely a function of (ordered tx list, prior state).
- Prism blocks carry opaque SCALE-encoded `UncheckedExtrinsic` bytes; the Prism layer never interprets them.

### 3.2 `primitives/prism` — types, sortition, constants (no_std, SCALE)

```rust
pub type H256 = sp_core::H256;
pub type ChainId = u16;              // voter chain index, 0..m
pub const M_VOTER_CHAINS: u16 = ...; // config, see 3.7

#[derive(Encode, Decode, TypeInfo, Clone, PartialEq, Eq, Debug)]
pub struct SuperHeader {
    pub parents_root: H256,   // merkle root over m+2 parent hashes (order: voter 0..m-1, tx, proposer)
    pub content_root: H256,   // merkle root over m+2 content hashes (same order)
    pub timestamp_ms: u64,
    pub nonce: u64,
    pub difficulty: U256,     // combined difficulty; sortition splits it
}

#[derive(Encode, Decode, TypeInfo, Clone, Debug)]
pub enum BlockContent {
    Transaction { txs: Vec<Vec<u8>> },                                 // opaque extrinsics
    Proposer    { tx_refs: Vec<H256>, prop_refs: Vec<H256> },
    Voter       { chain: ChainId, voter_parent: H256, votes: Vec<H256> }, // votes = proposer hashes, one per unvoted level, ascending
}

#[derive(Encode, Decode, TypeInfo, Clone, Debug)]
pub struct PrismBlock {
    pub header: SuperHeader,
    pub parent: H256,              // this block's own parent (proposer parent for tx/proposer, voter-chain parent for voter)
    pub content: BlockContent,
    pub parent_proof: MerkleProof, // proves `parent` at index i under parents_root
    pub content_proof: MerkleProof,// proves hash(content) at index i under content_root
}

pub fn block_hash(h: &SuperHeader) -> H256 { blake3(scale(h)) }

/// Sortition: map PoW hash into block type. Ranges follow paper Alg.1 lines 41-47.
pub enum Sortition { Voter(ChainId), Transaction, Proposer, Fail }
pub fn sortition(hash: &H256, params: &Params) -> Sortition {
    // interpret hash as U256 h; thresholds:
    // h < m*f_v                      → Voter(h / f_v)
    // m*f_v <= h < m*f_v+f_t         → Transaction
    // m*f_v+f_t <= h < m*f_v+f_t+f_p → Proposer
    // else                           → Fail (not a valid block)
}
```

Notes:
- Merkle trees: binary blake3 trees over the m+2 leaves; implement in this crate (≈80 lines) — do not pull a heavy dependency. Proof = Vec<(H256, bool)>.
- Genesis: one proposer genesis + m voter geneses, all with fixed constants, hashed deterministically from `(chain_spec_genesis_hash, chain_id)` so every network has distinct Prism geneses.
- Validation function `validate_block(&PrismBlock, params) -> Result<Validated, Error>`: PoW check (`block_hash <= difficulty`), sortition consistency (claimed content type matches sortition range of hash), Merkle proofs verify, content-specific sanity (votes strictly ascending by level, ≤ some max; refs deduped; tx block ≤ B_t bytes).

### 3.3 `client/prism-db`

RocksDB wrapper with the column families already named in the repo (keep verbatim, they mirror prism-rust):
`PROPOSER_NODE_LEVEL`, `VOTER_NODE_LEVEL`, `VOTER_NODE_CHAIN`, `VOTER_TREE_LEVEL_COUNT`, `PROPOSER_TREE_LEVEL`, `VOTER_NODE_VOTED_LEVEL`, `PROPOSER_NODE_VOTE`, `PROPOSER_LEADER_SEQUENCE`, `PROPOSER_LEDGER_ORDER`, `PROPOSER_VOTE_COUNT`, `GRAPH_PARENT_NEIGHBOR`, `GRAPH_VOTE_NEIGHBOR`, `GRAPH_VOTER_PARENT_NEIGHBOR`, `GRAPH_TRANSACTION_REF_NEIGHBOR`, `GRAPH_PROPOSER_REF_NEIGHBOR`, plus `BLOCKS` (hash → SCALE PrismBlock) and `BLOCK_STATUS` (hash → {Pending, Valid, Orphan}).

API surface: typed getters/setters per CF; `WriteBatch`-based atomic multi-CF commit for "insert block + update indices" (crash consistency matters — a block must never be half-indexed).

### 3.4 `client/prism-chain` — the DAG state machine (port of prism-rust `blockchain/`)

Owns in-memory + persisted consensus state:

```rust
pub struct PrismChain {
    proposer_best_level: u64,
    voter_tips: Vec<(H256, u64)>,          // per-chain (tip hash, depth)
    unreferenced_tx_blocks: HashSet<H256>, // proposer content candidates
    unreferenced_proposers: HashSet<H256>,
    unvoted_levels: Vec<BTreeMap<u64, H256>>, // per voter chain: level → first-seen proposer
    votes: HashMap<H256, VoteInfo>,        // proposer → {count, per-chain vote depth}
    orphans: OrphanPool,                   // blocks waiting on missing refs
}

impl PrismChain {
    /// Main entry. Idempotent. Returns network requests (missing refs) and
    /// whether new leader levels may be confirmable.
    pub fn insert_block(&mut self, b: ValidatedBlock, db: &PrismDb) -> InsertOutcome;
}
```

Insertion logic (mirror paper §3.1 "ReceiveBlock" + prism-rust):
- **Tx block:** remove its txs from the local mempool view; add to `unreferenced_tx_blocks`.
- **Voter block (chain i):** append to tree i; if it extends the longest chain, advance tip and apply its votes; if it creates a longer fork, perform a **vote rollback**: unwind votes of the abandoned segment, apply votes of the new main-chain segment (this is the trickiest code in the whole port — steal prism-rust's approach and test it exhaustively).
- **Proposer block at level ℓ:** index by level; if ℓ = best+1, advance proposer parent and mark level ℓ unvoted on all chains; remove its refs from unreferenced sets; if parent level missing → orphan + request.
- Vote accounting: a chain's vote for level ℓ is the proposer hash referenced by the main-chain voter block that voted ℓ; depth = distance from tip. Maintain `PROPOSER_NODE_VOTE` and `PROPOSER_VOTE_COUNT` incrementally.

### 3.5 `client/prism-miner`

```rust
loop {
    refresh_contents_if_dirty();   // m+2 content buffers:
      // tx content     = up to B_t bytes of pool txs (see queue-coloring note below)
      // proposer       = current unreferenced_tx_blocks + unreferenced_proposers
      // voter[i]       = unvoted_levels[i] first-seen proposer per level, ascending
    let header = SuperHeader { parents_root, content_root, timestamp, nonce, difficulty };
    for nonce_batch { if block_hash(&h) <= difficulty { 
        match sortition(hash) { … assemble PrismBlock with only its own parent/content/proofs … }
        broadcast(block); mark dirty; break;
    }}
}
```

- Content refresh triggers: new pool tx, any accepted block. Rebuild Merkle trees only when a leaf changed.
- Throughput dedupe (paper §4.4 "queue coloring"): partition pool txs into q queues by `blake3(tx) % q` and mine tx-block content from ONE randomly chosen queue per template. q = 8 is fine initially.
- Difficulty: single combined target; sortition thresholds implement the f_v : f_t : f_p split. Retarget like Bitcoin (rolling window over superblock timestamps) — one shared retarget, thresholds are fixed *fractions* of the target so the split survives retargeting.
- Run mining in dedicated threads (`std::thread`, not async), template swapped via `ArcSwap`.

### 3.6 `client/prism-network`

Two protocols on `sc-network`:
- Notifications `/argonaut/prism/blocks/1`: push SCALE `PrismBlock` announcements (send full block; they are small except tx blocks — if tx blocks exceed notification size comfort, announce hash + serve via req/resp).
- Request/response `/argonaut/prism/sync/1`: `GetBlocks(Vec<H256>) → Vec<PrismBlock>` for orphan resolution and initial sync. Initial sync strategy: request peers' voter tips + proposer best level, then breadth-first backfill by parent refs. (Naive but correct; optimize later.)
- On receive: `validate_block` → `PrismChain::insert_block` → forward `InsertOutcome.missing` into sync requests; re-gossip valid blocks (standard flood with seen-cache).

### 3.7 Parameters (initial testnet values)

Per the paper §4.2 with a deliberately small start:
- `m = 100` voter chains (paper simulates 1000; 100 keeps early devnets debuggable — make it a chain-spec constant, plan a network restart to raise it).
- Round/delay target `D ≈ 1 s`; per-tree mining rate `f_v = 0.1 blocks/s` (1 block/10 s per tree, matching the paper's simulation); `f_p = f_v`.
- `B_t` (tx block size): 32 KB to start. `B_v`, `B_p`: reference-only, naturally < 2 KB.
- Confirmation `ε = e^-20` equivalent depth parameters; expose `--prism-epsilon` for testing.

### 3.8 `client/prism-ledger` — confirmation + bridge

Stage 1 (correctness-first) confirmation rule: **conservative k-deep** — leader at level ℓ is the proposer with most votes once every counted vote is ≥ k deep on its voter chain (paper §4.1's conservative policy). Simple, obviously safe, latency ~Bitcoin — acceptable for the first devnet.

Stage 2: **Def. 4.5 confidence intervals** (fast confirmation): implement `V_lower_n(r)`, `V_upper_n(r)`, private-block upper bound, confirm proposer *set* Π_ℓ when `max_n V_lower_n > V_upper_private`; leader confirmed uniquely when one block's lower bound beats all others' uppers. Port constants (δ_d table) from paper §4.5.1; keep them in one `confirmation.rs` with the formulas cited in comments.

`BuildLedger` (paper Alg. 2): for confirmed leader sequence p_1..p_ℓ, expand each leader's direct+indirect tx-block refs in reference order, concatenate, dedupe txs by hash, emit ordered list. Persist `PROPOSER_LEDGER_ORDER`.

Bridge task (async, owns a `sc_client_api` handle):
```
on new confirmed leader level ℓ:
    txs = build_ledger_delta(ℓ)                 // only txs not in ledger before ℓ
    let mut builder = client.new_block_at(parent = current tip)?;
    for tx { let _ = builder.push(tx); /* skip individual failures */ }
    let block = builder.build()?;
    import_block(BlockImportParams{ finalized: true, .. })
    embed ℓ and leader hash in the header digest (DigestItem::Other) for auditability
```

### 3.9 Milestones & acceptance for B

- **B0 (optional but recommended):** swap Aura→`sc-consensus-pow` with a blake3 `PowAlgorithm` on the plain single chain. Teaches the import-queue plumbing with 300 lines at stake. Acceptance: two-node devnet mines and syncs.
- **B1:** primitives + db + chain state machine, fully unit-tested offline (no networking): feed scripted block sequences incl. voter-chain reorgs with vote rollback; property test: vote counts always equal recount-from-scratch.
- **B2:** miner + network on a single node (self-mining loopback), then 4-node docker-compose devnet: all node DBs converge (identical leader sequence) after churn.
- **B3:** ledger stage 1 + Substrate bridge; Aura/GRANDPA removed. Acceptance: balance transfer submitted → appears in a Substrate block whose digest names a leader level; all nodes' Substrate chains byte-identical.
- **B4:** stage-2 fast confirmation + adversarial simulation harness (censorship + balancing attackers as in paper §5) run in CI at small scale.
- **B5:** pruning: voter blocks > k_prune behind their chain tip AND below the last confirmed level are deletable; tx-block bodies referenced only by pruned history archived/dropped by config.

**Known hard spots to warn the implementer about:** vote rollback on voter-chain reorg; orphan/dependency resolution deadlocks (always re-check orphan pool after every insert); determinism of the bridge skip-rule; making `insert_block` atomic w.r.t. crash (WriteBatch).

---

## 4. Implementation C — STARK Aggregation Layer (optional, builds on A + B)

### 4.1 Design summary

A second signature path alongside Falcon: users sign with **XMSS over Poseidon2** (many-time hash-based scheme, `hash-sig` crate); an **aggregator service** batches signatures and produces one STARK proof per batch using **leanVM/leanMultisig**; transaction blocks reference proven **bundles**; the runtime verifies one proof per bundle via a host function instead of per-tx signatures. Net effect (from our analysis): ~150 B of signature material per tx amortized vs ~800 B registry-mode Falcon; ~1 s proving per ~1,000 sigs on laptop-class hardware; verify ~5 ms per bundle.

Everything upstream is research-grade: **isolate behind traits, pin git revisions, expect to bump.**

### 4.2 `primitives/agg`

```rust
#[derive(Encode, Decode, TypeInfo, Clone, Debug)]
pub struct AggBundle {
    pub scheme: AggSchemeId,          // versioned: (vm id, hash id, param set) — hash agility hook
    pub txs: Vec<Vec<u8>>,            // opaque extrinsics, canonical order = proof order
    pub signer_keys: Vec<XmssRootRef>,// 32-byte key refs (registry) or inline roots (bootstrap)
    pub epochs: Vec<u32>,             // XMSS leaf index used by each signer (state!)
    pub proof: Vec<u8>,               // opaque leanVM proof blob
}

#[runtime_interface]
pub trait AggVerify {
    /// Verify `proof` attests: for all i, XMSS_verify(pk_root_i, epoch_i, blake3(txs_i)) == true.
    /// Pure; native-only; deterministic across nodes (same pinned verifier build).
    fn verify_bundle(scheme: u8, statement_hash: &[u8; 32], proof: &[u8]) -> bool;
}
```

- `statement_hash = blake3(scale((scheme, txs_root, signer_keys_root, epochs_root)))`; the same statement construction must be implemented in the aggregator and in the circuit's public inputs. Write ONE shared function in this crate used by both sides.
- **Determinism warning for the implementer:** the runtime host fn must be a *verifier* only (fast, deterministic). All nodes must run the same pinned verifier revision — treat a verifier bump like a runtime upgrade (gate on-chain via `AggSchemeId` allow-list in the pallet).

### 4.3 `client/aggregator` (also a standalone binary)

- Ingest: RPC endpoint `agg_submit(tx_bytes, xmss_signature, pk_root, epoch)`; also usable as a sidecar service by third parties (the scheme is non-interactive — anyone with the signatures can aggregate).
- Batching policy: close a batch at `min(N_max = 1024, T_max = 750 ms)`.
- Prover driver: `trait AggregationBackend { fn prove(batch) -> Proof }` with the leanMultisig implementation behind a feature flag; run in a worker pool; on prover failure, fall back to submitting txs individually via Falcon path if the wallet provided a fallback signature (optional).
- Output: submit `AggBundle` into the node's tx-pool as ONE special extrinsic (`pallet-agg-verify::submit_bundle(bundle)`) — this reuses the entire Prism tx-block pipeline with zero changes: to Prism, a bundle is just a big opaque transaction.
- Economic note in code comments: bundle fee = base + per-inner-tx fee; aggregator collects inner-tx tips — this is what pays for prover hardware.

### 4.4 `pallets/agg-verify`

- `submit_bundle(origin: none/unsigned-with-validation OR signed-by-aggregator, bundle: AggBundle)`:
  1. `ValidateUnsigned`/extension: recompute `statement_hash`, call `agg_verify::verify_bundle`, reject invalid before it ever enters a block (pool-time check — critical for DoS).
  2. Dispatch: for each inner tx, decode as `RuntimeCall` + inner-signer context and dispatch with `origin = Signed(account_of(pk_root_i))`; skip (event, don't abort) on individual failure — same determinism rule as the Prism bridge.
  3. **Epoch replay protection:** storage `XmssEpochs: Map<XmssRootRef, BitfieldChunk>`; reject any (key, epoch) already used. This is the on-chain half of XMSS statefulness.
- Key registry: reuse the falcon-registry pattern — `XmssKeys: Map<AccountId32, XmssRoot>` with `AccountId32 = blake3(xmss_root)`, registered once. Consider merging both registries into one `pallet-key-registry` with an enum key type.
- Weights: per-bundle base (proof verify ≈ 5 ms → benchmark it) + per-inner-tx dispatch weight.

### 4.5 Wallet statefulness (do not skip)

XMSS leaf reuse is catastrophic (signature forgery). Client-side requirements:
- Keystore stores `(seed, next_epoch)`; increment-and-fsync BEFORE releasing a signature; refuse to sign if state file is missing/older than on-chain `XmssEpochs` high-water mark (query at startup).
- Document loudly: restoring a wallet from an old backup requires an on-chain epoch scan first.

### 4.6 Milestones & acceptance for C

- **C1:** vendored leanMultisig proves+verifies a 16-signature batch in a plain integration test (no chain). Pin revision; record proving time and proof size in the test output.
- **C2:** `primitives/agg` statement construction identical between aggregator and verifier (round-trip test); host fn wired; `pallet-agg-verify` verifies a real proof in a runtime test.
- **C3:** end-to-end on the B3 devnet: 100 XMSS-signed transfers → one bundle → one Prism tx block → confirmed → all 100 balances updated; measure bytes-on-wire per tx and assert < 300 B.
- **C4:** epoch replay attack test (same leaf twice → second rejected at pool); aggregator crash-recovery test.

---

## 5. Integrated Transaction Lifecycle (all three systems)

```
      Falcon path (default)                         Aggregated path (bulk)
──────────────────────────────              ─────────────────────────────────────
wallet signs (Falcon-512)                    wallet signs (XMSS/Poseidon2, epoch n)
   │ ~800 B (registry) / 1.7 KB (bootstrap)     │ tx + 3.1 KB sig  →  aggregator RPC
   ▼                                             ▼
node tx-pool  ◄──────────────────────────  aggregator batches ≤1024, proves (~1 s)
   │  pool-time check: TransactionExtension      │ AggBundle (~100 KB / 1024 tx)
   │  (registry lookup + host-fn verify)         │ pool-time check: proof verify host fn
   └───────────────┬─────────────────────────────┘
                   ▼
        prism-miner tx-block content ──► PoW+sortition ──► gossip
                   ▼
        prism-chain: proposer refs it; m voter chains vote; leader confirmed (ε-final, ~seconds)
                   ▼
        prism-ledger: BuildLedger → ordered txs → Substrate block (instant finality)
                   ▼
        runtime STF: balances / registry / agg-verify pallets execute
```

## 6. Build Order (respect the dependency arrows)

1. **Phase 0** — polkadot-sdk migration (everything depends on it).
2. **A** — Falcon + registry (independent; gives the chain a real signature scheme immediately, under Aura).
3. **B0→B3** — Prism to the point of replacing Aura/GRANDPA.
4. **C1→C4** — aggregation layer (needs A's registry pattern and B's running chain).
5. **B4→B5** — fast confirmation + pruning (can proceed in parallel with C).

## 7. Risk Register

| Risk | Severity | Mitigation baked into this design |
|---|---|---|
| polkadot-sdk API churn between releases | M | Pin one stable release for the whole project; upgrade only at phase boundaries |
| FIPS 206 final differs from current Falcon crates | L/M | **Not a one-file fix (corrected 2026-07-16, amendment first offered 2026-07-13):** the Falcon verify host function is native and version-less, and a full-history resync re-executes every historical block, so the pre-final verifier must be kept **forever** as a legacy host fn. Migration mechanics: keep the 0.2-line verifier as `falcon_crypto::verify` (legacy), add `verify_v2` for the final-standard scheme, add a new `ArgonautSignature` variant, and run a dual-scheme window — plus a wallet key-rotation UX, since `AccountId = blake3(pk)` means a re-serialized key is a *new* account, not an in-place upgrade. Gate: don't move real value before FIPS 206 final + fn-dsa 1.0 + external audit (also recorded in `docs/IMPLEMENTATION_E_SPEC.md` E.0 rule 9 and `docs/FRONTEND_DAPP_SPEC.md` §10). |
| Vote-rollback bugs in prism-chain | H | Property tests (recount invariant), fuzzed reorg sequences, port prism-rust logic rather than re-derive |
| Non-deterministic Substrate bridge (nodes build different blocks from same leaders) | H | Skip-rule specified as pure function; cross-node byte-identity assertion in B3 CI |
| leanVM/hash-sig API churn or soundness bug | H (for C) | Trait-isolated backend, pinned revs, `AggSchemeId` on-chain allow-list enables emergency disable via governance/sudo; Falcon path always remains as fallback |
| Poseidon2 cryptanalysis progress | M | `AggSchemeId` versioning = hash agility; keys can be re-registered under a new scheme |
| XMSS wallet state loss → leaf reuse | H (user-level) | On-chain epoch bitfield rejects reuse; wallet fsync-before-sign; documented restore procedure |
| State bloat from key registries | M | Storage deposits on registration; single merged registry pallet |
| Prism DB corruption on crash | M | Atomic WriteBatch per insert; startup consistency scan (recount spot-check) |

## 8. Handoff Prompts for the Implementing Model

Feed these one at a time, each with this document attached:

- **P0:** "Migrate this repo to the latest stable polkadot-sdk solochain template per §1. Delete `runtime/src/Consensus` and `node/src/Mining`. Deliver: green build, Aura devnet, CI. Do not implement any Prism/Falcon logic yet."
- **P-A:** "Implement §2 exactly: `primitives/falcon` with the `ArgonautSignature` enum and `#[runtime_interface]` host functions using the `fn-dsa` crate, `pallets/falcon-registry`, and the `CheckFalconSignature` TransactionExtension. Include all tests in §2.5. Where §2.2 flags the registry-mode Verify limitation, use the TransactionExtension design, not the strip-and-dedupe alternative."
- **P-B1:** "Implement §3.2–§3.4 (`primitives/prism`, `client/prism-db`, `client/prism-chain`) with the unit/property tests in B1. Use github.com/yangl1996/prism-rust `blockchain/` module as the logic reference for vote accounting and rollback; port, don't depend."
- **P-B2:** "Implement §3.5–§3.6 miner and network; deliver the 4-node convergence test."
- **P-B3:** "Implement §3.8 stage-1 confirmation, BuildLedger, and the Substrate bridge; remove Aura/GRANDPA; deliver byte-identical-chains CI test."
- **P-C:** "Implement §4 with leanMultisig pinned at revision <fill in>, behind `trait AggregationBackend`. Deliver C1–C4 acceptance tests."

Rules for the implementing model, restate in every prompt: SCALE everywhere; no serde in consensus types; native crypto behind host functions only; every phase lands with its acceptance tests; when a referenced upstream API has drifted from this document, keep this document's *interface* and adapt the internals, noting the deviation in the crate README.

---
*Prepared June 2026. Sources: Prism paper (arXiv:1810.08092), prism-rust reference implementation, ASIA CCS '22 STARK aggregation paper, leanEthereum leanMultisig/hash-sig repositories, polkadot-sdk documentation, NIST FIPS 206 IPD status.*

---

## 9. Implementation D — Decentralized Storage Platform (Iroh data plane, on-chain control plane)

> **Implementers: read `docs/IMPLEMENTATION_D_SPEC.md` instead.** This section
> is the product/architecture rationale; the spec expands it into exact storage
> layouts, extrinsic check-orders, escrow/vesting formulas, epoch/beacon
> mechanics, and per-milestone tests — and **supersedes this section where they
> conflict** (deviations listed in its §D.13, notably: the §9.5 beacon is
> replaced with an epoch-snapshotted block hash because the runtime cannot read
> the bridge's leader post-digest).

### 9.0 Feasibility & library inventory (verified June 2026; iroh/iroh-blobs/bao-tree rows re-verified July 2026)

| Need | Library | Status / Notes |
|---|---|---|
| P2P transport, hole-punching, discovery | `iroh` (n0-computer) | **Stable 1.0 released 2026-06-15** (1.0.2 as of 2026-07-06) with a wire-protocol stability commitment; running in production on millions of devices. Pin one version (`=1.0.x`). |
| Content-addressed blob transfer, BLAKE3 verified streaming | `iroh-blobs` | **Two lines exist:** 0.35 (older API, designated production-quality, tied to pre-1.0 iroh) and 0.10x (new API — 0.103.0 builds against stable `iroh ^1.0`, but the maintainers still label it "not yet production quality"). **Decision: build against the 0.10x line, pinned** — its provider-events API (per-request intercept/allow by hash and node id) is exactly what the paid-retrieval gate needs, and it is the only line that rides on stable iroh 1.0; accept the API-churn risk with a thin wrapper layer, mitigated by pinning + integration tests. Re-check the production-quality label at each phase boundary — it is expected to graduate. |
| Verified slice proofs (the on-chain proof primitive) | `bao-tree` (^0.16, dependency of iroh-blobs; 0.16.0 is latest — earlier "^1.x" note here was wrong) | Stable, standalone. Encodes/verifies byte-range slices of a blob against its BLAKE3 root. Outboards are stored at 16 KiB chunk-group granularity, but **slice verification works at 1024-byte chunk granularity against the same root** — the root hash is granularity-independent. This is what keeps on-chain proofs ~2.5 KB. |
| BLAKE3 with subtree hashing | `blake3` ≥ 1.8.0 | Subtree functionality upstreamed (the old `iroh-blake3` fork is deprecated). Already an Argonaut dependency. |
| Erasure coding | `reed-solomon-simd` (preferred; O(n log n), SIMD) or `reed-solomon-erasure` | Both stable on crates.io. |
| Everything on-chain | pallets per §2/§4 patterns; Prism leader hashes as randomness beacon | Already specified in this document. |

**Feasibility verdict: fully buildable today.** No missing primitives. The only research-flavored piece — proof-of-physical-replication — is deliberately designed *out* (§9.1). Risk concentrates in iroh-blobs 0.10x API churn (wrapper-isolate it; core iroh is now stable 1.0) and in economic-parameter tuning (testnet problem, not a code problem).

### 9.1 Binding design decisions

1. **Chain = control plane only.** File bytes never enter extrinsics, Prism blocks, or runtime storage. On-chain: hashes, deals, stakes, challenges, proofs, escrow, channels.
2. **No Proof-of-Replication.** Deduplication-resistance comes from Reed-Solomon: each provider stores a *distinct* shard, so there is no common replica to dedupe. We incentivize *availability of distinct shards*, not physical uniqueness of copies. Document this as the product definition; do not let the implementer wander into PoRep.
3. **One hash tree everywhere.** A shard's on-chain commitment IS its iroh-blobs content ID (BLAKE3 root). The same root serves: transfer verification (iroh), challenge proofs (bao slices), and retrieval payment verification (bao streaming). Never introduce a second commitment scheme.
4. **Proof verification is a native host function** over `bao-tree` (same pattern as Falcon/agg host fns). Deterministic, ~microseconds.
5. **Provider = three cohabiting components:** iroh endpoint + blob store, Argonaut node (or light client + RPC to a full node), challenge-responder daemon.

### 9.2 `primitives/storage`

```rust
pub type BlobRoot = [u8; 32];          // BLAKE3 root == iroh content id
pub type DealId = u64;
pub type EpochIndex = u32;

#[derive(Encode, Decode, TypeInfo, Clone, Debug)]
pub struct ShardSpec { pub root: BlobRoot, pub size: u64 }

#[derive(Encode, Decode, TypeInfo, Clone, Debug)]
pub struct DealParams {
    pub manifest: BlobRoot,        // iroh HashSeq blob: [shard roots..] + encoding params
    pub shards: BoundedVec<ShardSpec, ConstU32<64>>,
    pub k: u8, pub n: u8,          // RS parameters, e.g. 10-of-16; ensure k < n <= shards.len()
    pub duration_epochs: EpochIndex,
    pub price_per_shard_epoch: Balance,
    pub challenge_rate: u16,       // expected challenges per shard per day, see 9.5
}

#[runtime_interface]
pub trait BaoProof {
    /// Verify a bao slice: `slice` is the bao-encoded bytes for chunk range
    /// [chunk_start, chunk_start+chunk_len) of the blob with the given root
    /// and total size. Chunk = 1024 bytes (bao leaf granularity).
    /// Native impl: bao_tree slice decode/validate. Returns false on any mismatch.
    fn verify_slice(root: &[u8; 32], total_size: u64,
                    chunk_start: u64, chunk_len: u32, slice: &[u8]) -> bool;
}
```

Implementation notes for the host fn: use `bao_tree`'s decode-response / validate APIs with `BlockSize::ZERO` (chunk-level) semantics for verification regardless of the provider's stored outboard granularity; enforce `slice.len() <= MAX_PROOF_BYTES` (const, ≈ 4 KiB for single-chunk challenges: ~1 KiB data + depth×64 B parents; 1 GiB shard → depth ≈ 20 → ≈ 2.3 KiB total) *before* calling into the host to bound weight.

### 9.3 `pallets/provider-registry`

Storage: `Providers: Map<AccountId, ProviderInfo { stake, iroh_node_id: [u8;32], relay_hint: BoundedVec<u8>, capacity_bytes, price_per_shard_epoch, faults: u32, status }>`; `TotalStake`.
Extrinsics: `register(stake ≥ MinStake, iroh_node_id, terms)`, `update_terms`, `deregister` (only when no active shard assignments; unbonding period constant), `top_up_stake`.
Trait exported to other pallets: `Slashable { fn slash(who, amount, beneficiary) }`, `fn is_active(who) -> bool`.
Slashing routes: proofs pallet (missed/invalid proofs), deals pallet (abandonment). Sizing rule of thumb encoded as a constant: stake ≥ 2× total escrow a provider may hold across active deals (checked at shard acceptance).

### 9.4 `pallets/storage-deals` — deal lifecycle

State machine (store as enum on the deal):
```
Listed ──(providers accept shards)──► Assigning ──(all n shards activated)──► Active
   │ timeout, refund                      │ per-shard timeout → re-list shard
   ▼                                      ▼
Cancelled                        Active ──► Expired (duration reached; residual escrow → client)
                                     │
                                     └──► Degraded (≥1 shard faulted) ──repair (9.8)──► Active
                                              │ shards_alive < k
                                              ▼
                                          Lost (full escrow refund from remaining slashes; terminal)
```

Storage: `Deals: Map<DealId, Deal { client, params, state, created_epoch }>`; `ShardAssignments: DoubleMap<DealId, shard_idx, Assignment { provider, activated_epoch, last_proven_epoch, escrow_claimed }>`; `NextDealId`.
Extrinsics:
- `create_deal(params, escrow = n × duration × price)` — reserves escrow from client. Client must be serving all shards over iroh at this point.
- `accept_shard(deal_id, shard_idx)` — provider (active, sufficiently staked) claims a shard; at most `MaxShardsPerProviderPerDeal = 1` to preserve the erasure-coding independence assumption (constant; document why).
- `activate_shard(deal_id, shard_idx, proof)` — provider proves possession by answering an activation challenge (chunk index derived from `blake3(deal_id ‖ shard_idx ‖ acceptance_block_leader_hash)`); verified via `bao_proof::verify_slice`. Starts escrow vesting for this shard.
- `claim_earnings(deal_id, shard_idx)` — pays vested escrow up to `last_proven_epoch` minus already claimed.
- `terminate(deal_id)` by client — pro-rata refund of unvested escrow; small cancellation fee to providers.

### 9.5 `pallets/storage-proofs` — the availability heartbeat

**Randomness beacon:** `beacon(e) = leader_block_hash(confirmed_level_at_epoch_start(e) − K_DELAY)` with `K_DELAY = 10` levels, exposed by the Prism bridge as a runtime digest/storage item written when it builds each Substrate block (§3.8 — add `LeaderHashByLevel` to the bridge's digest). Grinding note for the implementer: an adversarial Prism miner influencing a leader hash forfeits/gambles PoW rewards worth far more than any single challenge evasion; acceptable for v1, revisit if deal sizes grow (mitigation path: drand-style external beacon or VDF, out of scope).

**Challenge selection (deterministic, no on-chain writes needed):** shard (d, i) is challenged in epoch e iff
`u64::from_le(blake3(beacon(e) ‖ d ‖ i)[..8]) % SELECT_MOD < challenge_rate_factor` — tune so each shard expects `challenge_rate` challenges/day. If selected, the challenged chunk index is `blake3(beacon(e) ‖ d ‖ i ‖ "chunk") % num_chunks(shard)`.
Providers compute this locally; anyone can compute it to check compliance.

Extrinsics:
- `submit_proof(deal_id, shard_idx, epoch, slice_bytes)` — checks: shard was selected in `epoch`; within deadline (`epoch` still current or previous); `verify_slice(root, size, challenged_chunk, 1, slice)`. On success: update `last_proven_epoch`, emit event. Weight: benchmarked host-fn call + O(1) storage.
- `report_missed(deal_id, shard_idx, epoch)` — permissionless after deadline; verifies selection arithmetic on-chain, then: increment provider fault counter, slash `MissedProofSlash` (split: reporter bounty + deal escrow top-up), mark shard `Faulted` after `MaxConsecutiveMisses` (→ deals pallet `Degraded` transition + shard re-listed).
Anti-spam: `report_missed` requires a small bond, returned iff the report is valid.

**Proof volume + aggregation hook (v2):** at 100k active shards × 2 challenges/day ≈ 2.3 proof-tx/sec (~2.5 KB each) — trivially within Prism capacity (§ TPS analysis). When volume grows: `submit_proof_bundle(bundle)` variant that verifies ONE STARK attesting many `verify_slice` executions, reusing §4's `AggVerify` pattern with a BLAKE3 AIR (BLAKE3/Blake2s AIRs exist in the Stwo/Plonky3 ecosystem — this is why we did NOT need Poseidon for the data layer). Design the storage-proofs pallet with a `ProofPayload::{Single(slice), Aggregated(bundle_ref)}` enum from day one so v2 is additive.

### 9.6 `pallets/payment-channels` — retrieval market

Unidirectional client→provider channels; vouchers off-chain, settlement on-chain.
Storage: `Channels: Map<ChannelId, Channel { from, to, deposit, claimed, expiry, status }>`.
Extrinsics: `open(to, deposit, ttl)`; `settle(channel_id, voucher)` where `voucher = (channel_id, cumulative_amount) + ArgonautSignature(from)` — verify via the §2 host fn, pay `cumulative − claimed`, callable multiple times with increasing cumulative; `close` by provider anytime (settles current voucher) or by client after `expiry` (refunds remainder); no challenge game needed because vouchers are strictly monotone and only the provider can present them.
Off-chain retrieval loop (documented in provider daemon + SDK): provider streams bao-verified 16 KiB chunk groups over iroh-blobs; client verifies each incrementally against the manifest root and returns a new voucher per `VOUCHER_STRIDE` bytes (e.g. 1 MiB); provider halts the QUIC stream if a voucher is late. Max mutual exposure = one stride. Falcon voucher signatures (666 B) are fine: they live off-chain; only the final voucher hits the chain.

### 9.7 `client/storage-provider` (daemon binary)

Components (single tokio process):
- **Iroh endpoint + blobs store** (file-backed store; pin iroh-blobs version; wrap ALL iroh-blobs API calls in an internal `trait BlobStore`/`trait Transfer` layer ≤ 500 lines so version bumps touch one module).
- **Serving gate:** use iroh-blobs provider events (request intercept) to allow: (a) shard fetches from the deal's client or repair agents, (b) retrieval requests only on connections tied to an open, funded channel (map iroh NodeId → channel via a hello handshake protocol `/argonaut/retrieval/1`).
- **Chain watcher:** subscribes to deal events (assignments won, challenges due) via node RPC.
- **Challenge responder:** each epoch, compute selections for all held shards (9.5 formula), read the challenged chunk + build bao slice from local store/outboard, submit `submit_proof` extrinsic. Submit early in the epoch window; retry with fee bump. **Alerting hook** (webhook/log) on any failure — a silent responder failure is how providers lose stake.
- **Voucher manager:** per-channel monotone counter, persisted with fsync before releasing bytes (mirror of the XMSS epoch discipline in §4.5).

### 9.8 Client SDK / CLI (`argonaut-store` crate + subcommands)

- `encode <file>`: RS-encode (k-of-n from config) → n shard files → per-shard BLAKE3 roots → manifest HashSeq blob (shard roots + `DealParams` fields) → prints manifest root.
- `deal create`: serve shards from a temporary local iroh node; submit `create_deal`; wait for assignments; hand off each shard to its provider (iroh push or serve-until-fetched); confirm activations; then local copies may be deleted.
- `fetch <manifest_root>`: read manifest (from any provider or public gateway), open channels to ≥ k providers, ranged-fetch shards in parallel with per-chunk verification + vouchers, RS-decode, verify reassembled file hashes match manifest, output file.
- `repair <deal_id>` (also runnable as an incentivized keeper daemon): on `Degraded`, fetch any k live shards, reconstruct the faulted shard (RS), re-serve it, call `accept_shard`+`activate_shard` as (or on behalf of) a replacement provider. Repair bounty = slashed stake portion earmarked by the deals pallet.

### 9.9 Milestones & acceptance for D

- **D1 (no chain):** `primitives/storage` host fn round-trip — encode a 64 MiB blob with bao-tree, extract slice for random chunk, `verify_slice` passes; mutate 1 byte anywhere → fails. RS encode/decode round-trip incl. reconstruction from k random shards.
- **D2:** registry + deals pallets on the Aura or Prism devnet; full deal lifecycle in an integration test with mocked proofs: create → accept ×n → activate → claim → expire; every state transition event-asserted.
- **D3:** proofs pallet + provider daemon: 3 provider daemons + 1 client on the B3 devnet; run 50 epochs (compressed time); assert: all selected challenges answered, earnings claimable, killing one daemon → `report_missed` → slash → `Degraded`.
- **D4:** retrieval channels end-to-end: client fetches a 100 MiB file from 10-of-16 shards with per-stride vouchers; assert byte-identity and that provider revenue == voucher totals; adversarial test: client stops vouchering → provider halts within one stride.
- **D5:** repair flow: fault a provider, keeper reconstructs + re-activates the shard, deal returns to `Active`; bounty paid.

### 9.10 Risk register additions

| Risk | Severity | Mitigation |
|---|---|---|
| iroh-blobs pre-1.0 API churn (core iroh is stable 1.0 since 2026-06) | M | Pin exact versions (`iroh =1.0.2`, `iroh-blobs =0.103.0`); all iroh calls behind internal traits (§9.7); upgrade only at milestone boundaries |
| Outsourced storage (provider keeps shard on S3) | Accepted | Product sells *availability*, not physical locality (§9.1 decision 2); tight proof deadlines make cold storage uneconomic anyway |
| Beacon grinding by Prism miners | L→M as deal value grows | Documented v1 acceptance + upgrade path (external beacon/VDF) |
| Client disappears before shard handoff completes | L | `Assigning` timeouts re-list shards; escrow refund path |
| Provider loses voucher/counter state | M (provider-level) | fsync-before-serve discipline; on-chain `claimed` is the recovery floor |
| Illegal content exposure for providers | Policy | Providers see only shard hashes of RS-coded fragments; per-provider deny-list hook in the serving gate; explicit ToS decision left to operator |
| Economic parameter mis-tuning (stake ratios, slash sizes, challenge rate) | M | All constants in one `parameters.rs` per pallet, governance/sudo-adjustable on testnet |

### 9.11 Handoff prompts (append to §8)

- **P-D1:** "Implement §9.2 `primitives/storage` (types + `BaoProof` host function over the `bao-tree` crate) and an `argonaut-store encode` CLI using `reed-solomon-simd`, with the D1 acceptance tests. Enforce MAX_PROOF_BYTES before host dispatch."
- **P-D2:** "Implement §9.3 `pallets/provider-registry` and §9.4 `pallets/storage-deals` exactly as specified, including the deal state machine enum, escrow vesting math, and D2 tests. Constants in `parameters.rs`."
- **P-D3:** "Implement §9.5 `pallets/storage-proofs` (deterministic selection formula verbatim, `report_missed` with bond, `ProofPayload` enum with `Aggregated` variant stubbed) and the §9.7 provider daemon with iroh-blobs pinned at =0.103.0 (iroh =1.0.2), all iroh calls behind internal traits. Deliver D3."
- **P-D4:** "Implement §9.6 `pallets/payment-channels` + SDK `fetch` with per-stride vouchers over iroh-blobs verified streaming, and the §9.8 `repair` flow. Deliver D4–D5."

**Build-order amendment to §6:** D1–D2 depend only on Phase 0 + A (they run fine under Aura); D3 onward wants B3 (needs the leader-hash beacon — for pre-B3 development, stub the beacon with the Substrate block hash and swap at B3). D is independent of C, but C's `AggVerify` pattern is the template for D's v2 proof bundles.

## 10. Implementation E — Production Bootstrap (0→1): Pallets & Utilities

> **Implementers: read `docs/IMPLEMENTATION_E_SPEC.md`.** E covers everything
> needed to take the chain from the current devnet runtime to a production
> mainnet: genesis/token identity and chain-spec hardening (E0), benchmarked
> weights and a real fee curve (E1), governance to retire sudo (E2), account
> utilities (E3), ink! smart contracts (E4), node/ops/release engineering
> (E5), and the Falcon signing/ecosystem tooling gap (E6). E is orthogonal to D and respects the same binding rule:
> Prism decides order, Substrate decides effect — no session/staking/
> authorship/finality pallets, ever.

## 11. Application layer — dApp catalog

> **See `docs/DAPP_CATALOG_SPEC.md`.** Which dApps Argonaut is uniquely good
> at (scored against its three differentiators: post-quantum signatures,
> no-reorg Prism finality, native verifiable storage with paid retrieval) and
> how each is built as ink! contracts + off-chain daemons on the D/E
> primitives. Tier 1: Anchor (PQ notarization), Keystone (key/cert
> transparency), Press (paid content — the D-stack flagship). Adds no runtime
> pallets; apps live in a workspace-excluded `apps/` directory. Includes an
> anti-catalog (oracle DeFi, bridges, randomness-critical apps — blocked on
> PQ/beacon grounds, not taste).
