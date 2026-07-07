# Implementation A & B — Fix Plan (execution checklist)

**You are picking this up in a fresh session.** Read [`IMPLEMENTATION_AB_REVIEW.md`](./IMPLEMENTATION_AB_REVIEW.md) for the *why* behind any item below (finding IDs B-1, AF-1, … refer to it). This file is the current *to-do list* — history of what was already fixed lives in git log / the review doc's `Status` lines, not here.

## Ground rules (do not skip)

1. **Live-verify, don't just unit-test.** `cargo test --workspace` is necessary but *not sufficient*. For any item that changes runtime behavior, build the release binary and drive a real devnet (single- or multi-node as noted), because every bug that mattered on this project was found live, not by tests. Use `--network-backend libp2p` on every node (litep2p's request/response is `unimplemented!()` — B-8).
2. **Commit after each completed item** (or each coherent group), with a message naming the finding ID. Branch off `Base` if not already on it.
3. **When a fix needs a decision you can't derive** (consensus-safety math, threat-model assumptions), stop and ask the user rather than guessing.
4. Update [`IMPLEMENTATION_B_PROGRESS.md`](./IMPLEMENTATION_B_PROGRESS.md) / [`IMPLEMENTATION_A_PROGRESS.md`](./IMPLEMENTATION_A_PROGRESS.md) and the review doc's `Status` lines as items land, so all three stay the source of truth.
5. **Don't trust a piped `cargo test`'s exit code.** `cargo test --workspace 2>&1 | tail -N` reports `tail`'s exit code, not `cargo test`'s — reads as `0` even when a test failed. Check the actual `test result: ... FAILED` lines, or redirect to a file and check `$?` there.

---

## Closed items (code done, unit-tested) — nothing to do unless noted

These don't need re-work. Each line is the one thing worth remembering, not a history.

