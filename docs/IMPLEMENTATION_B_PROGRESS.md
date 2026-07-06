# Implementation B Progress Tracker — Prism Consensus

Tracks status against [`ARGONAUT_ARCHITECTURE_GUIDE.md`](./ARGONAUT_ARCHITECTURE_GUIDE.md) §3 ("Implementation B — Prism Consensus") and its milestones in §3.9. Follows [`IMPLEMENTATION_A_PROGRESS.md`](./IMPLEMENTATION_A_PROGRESS.md) per the doc's §6 build order. Lives on the `Base` branch.

## Status: B0, B1, B2 complete and tested. B3 not started.

| §3.9 milestone | Status |
|---|---|
| **B0** (optional): Aura → `sc-consensus-pow` swap, blake3 `PowAlgorithm`, two-node devnet mines and syncs | ✅ done this session (see below) |
| **B1**: primitives + db + chain state machine, fully unit-tested offline; property test vote counts == recount-from-scratch | ✅ done this session (see below) |
| **B2**: miner + network, single-node self-mining loopback, then 4-node docker-compose convergence | ✅ done this session (see below) — 4-node convergence verified via a manual multi-process devnet with a real kill/restart churn cycle rather than docker-compose itself (see "Explicitly not done" for the distinction) |
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

## B2 — what changed

Done across two passes this session: first the single-node self-mining loopback (miner → network validation → chain insertion, all in-process), then real `sc-network` wire transport plus wiring both into `argonaut-node` itself, verified live on an actual two-node devnet. Only the 4-node docker-compose convergence test remains — see "Explicitly not done" below.

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

### `client/prism-network/src/wire.rs` (new) — real `sc-network` transport

Registers the two protocols named in `lib.rs`'s own doc comments, which until now were never actually wired to a swarm:

- **`blocks_protocol_config`/`sync_protocol_config`**: generic over `N: NetworkBackend<B, H>` (the same pattern `sc_consensus_grandpa::grandpa_peers_set_config` and `sc_network_transactions::TransactionsHandlerPrototype::new` use), so they work with either network backend the node can be started with (`NetworkWorker`/libp2p or `Litep2pNetworkBackend`) — confirmed by reading both concrete `NetworkBackend` impls in the vendored `stable2412` source, not guessed from older Substrate docs, since this API has churned across polkadot-sdk versions.
- **`PrismNetworkWorker`**: an async task owning the `NotificationService` (blocks gossip), the `IncomingRequest` receiver (sync protocol answering), a channel of self-mined blocks from `PrismMiner`, and a `NetworkService` handle for issuing outbound sync requests. Its `process()` method calls the exact same `handle_announcement` that `client/prism-network`'s existing tests already covered — self-mined and network-received blocks share one code path. On `to_fetch` (missing refs), it requests the missing hashes from whichever peer announced the block and recursively re-processes what comes back.
- Answering sync requests reads from `PrismDb::get_block` -- only ever finds something for blocks that were actually `insert_block`-accepted (not orphans), which is the correct scope for this protocol (orphan bodies are already held in-memory by the requester, not something a *peer* needs to serve back).

**Two real, non-obvious problems found only by actually running two nodes, not by reading the API docs:**

1. **Notification protocol peer slots.** Following GRANDPA/transactions' exact `SetConfig { in_peers: 0, out_peers: 0, non_reserved_mode: Deny }` pattern (their own doc comments describe it as "ride on whatever peers the default sync protocol already connected") produced a protocol that never opened a single substream between two nodes that were nonetheless connected and syncing the main chain fine. GRANDPA/transactions apparently keep their zero-slot protocol's peer set in sync with the default set by explicitly calling `NetworkPeers::add_peers_to_reserved_set` as sync-protocol peers connect/disconnect -- reproducing that mirroring exactly was out of scope for this milestone, so `blocks_protocol_config` instead gives the protocol its own independent, generously-sized peer slot budget (`in_peers`/`out_peers`: 25, `Accept` mode). This manages its own connections against the same peers already known via the peerstore, and reliably opened a substream in live testing (confirmed via added `substream opened with <peer>` logging).
2. **Peer disconnects from gossip flood.** With `Params::testnet()` (`m=100` voter chains) at its own calibrated difficulty, a single CPU produced dozens of Prism blocks per second — every one gossiped as a separate notification to the one connected peer. In live two-node testing this reliably tripped some reputation/rate limit within 1-2 seconds and dropped the peer connection entirely (both the Prism substream *and* the underlying connection, confirmed via peer count dropping to 0 and never recovering). Fixed empirically by scaling the node's Prism difficulty down ~100x from `Params::testnet()` (`node/src/service.rs`, not the shared `testnet()` constant itself, which other tests still rely on) — with the flood reduced to roughly one block every several seconds, the connection stayed stable and gossip continued for the remainder of the test run.

