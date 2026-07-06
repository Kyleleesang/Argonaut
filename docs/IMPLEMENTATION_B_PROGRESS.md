# Implementation B Progress Tracker — Prism Consensus

Tracks status against [`ARGONAUT_ARCHITECTURE_GUIDE.md`](./ARGONAUT_ARCHITECTURE_GUIDE.md) §3 ("Implementation B — Prism Consensus") and its milestones in §3.9. Follows [`IMPLEMENTATION_A_PROGRESS.md`](./IMPLEMENTATION_A_PROGRESS.md) per the doc's §6 build order. Lives on the `Base` branch.

## Status: B0 and B1 complete and tested. B2 partially done: single-node self-mining loopback works and is tested; real sc-network wiring and multi-node convergence are not started.

| §3.9 milestone | Status |
|---|---|
| **B0** (optional): Aura → `sc-consensus-pow` swap, blake3 `PowAlgorithm`, two-node devnet mines and syncs | ✅ done this session (see below) |
| **B1**: primitives + db + chain state machine, fully unit-tested offline; property test vote counts == recount-from-scratch | ✅ done this session (see below) |
| **B2**: miner + network, single-node self-mining loopback, then 4-node docker-compose convergence | 🟡 partial this session — self-mining loopback (miner → network validation → chain insertion, in-process) done and tested; real `sc-network` wire protocols and the 4-node docker-compose test are not started (see below) |
| B3: ledger stage 1 + Substrate bridge; Aura/GRANDPA removed | ⬜ not started |
| B4: stage-2 fast confirmation + adversarial simulation | ⬜ not started |
| B5: pruning | ⬜ not started |

## B1 — what changed

Before this session, `client/prism-chain`'s own doc comment stated plainly: *"The stub below handles the happy path only. Full rollback logic must be ported from prism-rust `blockchain/mod.rs` before the B1 acceptance tests can pass."* Concretely, the Phase-0-scaffolded version had these bugs/gaps (confirmed by reading it, not assumed):
- Voter-block insertion blindly did `new_depth = tip.1 + 1` for every incoming block on a chain — no check that the block's `voter_parent` was actually the current tip, no fork detection, no rollback. Two competing voter blocks on the same chain would have silently corrupted depth tracking.
- `proposer_best_level` was set once in `new()` and never updated anywhere else — proposer levels weren't tracked at all.
- `unvoted_levels` was initialized empty and never populated when new proposer levels appeared, so there was no way to know what a chain still needed to vote on.
- Voter blocks with an unknown `voter_parent` weren't orphaned (unlike proposer blocks, which already had working orphan handling) — they'd hit the same blind-increment bug instead.
- `VoteInfo.per_chain` existed but was never written to.