- **B-1 + AF-4** (deterministic bridge timestamp): fixed — `ConfirmedLevel` carries the leader block's own `timestamp_ms`, fed into `sp_timestamp::InherentDataProvider`, clamped forward against on-chain `pallet_timestamp::Now`.
- **A-1** (sr25519 out of production wasm): fixed — `dev-signatures` Cargo feature, off by default, propagated identically to wasm/native so the enum shape stays in sync.
- **B-2** (difficulty retargeting): fixed — client-side rolling-window retarget (±4x clamp/step) in `primitives/prism` + `client/prism-chain`, persisted across restart; replaces the old permanent `>>7` scale.
- **AF-1** (event-driven bridge wakeup): fixed — `Arc<tokio::sync::Notify>` signaled on `may_confirm`, replacing the 250ms poll loop (5s fallback tick retained).
- **AF-2** (confirmation policy layering): fixed — `confirmed_leader_at_level` moved out of `prism-chain` into `prism-ledger`, which now owns both confirmation stages.
- **B-4** (wire stage-2 into the live path): fixed — `PrismLedger::advance` tries `confirm_unique_leader` (stage-2) first, falls back to stage-1, on every bridge wakeup.
- **B-7 + AF-5** (restart correctness): fixed — `PrismChain::with_db` full-replays persisted blocks; `PrismLedger::recover` rebuilds the confirmed cursor from the Substrate client's own height. This closed a real double-confirm/double-import bug on restart, not just a performance gap.
- **B-9** (two-honest-miner stall): root-caused, not a bug — an *exact* even split between two honest miners is structurally unresolvable forever (proven, `client/prism-ledger/src/honest_split_tests.rs`); any real skew resolves via stage-2. **Operational caveat, keep in mind for future devnet runs:** don't run small-`m` (~4), exactly-2-miner devnets expecting fast guaranteed confirmation — the paper's intended `m` (100+) makes a sustained exact tie vanishingly unlikely.
- **B-3, partial** (paper's exact `δ_d`/`k_min`, Appendix E.1 eq. 31): the faithful port exists (`paper_k_min`, `paper_delta_d` in `client/prism-ledger/src/confirmation.rs`) but is **deliberately not wired in as the default** — the paper's literal `k_min` is in the thousands for any sane β (an asymptotic-proof artifact), which would make stage-2 *slower* than stage-1, the opposite of its purpose. `ConfirmationParams::default()` still uses the practical placeholder. **Don't "fix" this by switching the default to `paper_k_min` without re-deriving a usable operating point first** — that's the mistake this note exists to prevent.

---

## Open work

### Live-verification debt (code done above; these runs haven't happened yet)

- [ ] Two-node `chain_getBlockHash(n)` byte-diff across many confirmed levels (B-1's own acceptance criterion — never actually run).
- [ ] AF-4 determinism choke-point: an assert/test that two `PrismChain`s with the same accepted-block set produce byte-identical `BlockImportParams`.
- [ ] A-1: confirm a production wasm build (no `dev-signatures`) actually rejects an sr25519-signed extrinsic at `TaggedTransactionQueue_validate_transaction`.
- [ ] B-2: confirm retargeting moves the observed block interval toward `target_interval_ms` on a running devnet (single miner steady → second joins → interval returns to target).
- [ ] AF-1: measure the actual confirmation-to-Substrate-block latency improvement.
- [ ] B-4: measure that the devnet confirms levels faster with stage-2 live than stage-1 alone.
- [ ] B-7/AF-5: live-devnet restart with a non-trivial DAG (unit-tested only so far).

### 1. B-6: initial sync / historical backfill for Prism state

**Files:** `client/prism-network` (`src/lib.rs` for wire types, `src/wire.rs` for the worker).

Currently only reactive orphan-fetch exists, so a fresh/long-offline node never converges (§3.6).

**Plan:**
- **Wire: add a status exchange.** New `SyncRequest::GetStatus` → `SyncResponse::Status { proposer_best_level: u64, voter_tips: Vec<H256>, unreferenced_proposers: Vec<H256>, unreferenced_tx_blocks: Vec<H256> }`. Reads straight off `PrismChain`'s already-public fields — no new DB indices needed now that B-7 keeps the in-memory DAG complete at all times. SCALE enums decode by variant index, so appending variants keeps old messages decodable; an old node receiving `GetStatus` just fails decode and returns `Err` (fine for a devnet).
- **Replace recursion with an iterative work-queue for the backfill path.** Convergence would already happen via the existing fetch→process→fetch recursion (same fixed-point argument B-7 proved for replay), but it resolves one hop per round trip with an unbounded `Box::pin` recursion depth. Instead: keep a `pending: HashSet<H256>` + queue; request in batches (~64 hashes, response cap 16 MiB per `sync_protocol_config`); re-queue hashes that didn't come back with a per-hash retry cap / peer rotation. Insert each returned block through `handle_announcement`, push its `to_fetch` back onto the queue, loop until empty.
- **Trigger points:** on `NotificationStreamOpened`, send `GetStatus`; start backfill if the peer's `proposer_best_level` exceeds ours or any of its tips is unknown. Add a slow periodic re-check (~30s) so a long-offline node converges without needing a fresh connection event.
- **Suppress re-gossip for backfilled blocks:** `process` currently floods every accepted block to all peers; pass a `from_backfill` flag that skips `gossip` but keeps validation/insertion/`may_confirm` identical.
- **Bridge needs no changes:** a fresh node building its Substrate chain from level 1 as backfill confirms levels is the intended behavior; `PrismLedger::recover(db, client.info().best_number)` starting at 0 on a fresh node is already right.
- **Inherits B-8's `--network-backend libp2p` requirement** (same `network.request` call site as orphan-fetch; B-8 fixes both at once).
- **No new trust surface:** every backfilled block still flows through `validate_block` + live-params sortition checks; only new resource to bound is the pending-queue size.

**Verify:** start node2 with an empty `PrismDb` (libp2p backend) against a node1 holding a long non-trivial history; node2's `proposer_best_level` converges to node1's, and `chain_getBlockHash(n)` matches node1 for every n (doubles as B-1's live check — do both in one session). Repeat with node2 stopped for a long stretch mid-run and restarted.

### 2. B-5: per-chain genesis hashes (not to be confused with "B5 pruning" below)

**What it costs today:** every Prism DAG on every network roots at the shared `H256::zero()` sentinel — no network domain separation (a block mined against network A's genesis is structurally valid ancestry on network B) and no chain-slot separation at the genesis boundary. `primitives/prism::genesis_hash(chain_spec_genesis, slot)` already does the right derivation (blake3 over genesis‖slot) but is unused — this is purely plumbing:

- **Slot convention:** voter chain `i` → slot `i` (0..m-1), proposer tree → slot `m`. Tx blocks parent onto proposers, so need no slot of their own — fix `genesis_hash`'s stale doc comment ("m voter + 1 tx + 1 proposer") to match.
- **Put `chain_spec_genesis: H256` in `Params`** — reachable by both `PrismChain` and `PrismMiner::new` (which builds its default template from `Params` alone, no chain handle). `node/src/service.rs` sets it from `client.chain_info().genesis_hash`; tests pass a fixed dummy.
- **`PrismChain`:** compute `voter_genesis: Vec<H256>` + `proposer_genesis: H256` once in `new()`; seed `voter_meta`/`proposer_meta` accordingly. Delete `genesis_sentinel()`.
- **Check while in there:** a voter block claiming chain `i` whose parent is chain `j`'s genesis/tip should get rejected — per-chain geneses make this enforceable at the genesis boundary for the first time.
- **DB compatibility — don't skip:** existing devnet DBs hold zero-parented blocks; after this change, B-7's replay would orphan the entire store. Store `chain_spec_genesis` under a meta key in `PrismDb` and hard-error on mismatch. Devnet migration answer: purge and remine — document it, don't build migration.
- **No wire-format changes:** parents are already plain `H256`s; only the *values* change.

**Verify:** adapt fixtures to a nonzero dummy genesis; new tests for foreign-genesis-orphans and wrong-chain-genesis-parent-rejected; B-7's restart-recovery test still passes with per-chain geneses. Then fresh DBs on both nodes, two-node mine-and-confirm, plus one restart.

### 3. Backlog — as convenient, no correctness impact

- [ ] **AF-3:** finer-grained DAG concurrency (per-voter-chain locks or actor model). Only when measured to matter — the whole DAG is behind one global `Mutex` today.
- [ ] **B-8:** port `client/prism-network`'s sync-request from litep2p's `unimplemented!()` async `request` to the callback-based `start_request`, so `--network-backend libp2p` is no longer mandatory.
- [ ] **B-10:** docker-compose devnet artifact; mirror the default peer set (`add_peers_to_reserved_set`) instead of independent Prism peer slots.
- [ ] **A-3/A-4:** registry-mode transaction-builder tooling (extend `submit_transfer.rs`); size-assertion test (registry-mode ≤ 850 bytes); optional auto-register-on-first-bootstrap-tx.
- [ ] **Full network-level adversarial run:** the B4 adversarial harness (`client/prism-ledger/src/adversarial_tests.rs`) drives `PrismChain`/`PrismLedger` directly with attacker-chosen blocks (real state machine, no mocked adversary) — but nothing has run an actual multi-node adversarial scenario with real gossip/timing the way the honest-case live devnets have been run.

---

## Then: B5 (pruning)

Only after B-7 (persisted, on-disk DAG indices — still deferred as a scale concern, see B-7's closed-item note above; pruning needs something on-disk to prune against). Scope: voter blocks > `k_prune` behind their chain tip AND below the last confirmed level are deletable; tx-block bodies referenced only by pruned history archived/dropped by config. Must also make `PrismLedger::build_ledger_delta`'s DFS over `tx_refs`/`prop_refs` treat "ref below the prune frontier" as "already counted," not a DB-lookup failure.
