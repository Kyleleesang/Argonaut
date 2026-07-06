# Implementation B Progress Tracker — Prism Consensus

Tracks status against [`ARGONAUT_ARCHITECTURE_GUIDE.md`](./ARGONAUT_ARCHITECTURE_GUIDE.md) §3 ("Implementation B — Prism Consensus") and its milestones in §3.9. Follows [`IMPLEMENTATION_A_PROGRESS.md`](./IMPLEMENTATION_A_PROGRESS.md) per the doc's §6 build order. Lives on the `Base` branch.

## Status: B1 complete and tested. B0 not started.

| §3.9 milestone | Status |
|---|---|
| **B0** (optional): Aura → `sc-consensus-pow` swap, blake3 `PowAlgorithm`, two-node devnet mines and syncs | ⬜ not started |
| **B1**: primitives + db + chain state machine, fully unit-tested offline; property test vote counts == recount-from-scratch | ✅ done this session (see below) |
| B2: miner + network, single-node self-mining, 4-node convergence | ⬜ not started (depends on B0/B1) |
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

## Next: B0

Not started. Per §3.9, B0 swaps Aura for `sc-consensus-pow` with a blake3 `PowAlgorithm` on the existing single (non-Prism) chain, as a self-contained rehearsal for the import-queue/mining-worker plumbing before it's needed for real in B3 (where Aura/GRANDPA get removed permanently in favor of the Prism-ledger bridge). Explicitly labeled optional in the doc; its own output is transitional and gets superseded at B3, not reused. Scope: replace `pallet_aura`/`pallet_grandpa` wiring in `runtime/` and `node/src/service.rs` with `sc_consensus_pow`, a minimal fixed-difficulty blake3 `PowAlgorithm` impl, and a mining worker; update the chain spec to drop Aura/Grandpa authority seeding; acceptance is a two-node devnet that mines and syncs.