### `node/src/service.rs` — Prism wired into the actual running node

Previously nothing in `argonaut-node` touched Prism at all -- everything B1/B2 had verified ran inside `cargo test`, never inside the binary. Now, alongside the unrelated B0 PoW chain (separate DB, separate consensus, the two run side by side and don't yet interact -- that's B3's job):
- Opens a `PrismDb` at `<base-path>/prism` and constructs `PrismChain::with_db(..)` behind an `Arc<Mutex<_>>`.
- Registers both Prism protocol configs on `net_config` *before* `sc_service::build_network` consumes it by value (this ordering is load-bearing -- confirmed against `substrate/client/service/src/builder.rs`'s own internal sequencing for its built-in protocols).
- Spawns `PrismNetworkWorker::run()` as a task.
- If the node is an authority: spawns a `PrismMiner` mining thread (mined blocks flow into the worker over a channel) plus a small periodic thread that rebuilds the mining template from current chain state every 500ms (there's no mempool wired up yet, so only the parent/vote/reference slots ever actually change between refreshes -- transaction-pool integration is out of scope here).

### Verified live (not just `cargo test`)

Built the release binary and ran an actual two-node devnet (same fixed-`--node-key` + `--bootnodes` technique as B0's own two-node test), with `--log prism=info` added logging for accepted-block source and substream open/close events:
- Confirmed **real cross-node gossip**, not just coincidental parallel mining: node1's self-mined block hash `0x000009d6da1c...` appeared in node2's log tagged `source: <node1's peer ID>` at the same timestamp node1 logged it as `self-mined` -- and symmetrically in the other direction. Exact hash equality between two independently-mined blocks is only possible if one was actually transmitted, not coincidence.
- After the difficulty fix above, the peer connection (both the main sync protocol and the Prism substream) stayed up for the remainder of a 40+ second run, with gossip continuing to flow throughout.
- `cargo test --workspace`: still green, 61 tests, 0 failures.

## B2 — finishing the 4-node convergence test

### `PrismChain::leader_at_level` (new, small addition)

The doc's B2 acceptance criterion talks about nodes converging on an "identical leader sequence" — but nothing in `PrismChain` could answer "who's the leader at level ℓ" at all yet (B1 built vote totals and DAG traversal, not a leader query). Added `leader_at_level(&self, level) -> Option<H256>`: the proposer at `level` with the most votes, tie-broken by hash for determinism. This is explicitly a **raw vote-count snapshot**, not the stage-1 "conservative k-deep" *confirmed* leader the doc specifies for B3 §3.8 (that needs per-vote depth tracking that doesn't exist yet — `VoteInfo` was trimmed to a flat `total: u64` in B1 and deliberately deferred this). Good enough to make "convergence" a checkable, deterministic property for B2's own test: since `PrismChain::insert_block` already guarantees vote totals converge to the same values regardless of arrival order (B1's core invariant, re-proven every session since), two nodes that have accepted the identical *set* of blocks must produce identical `leader_at_level` results for every level, by construction. 3 new tests (`client/prism-chain/src/tests.rs`): returns `None` before any proposer exists, picks the higher-vote proposer, and tie-breaks deterministically by hash when votes are equal.

### Live 4-node devnet with a real churn cycle

Built the release binary and ran 4 `argonaut-node` processes (fixed `--node-key`s, mesh-bootnoded to each other) — all 4 fully connected (3 peers each) on both the main sync protocol and the Prism gossip protocol, mining and gossiping Prism blocks concurrently.

**Churn**: killed node3 mid-run (`kill`, not `--dev`'s ephemeral state — its `PrismDb`/chain data directory was preserved, modeling a realistic restart-after-downtime rather than a fresh cold join), let the remaining 3 nodes continue mining and gossiping without it, then restarted node3 pointed at the same data directory and bootnodes.

**Result**: node3 reconnected on both protocols (peer count recovered to 3, Prism substreams reopened with all three peers, confirmed via the `substream opened with <peer>` logging added earlier this session), and bidirectional gossip resumed correctly: a block node3 mined *after* reconnecting appeared in node1's log tagged with node3's exact peer ID at the identical timestamp node3 logged it as self-mined, and symmetrically a block node1 mined after node3's reconnect appeared in node3's log tagged with node1's peer ID. This is the same "exact hash equality proves transmission, not coincidence" argument used for the two-node test, now demonstrated surviving an actual node restart.

**Honest scope of what this proves**: it demonstrates gossip resilience across a real disconnect/reconnect cycle — a node dropping out and coming back correctly rejoins the mesh and exchanges blocks bidirectionally from that point forward. It does **not** demonstrate a node recovering the *specific* blocks mined by others strictly during its downtime, because there's no historical backfill/initial-sync counterpart for Prism data (`PrismNetworkWorker`'s sync protocol only serves reactive orphan-fetch requests — "I have a block that references something I don't have" — not "catch me up on everything I missed"). A node whose downtime is longer than however long its peers keep relevant state reachable via orphan-chasing would end up with a genuinely divergent (though internally self-consistent) DAG. This is a real, scoped-out gap, not a bug; see below.

## Explicitly not done for B2

- **Full historical backfill / initial sync for Prism state.** As described just above — the only catch-up mechanism is reactive orphan-fetching triggered by an incoming block referencing something unknown, not a real "sync from genesis" or "sync from last-known-height" flow the way the main Substrate chain already has (via `sc-network-sync`). A node offline for any nontrivial period, or one joining fresh with an empty `PrismDb`, will not converge with the rest of the network on its own.
- **Docker-compose specifically.** The convergence test above used 4 plain OS processes on one machine rather than actual container orchestration — functionally exercises the same real-networking code path (this is genuine loopback-interface TCP/libp2p traffic between 4 independent processes, not an in-process simulation), but doesn't produce the reusable `docker-compose.yml` artifact the doc's own phrasing suggests. Worth adding later if this devnet needs to be handed to someone else to run.
- **Mirroring the default set's peer membership** (`add_peers_to_reserved_set`) rather than giving the Prism protocol its own independent peer slots -- functionally fine for a small devnet, but not how GRANDPA/transactions actually do it in production, and worth revisiting if peer-slot contention ever becomes a real issue.
- **No mempool / transaction pool integration** -- `build_template`'s tx slot is always fed `&[]` from the live node; wiring a real pending-transaction source is a separate task.
- **Persistent per-chain genesis hashes** (still the single shared `H256::zero()` sentinel from B1) -- becomes actually important once multiple independent networks/chain-specs need to coexist, which multi-node testing would surface.
- **Prism's own difficulty is another hardcoded, untuned constant** (scaled down by a fixed `>>7` from `testnet()`'s values, chosen empirically from a single failed/succeeded live-test cycle, not derived from any target block-time calculation) -- same category of limitation as the B0 PoW difficulty flagged earlier in this doc, just for a different chain. Not re-flagging as a separate action item since the underlying issue (no retargeting, needs real tuning before any serious use) is identical to the one already on record above.
- **Per-vote depth tracking for real stage-1 k-deep confirmation** -- `leader_at_level` above is a raw vote-count snapshot on purpose; the actual "conservative k-deep" confirmation rule (architecture guide §3.8) is B3's job and needs `VoteInfo` to track each vote's originating voter-chain depth, not just a flat total.

## Next: B3

B3 is "ledger stage 1 + Substrate bridge; Aura/GRANDPA removed" -- GRANDPA and Aura are already gone (removed in B0), so the remaining scope here is specifically: (1) per-vote depth tracking + the real conservative k-deep confirmation rule on top of today's raw `leader_at_level`; (2) `BuildLedger` (dedupe + order transactions from a confirmed leader sequence's referenced tx blocks); (3) the actual bridge that imports one Substrate block per confirmed leader level, replacing the throwaway B0 PoW chain this session's testing has been running against; (4) a byte-identical-chains CI-style test across nodes, per the doc's own B3 acceptance criterion.