### The fix
Rewrote `client/prism-chain/src/lib.rs` around two lightweight in-memory metadata maps (`voter_meta: HashMap<H256, VoterMeta{parent, depth, votes}>`, `proposer_meta: HashMap<H256, ProposerMeta{level}>`) that make chain traversal fast without needing DB round-trips:
- **Proposer levels**: `level(block) = level(parent) + 1`, computed from `proposer_meta`. The first block ever seen at a new deepest level advances `proposer_best_level` and marks that level "unvoted" on every voter chain (`unvoted_levels[chain]`); sibling proposers at an already-seen level don't reset that marker (tested explicitly — `sibling_proposers_at_same_level_do_not_reset_unvoted_marker`).
- **Voter fork-choice + rollback**: each new voter block's depth is computed from its parent; if that depth exceeds the chain's current tip depth, the chain reorgs. Reorg walks both the old and new tip back to their lowest common ancestor (LCA) — a standard longest-chain reorg (same shape as Bitcoin-style chain reorgs) — then unwinds every vote cast on the abandoned segment and (re-)applies every vote cast on the newly-adopted segment, in the correct order. `unvoted_levels` is kept in sync symmetrically (a rolled-back vote's level becomes unvoted again on that chain).
- **Orphaning**: voter blocks with an unknown parent are now orphaned exactly like proposer blocks with unknown parent/refs, and `drain_orphans` retries both kinds whenever new blocks arrive.
- **`db: Arc<PrismDb>` became `db: Option<Arc<PrismDb>>`**, with two constructors: `PrismChain::new(params)` (pure in-memory, used by all of `PrismChain`'s own tests — no disk I/O) and `PrismChain::with_db(params, db)` (also persists full block bodies as they're accepted, for production wiring). Checked before making this change that no other crate calls `PrismChain::new()` directly (`client/prism-network` only takes `&mut PrismChain` as a parameter, never constructs one), so this was a safe, non-breaking internal redesign.
- `VoteInfo` was trimmed to just `{ total: u64 }` — the unused `per_chain` field was dropped rather than left half-implemented; per-chain vote depth (needed for B3/B4's k-deep and confidence-interval confirmation rules) is deferred to whichever milestone actually consumes it.

### Tests (9 new, `client/prism-chain/src/tests.rs`)
All operate purely in memory (no RocksDB), constructing `PrismBlock`s directly rather than mining them — `insert_block` doesn't itself re-validate PoW/sortition/Merkle proofs (that's `validate_block`'s job, called upstream in `prism-network` before blocks ever reach `PrismChain`), so tests exercise the state machine directly:
- Proposer level advancement + unvoted-marking, including the sibling-at-same-level edge case.
- Tx-block reference bookkeeping (`unreferenced_tx_blocks`).
- Proposer and voter orphan resolution (both previously proposer-only).
- Linear voter-chain growth matches recount.
- **The central scenario**: a competing, longer voter-chain fork correctly rolls back the abandoned segment's votes and applies the new segment's, including `unvoted_levels` symmetry, with exact agreement against an independent from-scratch recount.
- Reorg on one voter chain leaves other chains' vote counts untouched.
- A longer scripted sequence (build → shorter competing fork arrives, must not preempt → extend it past the original, forcing a reorg → extend further → yet another, longer-still fork from genesis) that asserts the recount invariant after **every single insertion**, not just at the end — this is the literal "property test: vote counts always equal recount-from-scratch" the doc's B1 acceptance criterion asks for.

### `client/prism-db` also gained tests (6 new, previously had zero)
Using a real `tempfile`-backed RocksDB instance per test (no mocking): block insert/get round-trip, status defaults to `Valid` on insert, status can be overridden, `u64`/`H256` generic column round-trips, and — notably — a **restart test**: write via one `PrismDb` handle, drop it, re-open the same on-disk path with a fresh handle, confirm the data survived. This is the closest thing to a crash-consistency check available without actually killing the process mid-write.

### `primitives/prism`
Unchanged this session — it already had 4 passing tests from Phase 0 and matched the doc's §3.2 spec closely (SCALE types, blake3 Merkle tree, sortition, `validate_block`). No bugs found here.

## Verification

- `cargo test -p argonaut-prism-chain`: 9/9 passing.
- `cargo test -p argonaut-prism-db`: 6/6 passing.
- `cargo test -p argonaut-primitives-prism`: 4/4 passing (unchanged).
- `cargo check --workspace`: clean (verified `PrismChain`'s constructor signature change didn't break `client/prism-network`, the only other crate referencing the type).
- `cargo test --workspace`: green, 49 tests total (4 prism primitives + 9 prism-chain + 6 prism-db + 10 falcon primitives + 11 falcon registry + 7 runtime + 2 agg primitives), 0 failures.

## Explicitly out of scope for B1 (per the doc's own boundaries)

- **Persistence of the DAG indices** (`PROPOSER_NODE_LEVEL`, `VOTER_NODE_CHAIN`, `GRAPH_*` columns, etc.) — `client/prism-db` declares all these column families (§3.3), but only `BLOCKS`/`BLOCK_STATUS` are actually written to; `PrismChain`'s fast traversal metadata (`voter_meta`/`proposer_meta`) lives only in memory. B1's acceptance criteria ask for "fully unit-tested offline" scripted-sequence testing, not crash-recovery of the DAG state itself — that's closer to B5 (pruning) territory, where the on-disk indices actually start mattering. Flagging so it isn't mistaken for done.
- **Networking** — deliberately out of scope per B1's own description ("no networking"); that's B2.
- **Distinct per-chain genesis hashes** (doc §3.2: "hashed deterministically from `(chain_spec_genesis_hash, chain_id)`") — `primitives/prism::genesis_hash()` exists but `PrismChain::new()` still seeds every voter chain and the proposer tree from a single shared `H256::zero()` sentinel. Fine for offline state-machine testing (which is all B1 asks for); wiring real chain-spec-derived genesis hashes is a B2/network-integration concern.

## B0 — what changed

Swapped Aura+GRANDPA for `sc_consensus_pow` with a minimal blake3 `PowAlgorithm` (`node/src/pow.rs`), as a self-contained rehearsal for the import-queue/mining-worker plumbing before it's needed for real in B3 (where this whole module gets replaced by the Prism-ledger bridge — nothing here is meant to survive past that point, per its own module doc comment).

### `runtime/`
- Removed `pallet_aura`/`pallet_grandpa` from `Cargo.toml`, `construct_runtime!`, and their `Config` impls; removed the `AuraApi`/`GrandpaApi` runtime API impls.
- `pallet_timestamp::Config::OnTimestampSet` changed from `Aura` to `()` (nothing to notify — PoW authors client-side).
- **Kept an empty `SessionKeys` runtime API** (`impl_opaque_keys! { pub struct SessionKeys {} }` + the `sp_session::SessionKeys` impl) even though nothing in this design uses session keys — discovered the hard way that `sc_service::spawn_tasks` (used unconditionally by every node, regardless of consensus mechanism) has a structural `RuntimeApiImpl: sp_session::SessionKeys<Block>` bound. Removing it entirely doesn't compile; this is apparently a fact of life for any polkadot-sdk `stable2412` runtime, not something specific to Aura/GRANDPA.

### `node/src/pow.rs` (new)
- `Blake3Pow`: `PowAlgorithm<Block>` with `Difficulty = U256` (which already implements the required `TotalDifficulty` trait in `sp_consensus_pow`, no custom type needed). `verify()` decodes the seal as a little-endian `u64` nonce, hashes `blake3(pre_hash ++ nonce)`, and checks it against difficulty as a big-endian `U256` — the same shape `argonaut_primitives_prism::sortition`/`block_hash` already use, for consistency.
- **Fixed difficulty, no retargeting at all** — deliberately out of scope for this rehearsal milestone (real retargeting is specified for the actual Prism miner in §3.5, not this throwaway chain). See "Known limitation" below.

### `node/src/service.rs`
- Replaced the Aura import queue + GRANDPA block import/voter/warp-sync wiring with `PowBlockImport` + `sc_consensus_pow::import_queue` + `sc_consensus_pow::start_mining_worker`.
- The actual CPU-bound nonce search runs on a **plain `std::thread`**, not through `task_manager`'s `spawn_blocking`. Reason: `MiningHandle::submit` (in `sc-consensus-pow` itself) holds a `parking_lot` mutex guard across an `.await` internally, so the future it returns isn't `Send` — `spawn_blocking` requires `Future + Send` and rejects it. `futures::executor::block_on` sidesteps this: it only requires the *thread* to block, not the future to be `Send`.
- **Bug caught during manual testing, not code review**: the first version of the mining loop reset its nonce counter to `0` at the top of every outer-loop iteration. With difficulty giving roughly a 1-in-2^20 chance per hash and only 200,000 attempts per batch (~17% success chance per batch), any batch that failed would retry the *exact same* nonce range forever, permanently stalling. Fixed by moving the counter outside the loop so it keeps climbing across iterations/targets.
- No warp sync (it piggybacks on GRANDPA's authority-set proofs, which don't exist here); full sync only, fine for a devnet.

### `node/src/chain_spec.rs` / `command.rs`
- Genesis no longer seeds Aura/GRANDPA authority sets (nothing to seed).
- `Revert` subcommand's GRANDPA-specific `aux_revert` hook removed (no finality gadget to revert).

## Known limitation, flagged for follow-up (per explicit request): difficulty does not scale with network hash rate

The mining target (`node/src/pow.rs::fixed_difficulty`) is a **hardcoded constant**, empirically tuned only against this one dev machine's blake3 throughput to land around single-digit-second blocks. It was initially copied from `argonaut_primitives_prism::Params::testnet()`'s own difficulty shift for consistency, but that value turned out calibrated for a completely different (much larger, competitive network) assumption — at that setting, this single CPU mined sub-second blocks and regularly outran the block-timestamp inherent's tolerance. Retuned empirically (tried three values live against a running node) to `U256::max_value() >> 24`.

This has no feedback loop: if more miners join the network (or run faster hardware), blocks will simply come out faster than intended indefinitely — there's nothing pulling difficulty back up to compensate, unlike Bitcoin-style rolling-window retargeting. Before this consensus is used for anything beyond a single-machine/local-devnet rehearsal, it needs either a `DifficultyApi`-backed on-chain adjustment or a client-side rolling-window retarget against recent block timestamps. Explicitly not implemented here since B0's own scope (§3.9) frames it as throwaway rehearsal code for the import-queue plumbing, with real retargeting logic specified for the actual Prism miner (§3.5) instead — but noting it prominently since a fixed difficulty is a real footgun if anyone tries to run this multi-node setup unattended or at scale.

## Verified (B0)

- `cargo check --workspace` / `cargo test --workspace`: clean throughout (49 tests, 0 failures — consensus swap doesn't touch anything the existing test suite exercises).
- `cargo build --release -p argonaut-node`: clean.
- **Single-node mining**: `--dev` node authors and imports blocks steadily under PoW (no Aura at all in the binary).
- **Two-node sync (the literal acceptance criterion)**: started node1 with a fixed `--node-key` for a deterministic peer ID, let it mine to block #15, started node2 with `--bootnodes` pointing at node1. Node2 connected (`1 peers` in both logs), synced from #0 to #20 within seconds, and both nodes converged on byte-identical best-block hashes at every height checked, continuing to co-mine (each occasionally wins a block, the other imports it) through block #26 before the test was stopped.
- Occasional `"timestamp of the block is too far in the future"` import failures observed on both single- and two-node runs — non-fatal (mining simply retries and succeeds shortly after); a symptom of block times not being perfectly tuned against `pallet_timestamp`'s tolerances, not a correctness bug. Left as-is given B0's rehearsal scope.

## B2 — what changed (partial: single-node loopback only)

Scoped down from the full milestone this session: implemented and tested the "single-node self-mining loopback" half of B2's acceptance description (miner produces a block → network layer validates and inserts it), but did **not** implement real `sc-network` wire transport or the 4-node docker-compose convergence test — see "Explicitly not done" below.

### `client/prism-miner` — wired to live chain state
Before this session, `PrismMiner` mined against a single **static, empty** template built once in `new()` (all-zero parents, empty content) — `refresh()` existed but nothing ever called it with real content, so the miner could run forever without ever reflecting actual chain/mempool state.

- Added `build_template(chain, mempool, queue, q, params) -> MinerTemplate`, a pure function assembling all m+2 content slots from live `PrismChain` state per architecture guide §3.5: voter slots from `unvoted_levels[i]` (ascending), tx slot from a queue-colored mempool slice (§4.4), proposer slot from `unreferenced_tx_blocks`/`unreferenced_proposers`. Parent selection: voter slots use their own chain's tip; tx and proposer slots both use the "proposer parent" (the deepest known unreferenced proposer, matching the guide's "this block's own parent (proposer parent for tx/proposer, voter-chain parent for voter)").
- Added two small public accessors on `PrismChain` to support this: `genesis_sentinel()` (made `pub`, was a private free function) and `proposer_level(&self, hash) -> Option<u64>` (used to pick the *deepest* unreferenced proposer as parent).
- Refactored the nonce search into a standalone, synchronous, testable `try_mine(tmpl, params, start_nonce, max_attempts) -> (Option<PrismBlock>, next_nonce)`. `spawn_mining_thread` is now a thin loop calling it. Deliberately designed to hand back `next_nonce` and require the caller to carry it forward -- avoiding the exact reset-to-zero bug B0's mining loop had (see above); this design rules that bug class out structurally rather than relying on remembering to fix it again.
- **Real bug found via the self-mining loopback test, not by inspection**: `primitives/prism::validate_block`'s "votes strictly ascending" check compares the proposer *hashes* in a Voter block's `votes` list directly (`w[0] >= w[1]` on `H256` values) -- it has no DAG access to resolve a hash back to the on-chain level it corresponds to, so hash-value order is the only thing it can structurally check. But `PrismChain::unvoted_levels[i]` is a `BTreeMap<level, hash>`, which naturally iterates in ascending *level* order -- and level order has no relationship to hash-value order (hashes are effectively random). Any Voter block that ended up voting on 2+ pending levels at once would eventually fail this check purely by chance (confirmed: the loopback test failed at round 12 of a 40-round run once enough levels had piled up). Fixed by sorting `votes` by hash value in `build_template` before handing it to the miner -- satisfies the structural check `primitives/prism` actually enforces, while `PrismChain::insert_block`'s own hash-keyed vote accounting (which never depended on wire order) stays correct regardless.
- 7 new tests (`client/prism-miner/src/tests.rs`): template parent/content correctness against a live chain, queue-coloring correctness, mined blocks pass `validate_block`, and the central **40-round self-mining loopback** (mine → insert directly into the same chain → check `recount_votes() == vote_totals()` after every round).

### `client/prism-network` — tested against real mined blocks for the first time
`handle_announcement` (validate → dedupe via seen-cache → `chain.insert_block` → decide what to gossip/fetch) already existed from Phase 0 and looked reasonable, but had **zero tests** and had never actually been exercised against a real mined block -- only ever called conceptually. Added 5 tests (`client/prism-network/src/tests.rs`):
- A valid mined block is accepted and marked for gossip.
- A duplicate announcement (same hash) is dropped via the seen-cache, second time through.
- A structurally invalid block (corrupted Merkle proof) is rejected and never reaches `chain.insert_block` at all.
- A hash-only announcement (no body) is a no-op, as designed (caller is expected to issue a sync request instead).
- **The full B2 loopback, through the network layer this time**: 40 rounds of mine → announce → validate → insert, asserting every round is accepted, gossiped, and never missing a ref (since it's mining from its own live state, it can never reference something it doesn't already know about), and that the chain invariant holds throughout.

## Explicitly not done for B2 (this is the bulk of the remaining milestone)

- **Real `sc-network` wire transport.** `client/prism-network`'s `Cargo.toml` has no dependency on `sc-network` at all, and nothing registers the two protocols named in its own doc comments (`/argonaut/prism/blocks/1` notifications, `/argonaut/prism/sync/1` request/response) with an actual libp2p/litep2p swarm. `handle_announcement` is real, tested logic for "what do I do with a message I already have" -- there is no code yet for "how do messages get from one node's process to another's." This is a substantial, self-contained task on the order of what B0 did for GRANDPA's protocol registration (before it was removed) or bigger.
- **Wiring into `node/src/service.rs`.** Nothing in the running node currently spawns a `PrismMiner` thread or registers `prism-network`'s protocols. The Substrate chain the node actually runs (B0's PoW chain) and the Prism DAG logic this session tested are not connected to each other or to a live node process at all yet -- everything B1/B2 has verified so far runs inside `cargo test`, not inside `argonaut-node`.
- **4-node docker-compose convergence test.** Not attempted. Requires the above two items first (there's no way for 4 processes to converge on "an identical leader sequence" without real networking to exchange blocks over).
- **Persistent per-chain genesis hashes** (still the single shared `H256::zero()` sentinel from B1) -- becomes actually important once multiple independent networks/chain-specs need to coexist, which multi-node testing would surface.

## Next: finish B2

The remaining work is exactly the "Explicitly not done" list above, in roughly this order: (1) register the two real `sc-network` protocols and implement actual gossip flood + sync request/response over them; (2) wire a `PrismMiner` thread and the network protocol handlers into `node/src/service.rs` so a running `argonaut-node` actually produces and gossips Prism blocks (today it only runs the unrelated B0 PoW chain); (3) a multi-node convergence test -- docker-compose as the doc suggests, or a manual multi-process test on one machine (as B0's two-node sync test did) if that proves more practical here.
