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
- **B-6** (initial sync / historical backfill): fixed — `SyncRequest::GetStatus`/`SyncResponse::Status` wire messages (`client/prism-network/src/lib.rs`); `PrismNetworkWorker::request_status_and_maybe_backfill` sends `GetStatus` on `NotificationStreamOpened` and every 30s per connected peer, filters the response against the new `PrismChain::has_block` (chain-state check, not the worker's own `seen` cache, so this is correct across restarts too), and feeds unknown hashes into `backfill`'s iterative batched (64/round) work-queue with a bounded per-hash retry count. Backfilled blocks go through `insert_backfill_block`, which skips re-gossip (unlike `process`'s live path) but is otherwise identical validation/insertion. `PrismChain::has_block` is new and load-bearing: re-`insert_block`-ing an already-accepted proposer is not idempotent (see its doc comment), so backfill must filter before requesting, not rely on `handle_announcement`'s dedup.

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
- [ ] B-6: two-node live backfill (fresh node2 against a non-trivial node1 history; repeat with node2 stopped mid-run and restarted) — unit-tested only so far (`insert_backfill_block`/`backfill`/`has_block` logic), no real `sc-network` run yet since `PrismNetworkWorker` has no test harness (same gap every other wire.rs-level item has).
- [ ] B-5: two-node fresh-DB mine-and-confirm plus one restart with real per-chain genesis hashes (unit-tested only so far).

### 1. B-5: per-chain genesis hashes (not to be confused with "B5 pruning" below)

**What it costs today:** every Prism DAG on every network roots at the shared `H256::zero()` sentinel — no network domain separation (a block mined against network A's genesis is structurally valid ancestry on network B) and no chain-slot separation at the genesis boundary. `primitives/prism::genesis_hash(chain_spec_genesis, slot)` already does the right derivation (blake3 over genesis‖slot) but is unused — this is purely plumbing:

- **Slot convention:** voter chain `i` → slot `i` (0..m-1), proposer tree → slot `m`. Tx blocks parent onto proposers, so need no slot of their own — fix `genesis_hash`'s stale doc comment ("m voter + 1 tx + 1 proposer") to match.
- **Put `chain_spec_genesis: H256` in `Params`** — reachable by both `PrismChain` and `PrismMiner::new` (which builds its default template from `Params` alone, no chain handle). `node/src/service.rs` sets it from `client.chain_info().genesis_hash`; tests pass a fixed dummy.
- **`PrismChain`:** compute `voter_genesis: Vec<H256>` + `proposer_genesis: H256` once in `new()`; seed `voter_meta`/`proposer_meta` accordingly. Delete `genesis_sentinel()`.
- **Check while in there:** a voter block claiming chain `i` whose parent is chain `j`'s genesis/tip should get rejected — per-chain geneses make this enforceable at the genesis boundary for the first time.
- **DB compatibility — don't skip:** existing devnet DBs hold zero-parented blocks; after this change, B-7's replay would orphan the entire store. Store `chain_spec_genesis` under a meta key in `PrismDb` and hard-error on mismatch. Devnet migration answer: purge and remine — document it, don't build migration.
- **No wire-format changes:** parents are already plain `H256`s; only the *values* change.

**Verify:** adapt fixtures to a nonzero dummy genesis; new tests for foreign-genesis-orphans and wrong-chain-genesis-parent-rejected; B-7's restart-recovery test still passes with per-chain geneses. Then fresh DBs on both nodes, two-node mine-and-confirm, plus one restart.

**Status (2026-07-07): done, code + unit tests, `cargo test --workspace` green.** Implemented per the plan above with no deviations:
- `primitives/prism::Params` gained `chain_spec_genesis: H256`; `genesis_hash`'s doc comment fixed to the real `m voter (slot 0..m-1) + proposer tree (slot m)` convention, no tx-block slot.
- `PrismChain` gained `voter_genesis: Vec<H256>`/`proposer_genesis: H256` (computed once in `new()` from `params.chain_spec_genesis`), replacing `genesis_sentinel()` (deleted) everywhere it was used, including inside `ChainCheckpoint`-based restart (recomputed identically from `params` every `with_db` call, not persisted -- deterministic given the same `chain_spec_genesis`).
- New genesis-boundary check in `insert_block`'s voter arm: a block claiming chain `i` whose parent is *some* chain's genesis but not chain `i`'s own is rejected outright (not orphaned) -- was structurally undetectable before per-chain geneses existed.
- `PrismDb` gained `CHAIN_SPEC_GENESIS` (single-key CF); `PrismChain::with_db` hard-errors (`assert_eq!`) on a mismatch against a previously-stored value, matching the "purge and remine" migration answer -- no migration path was built.
- `client/prism-miner`: `build_template`'s proposer-parent fallback and `PrismMiner::new`'s default template both switched from the shared sentinel to `chain.proposer_genesis`/re-derived per-chain genesis hashes (the latter has no `PrismChain` handle, so it re-derives via `genesis_hash` directly from `params.chain_spec_genesis` -- must stay in sync with `PrismChain::new`'s derivation, same slot convention).
- `node/src/service.rs` sets `chain_spec_genesis` from `client.info().genesis_hash` (the Substrate chain's own genesis, already network-unique and agreed by every node).
- Test fixtures across `prism-chain`, `prism-ledger` (`tests.rs`, `adversarial_tests.rs`, `honest_split_tests.rs`, `confirmation/tests.rs`), `prism-miner`, and `prism-network` updated from the shared sentinel to `chain.proposer_genesis`/`chain.voter_genesis[i]`, picking the right per-chain value at each call site (several tests use multiple voter chains and needed splitting a single shared `genesis` variable into distinct proposer/per-chain values). Two new tests added to `prism-chain`: `foreign_chain_spec_genesis_produces_unresolvable_orphans` and `wrong_chain_genesis_parent_is_rejected_not_orphaned`, matching this section's own "Verify" line.
- Not yet done: the live two-node verify pass (fresh DBs, mine-and-confirm, one restart) -- unit-tested only so far, per [[feedback-live-verification-required]] in memory.

### 2. Backlog — as convenient, no correctness impact

- [ ] **AF-3:** finer-grained DAG concurrency (per-voter-chain locks or actor model). Only when measured to matter — the whole DAG is behind one global `Mutex` today.
- [ ] **B-8:** port `client/prism-network`'s sync-request from litep2p's `unimplemented!()` async `request` to the callback-based `start_request`, so `--network-backend libp2p` is no longer mandatory.
- [ ] **B-10:** docker-compose devnet artifact; mirror the default peer set (`add_peers_to_reserved_set`) instead of independent Prism peer slots.
- [ ] **A-3/A-4:** registry-mode transaction-builder tooling (extend `submit_transfer.rs`); size-assertion test (registry-mode ≤ 850 bytes); optional auto-register-on-first-bootstrap-tx.
- [ ] **Full network-level adversarial run:** the B4 adversarial harness (`client/prism-ledger/src/adversarial_tests.rs`) drives `PrismChain`/`PrismLedger` directly with attacker-chosen blocks (real state machine, no mocked adversary) — but nothing has run an actual multi-node adversarial scenario with real gossip/timing the way the honest-case live devnets have been run.

---

## Then: B5 (pruning)

Only after B-7 (persisted, on-disk DAG indices — still deferred as a scale concern, see B-7's closed-item note above; pruning needs something on-disk to prune against). Scope: voter blocks > `k_prune` behind their chain tip AND below the last confirmed level are deletable; tx-block bodies referenced only by pruned history archived/dropped by config. Must also make `PrismLedger::build_ledger_delta`'s DFS over `tx_refs`/`prop_refs` treat "ref below the prune frontier" as "already counted," not a DB-lookup failure.

**Design decisions (resolved 2026-07-07, offline consultation) — these are the instructions; the original two open questions are restated inline for context.**

1. **On-disk index shape → (b), the scoped checkpoint. Do not populate the 13 legacy CFs.**
   *(Question was: (a) design encodings for the 13 declared-and-unused prism-rust column families in `client/prism-db/src/columns.rs`, vs. (b) persist one SCALE-encoded checkpoint of exactly what `PrismChain::with_db` needs.)*
   - **Why (b) is not just smaller but forced:** pruning deletes voter bodies from `BLOCKS`, after which `with_db`'s full replay is *structurally impossible* — retained blocks' ancestors are gone, so replay would orphan everything above the prune frontier. Any pruning design therefore needs a non-replay restart path; option (a) would need that same checkpoint *plus* 13 speculative schemas nothing reads. The "cross-repo compatibility" the legacy names promise is illusory: no tool reads those CFs, and this port's data model (`VoteInfo::per_chain` depth map, `unvoted_levels`, `level_proposer`) has no prism-rust equivalent anyway. Delete the dead constants from `ALL_COLUMNS` (devnet DB answer stays purge-and-remine, so no CF-drop migration); keep the five live ones: `BLOCKS`, `BLOCK_STATUS`, `CURRENT_DIFFICULTY_TARGET`, `PROPOSER_LEADER_SEQUENCE`, `PROPOSER_LEDGER_ORDER`.
   - **Checkpoint contents** (one versioned SCALE struct, single key in a new CF): `voter_tips`, `proposer_best_level`, `voter_meta` (retained window only), `proposer_meta`, `votes`, `unvoted_levels`, `level_proposer`, `unreferenced_tx_blocks`, `unreferenced_proposers`, `known_tx_blocks`. **Exclude `orphans`** — unaccepted by definition; B-6 backfill re-fetches them.
   - **Do NOT write it on every `insert_block`** (the question's own framing was wrong there): the struct is O(total state), so per-insert writes are quadratic write amplification. Write it every N inserts (N ~ hundreds), at every prune, and on clean shutdown. Restart = load checkpoint, then iterate `BLOCKS` feeding only blocks where `has_block` is false (that guard is what makes replay-over-a-checkpoint safe — re-inserting an accepted proposer is the known non-idempotency). A seq-number watermark CF to skip the iteration entirely is a later perf item, not needed for correctness.
   - **Ledger-side precondition, do this FIRST:** `PrismLedger::recover` rebuilds `emitted_tx_hashes` by re-reading tx *bodies* (`build_ledger_delta_for_leader` over all confirmed levels) — the moment any tx body is prunable, that silently loses dedup state on restart → double-emission. Persist emitted tx hashes incrementally (append to a new CF inside `advance`, same moment as `PROPOSER_LEADER_SEQUENCE`), and make `recover` load that CF instead of re-expanding. Independent, safe, and speeds up restart even before pruning lands.

2. **Prune safety bound → reading (a) for voter blocks; the confirmed-level gate was never about voter blocks — it belongs to proposer/tx *body* pruning, supplied by the caller.**
   *(Question was: is `k_prune` behind the voter-chain tip alone sufficient, or must a voter block's voted levels also be `<= PrismLedger::confirmed_tip`?)*
   - **Verified basis for (a):** both confirmation stages read only `chain.votes` + `voter_tips` (`vote_depths_from_tip`, `confirmation.rs`) — never `voter_meta` history. A pruned voter block's votes stay fully counted at their recorded cast-depths whether or not the voted levels are confirmed yet. Pruning must simply **never mutate `votes`** — that's the real invariant (see tests note below). No `prism-ledger` dependency, no proposer-level gate for voter blocks.
   - **What `k_prune` actually bounds is reorg processability:** `reorg_voter_chain`'s LCA walk indexes `voter_meta[&h]` directly and would *panic* if a reorg crosses the prune frontier. Track a per-chain pruned-frontier depth; a fork whose walk would cross it gets logged and ignored (by assumption it's adversarial beyond the security parameter), and an incoming voter block whose parent is below the frontier is **rejected, not orphaned** — otherwise it sits in `orphans` requesting a parent that will never arrive. Retention is cheap: default `k_prune` to a large multiple of stage-1's `k` (≥10x).
   - **Where the confirmed-level gate genuinely applies:** proposer/tx-block *bodies*. `build_ledger_delta_for_leader` reads tx bodies from db at confirmation time, so a body may only be dropped once every proposer level that could still pull it into the ledger is `<= confirmed_tip`. Resolve the crate-coupling objection by putting the decision at the caller: `prune(per_chain_voter_frontier, ledger_confirmed_level)` on `PrismChain`/`PrismDb`, with `node/src/bridge.rs` (which already owns both crates' handles) supplying `ledger_confirmed_level` from `PrismLedger::confirmed_tip()`. `prism-chain` never imports `prism-ledger`.
   - **"Ref below frontier = already counted" falls out for free:** `expand_proposer_tx_refs` already `Ok(Some(..))`-skips missing blocks, and the durable emitted-hash set (item 1's precondition) carries the dedup semantics. No DFS change needed beyond that precondition.
   - **`has_block`/backfill hazard — must fix in the same change:** `has_block` reads only in-memory maps, so pruning meta makes pruned hashes look unknown → B-6 backfill re-requests them → re-insert non-idempotency corruption plus unbounded re-download. On prune, do **not** delete the `BLOCK_STATUS` row (33 bytes/entry, keep forever); set it to a new `BlockStatus::Pruned` and make the backfill filter (and the insert path's below-frontier rejection) consult it.
   - **Tests:** the blanket `vote_totals() == recount_votes()` invariant stays valid for all non-pruning tests, unchanged. For pruning tests the invariant becomes: snapshot `vote_totals()` before `prune()`, assert byte-identical after (pruning is a no-op on vote accounting), and assert both confirmation stages return the same answers before/after. Don't build a windowed `recount_votes` variant — it would need a pruned-votes baseline to compare against `vote_totals` and adds nothing over the snapshot check.
   - **Operational note, document with the feature:** a pruned node cannot serve full historical backfill to a fresh peer (B-6 serves from `BLOCKS`). Until checkpoint-sync exists, pruning ships **off by default** behind a config flag, and every devnet keeps ≥1 archive node.

**Implementation order:** (1) persist emitted tx hashes + `recover` from CF; (2) `PrismChain` checkpoint + guarded replay + dead-CF removal; (3) `BlockStatus::Pruned` + `has_block`/backfill awareness + below-frontier rejection; (4) `prune()` itself (voter meta+bodies by `k_prune`; proposer/tx bodies by caller-supplied confirmed level); (5) tests per above, then the ground-rule live run: two-node devnet with pruning on, node2 restart after prune, fresh node3 backfilling from the archive node.

**Status (2026-07-07): steps 1-5's code + unit tests done, `cargo test --workspace` green.** All five steps landed as designed above:
- `EMITTED_TX_HASH` CF + `PrismDb::put_emitted_tx_hash`/`iter_emitted_tx_hashes`; `PrismLedger::recover` now just loads the CF (no more re-running the DFS over every confirmed level's tx bodies).
- `client/prism-db/src/columns.rs` trimmed to the 5 live CFs; `CHAIN_CHECKPOINT` added.
- `PrismChain`: `ChainCheckpoint` (SCALE struct, `Vec`-pair fields per the existing wire-message convention, `checkpoint_now`/`maybe_checkpoint` every `CHECKPOINT_INTERVAL = 256` accepted blocks), `with_db` loads it then replays only `!has_block` blocks from `BLOCKS`, `has_block` falls back to a `BlockStatus::Pruned` DB check, `insert_block`'s voter arm rejects (not orphans) a block whose parent is tombstoned pruned.
- `PrismChain::prune(k_prune, ledger_confirmed_level)`: voter blocks walked back from each chain's live tip and removed once `> k_prune` deep (never touches `votes`); proposer/tx bodies below `ledger_confirmed_level` dropped via the same DFS shape as `build_ledger_delta_for_leader`, tombstoned, meta kept forever. Calls `checkpoint_now()` at the end.
- 4 new tests in `client/prism-chain/src/tests.rs`: vote-accounting-unchanged-by-pruning (+ the accepted `recount_votes` divergence past the frontier), pruned-blocks-stay-known + descendant-rejection, confirmed-body-pruning-keeps-meta-queryable, restart-after-pruning.

**Wiring `prune()` into a live node: done (2026-07-07).** `--prism-prune` (off by default) + `--prism-k-prune` (default 60, i.e. 10x `K_DEEP_CONFIRMATION`) added to `Cli` in `node/src/cli.rs`, threaded through `command.rs` → `service::new_full` → `run_prism_bridge`. The bridge loop now distinguishes which branch of its `tokio::select!` fired (notify vs. the 5s fallback tick) and only calls `chain.prune(k_prune, ledger.confirmed_tip())` on the fallback-tick branch when `--prism-prune` is set -- so pruning runs at most once per ~5s, not once per confirmed level. `cargo check -p argonaut-node` clean.

**Not done — deliberately out of scope for this pass, do next:**
- **Live-verify** (per ground rule 1, and unit-tests-only per [[feedback-live-verification-required]]): two-node devnet with `--prism-prune` on; node2 restart after a prune; fresh node3 backfilling against an archive (non-pruning) node1 -- this last one exercises B-6 and B5 together and is the actual point of the "no new trust surface" claim in B-6's own section.
- **Known, accepted, bounded leaks** (documented in `PrismChain::prune`'s doc comment, not bugs): voter-chain blocks on an already-abandoned fork predating the frontier aren't reachable by walking a live tip backward, so they're never swept; non-canonical sibling proposers at a confirmed level keep their bodies (only the leader's own subtree is walked). Both would need a per-chain/per-level reverse index this port doesn't maintain -- same practical-compromise category as B-3's `k_min`.
