# Implementation A & B — Fix Plan (execution checklist)

**You are picking this up in a fresh session. Read this whole file first, then [`IMPLEMENTATION_AB_REVIEW.md`](./IMPLEMENTATION_AB_REVIEW.md) for the *why* behind each item.** This file is the *what to do*; the review is the *why*. Finding IDs (B-1, AF-1, …) refer to the review.

## Ground rules (do not skip)

1. **Work top to bottom.** Items are ordered by priority and dependency. Earlier items unblock later ones.
2. **Live-verify, don't just unit-test.** This project's standard: `cargo test --workspace` is necessary but *not sufficient*. For any item that changes runtime behavior, build the release binary and drive a real devnet (single- or multi-node as noted), because every bug that mattered this project was found live, not by tests. Use `--network-backend libp2p` on every node (see B-8).
3. **Commit after each completed item** (or each coherent group), with a message naming the finding ID. Branch off `Base` if not already on it.
4. **Check the box** (`- [ ]` → `- [x]`) and add a one-line note when an item is done, so the next session sees progress. If you get blocked, leave the box unchecked and write why.
5. **When a fix needs a decision you can't derive** (notably B-3's `δ_d` transcription — see that item), stop and ask the user rather than guessing at consensus-safety math.
6. Update [`IMPLEMENTATION_B_PROGRESS.md`](./IMPLEMENTATION_B_PROGRESS.md) / [`IMPLEMENTATION_A_PROGRESS.md`](./IMPLEMENTATION_A_PROGRESS.md) as items land, so the progress docs stay the source of truth.
7. **Don't trust a piped `cargo test`'s exit code.** `cargo test --workspace 2>&1 | tail -N` reports `tail`'s exit code, not `cargo test`'s -- it will read as `0` (success) even when a test failed. This bit this exact fix-plan session once (a real, since-fixed test failure was briefly reported as "exit code 0" by the pipeline). Either check the actual `test result: ... FAILED` lines in the output, or redirect to a file (`cargo test --workspace > out.log 2>&1; echo $?`) instead of piping through another command.

---

## Phase 1 — correctness & security (must land before any multi-party network)

### - [x] 1. B-1 + AF-4: deterministic bridge block, proven byte-identical across nodes

**Status (this session, code done, live-verify still pending):** `ConfirmedLevel` now carries `timestamp_ms` (the leader `PrismBlock`'s own header timestamp, read from `PrismDb` in `PrismLedger::advance`). `node/src/bridge.rs::build_and_import_block` uses it via `sp_timestamp::InherentDataProvider::new(ts.into())` instead of `from_system_time()`, clamped forward against the previous Substrate block's on-chain `pallet_timestamp::Now` (`previous_timestamp`, a raw storage read) plus `MinimumPeriod` -- `pallet_timestamp::set`'s `assert!(now >= prev + MinimumPeriod)` is a hard panic, not a graceful rejection, so this clamp is load-bearing, not cosmetic. Added `advance_reports_the_leader_blocks_own_timestamp` in `client/prism-ledger/src/tests.rs`. **Not yet done:** the actual two-node `chain_getBlockHash` byte-diff live-verification this item's own acceptance criterion calls for, and the determinism choke-point assertion/test. Do these before considering B-1 fully closed.

**Files:** `node/src/bridge.rs`, `client/prism-ledger` (may need to surface the leader block's timestamp).

**Do:**
- In `build_and_import_block`, replace `sp_timestamp::InherentDataProvider::from_system_time()` with a timestamp that is a pure function of confirmed DAG state. Simplest correct source: the confirmed leader `PrismBlock`'s `SuperHeader.timestamp_ms`. More robust: the median `timestamp_ms` across the leader + its directly-referenced tx blocks (resists one miner lying). Feed it via `sp_timestamp::InherentDataProvider::new(ts.into())`.
- `ConfirmedLevel` (in `prism-ledger`) probably needs to carry that timestamp out to the bridge — add it there so the bridge doesn't re-read the DB.
- Add a determinism choke-point: a `debug_assert`/test that building the same confirmed level against two `PrismChain`s holding the same accepted-block set yields byte-identical `BlockImportParams` (same header hash).

**Verify (this is the B3 acceptance criterion that was never actually run):** two-node libp2p devnet, both mining; after several confirmed levels, `chain_getBlockHash(n)` must be **identical on both nodes for every n**. Script it: loop n, diff node1:9944 vs node2:9945. This test currently would fail — it must pass when this item is done.

### - [ ] 2. A-1: sr25519 dev signature out of the production wasm runtime

**Files:** `primitives/falcon/src/lib.rs`, `primitives/falcon/Cargo.toml`, `runtime/Cargo.toml`, `node/Cargo.toml`.

**Do:**
- Add a Cargo feature (e.g. `dev-signatures`) to `argonaut-primitives-falcon`. Gate `ArgonautSignature::Sr25519`, its `impl Verify` match arm, and `ArgonautSigner::Sr25519` on `#[cfg(feature = "dev-signatures")]` — **not** `cfg(feature = "std")` (that's what caused the original wasm/native mismatch bug: the gate must apply identically to both builds so the enum's variant count stays in sync).
- Propagate the feature through `runtime` and `node` so it's **off** for release/production builds and **on** for dev/test. Ensure the wasm runtime built for production does *not* include the variant.
- Dev accounts (`//Alice` etc.) currently rely on this variant — the `dev`/`local` chain specs and `node/examples/submit_transfer.rs` must build with the feature enabled.

**Verify:** production runtime build (no `dev-signatures`) rejects an sr25519-signed extrinsic; dev build (with it) accepts one. `cargo test --workspace` still green with the feature on. `cargo build --release -p argonaut-node` still works (decide whether release enables `dev-signatures` for the devnet — likely yes for now, but the *runtime wasm* determines what's accepted, so make sure a genuinely-production runtime profile exists that omits it).

**Status: [x] done this session, code + `cargo check --workspace` verified, live wasm-rejection not yet manually confirmed.** `dev-signatures` feature added to `primitives/falcon`, forwarded (not into `std`/default) from `runtime`, unconditionally enabled by `node`'s dependency on both crates -- native-only effect, confirmed safe because `runtime/build.rs`'s `WasmBuilder` runs the wasm target as a fully separate `--no-default-features` cargo subprocess with no explicit feature-forwarding call, so a feature the outer native build enables never reaches the wasm blob. `ArgonautSignature::Sr25519`, its `Verify` arm, and `ArgonautSigner::Sr25519`/its `From` impl are now gated on `dev-signatures` (not `std`). **Not yet done:** an explicit live check that a production wasm build (`argonaut-runtime` compiled without `dev-signatures`) actually rejects an sr25519-signed extrinsic at `TaggedTransactionQueue_validate_transaction` -- the reasoning is solid (mirrors exactly how the original bug was diagnosed) but hasn't been manually reproduced from the "now fixed" side.

### - [x] 3. B-2: client-side rolling-window difficulty retarget

**Status (this session, code + unit-tested, live devnet retarget behavior not yet observed):** Implemented substantially as specified, plus two things the plan above didn't anticipate that turned out to be load-bearing:
- `primitives/prism`: `Params` gained `target_interval_ms` (policy choice, ~1s, explicitly documented as *not* derived from the paper's physical-parameter formula) and `set_combined_target` (the *only* place `difficulty`/`f_v`/`f_t`/`f_p` should ever be reassigned together, to stop the m/f_v mismatch class of bug from recurring). `DifficultyRetarget` (rolling window, ±4x clamp, ignores non-advancing timestamps so it's inert against old-style test blocks that all use `timestamp_ms: 0`) lives here too, with its own unit tests.
- `client/prism-chain`: `PrismChain` owns a `DifficultyRetarget`, fed exactly once per *accepted* block (not per `insert_block` call -- orphan-then-drained blocks would otherwise be double-counted) across all three content-type branches. Persists the current target to a new `PrismDb` column (`CURRENT_DIFFICULTY_TARGET`) and restores it in `with_db` so a restart doesn't reset to the initial guess.
- **Not anticipated by the original plan, found while implementing:** retargeting only changing `PrismChain::params` doesn't actually change mining behavior. `client/prism-miner::PrismMiner`'s real accept/reject gate (`try_mine`'s `sortition(hash, params)` call) reads `PrismMiner`'s *own*, previously-frozen-forever `params` field, not the template's `difficulty` field (that's only ever stamped into the mined header for other nodes to check later) -- so a retargeted `chain.params` reaching `build_template` alone would silently desync the embedded `header.difficulty` from the threshold actually used to accept the block. Fixed by making `PrismMiner.params` swappable (`ArcSwap`, same pattern as its template) and changing `refresh()` to take both together atomically-enough. The *same* staleness existed on the validation side: `PrismNetworkWorker` held its own frozen `params` copy for `handle_announcement`'s sortition-consistency check; fixed by reading `chain.params.clone()` live at validation time instead (removed the now-dead field/constructor arg entirely).
- `node/src/service.rs`: the old permanent `>>7` scaling is now only the *initial guess* (via `set_combined_target`), kept at the same empirical value so the first `RETARGET_WINDOW` blocks -- before retargeting fires even once -- don't reproduce B2's original gossip-flood problem while waiting to converge.
- **Not yet done:** live confirmation that retargeting actually moves the observed block interval toward `target_interval_ms` on a running devnet (the plan's own "single-node holds steady, second miner joins, interval returns to target" check). Only unit-tested (`primitives/prism` and one `client/prism-chain` integration test using real advancing timestamps through `insert_block`) so far.

**Files:** `primitives/prism` (retarget fn + params), `client/prism-chain` or `client/prism-miner` (apply it), `client/prism-db` (persist current target), `node/src/service.rs` (remove the `>>7` hack).

**Do (route 1 from the review — client-side, guide §3.5):**
- Maintain a rolling window of the last N superblock `timestamp_ms` values. Every N blocks, `new_target = old_target * (actual_elapsed / expected_elapsed)`, clamped per step (±4x, Bitcoin-style) to resist timestamp manipulation.
- Retarget the **single combined target only**; keep `f_v : f_t : f_p` as fixed *fractions* of it (they already are in `Params`) so the sortition split is invariant. `Params::testnet()` supplies only the *initial* target + fractions.
- Persist the current target in `PrismDb` (new small column or reuse the metadata pattern) so it survives restart.
- Delete the `>>7` empirical scaling in `node/src/service.rs`; the retarget makes it obsolete.

**Verify:** single-node devnet holds a roughly-stable block interval; then a second miner joins and the interval returns toward target within a window or two (log the target each retarget). This is the paper-model fix — it supersedes all the empirical `>>N` tuning.

---

## Phase 2 — realize the intended design (unblocks Prism-speed confirmation)

### - [x] 4. AF-1: event-driven bridge (wire up `may_confirm`)

**Files:** `client/prism-network/src/wire.rs`, `node/src/bridge.rs`, `node/src/service.rs`.

**Do:** When `insert_block` returns `may_confirm = true`, have the network worker signal the bridge over a `tokio::sync::Notify` (or a channel) instead of the bridge polling. Bridge `await`s the notify; keep a long (multi-second) fallback tick only as a safety net. Removes the dead `may_confirm` plumbing and the 250ms latency floor.

**Status: [x] done this session, code + `cargo check --workspace` verified, latency improvement not yet measured live.** `ReceiveOutcome` gained `may_confirm`, threaded from `InsertOutcome`. A shared `Arc<tokio::sync::Notify>` is created once in `node/src/service.rs` and handed to both `PrismNetworkWorker::new` (calls `.notify_one()` whenever `may_confirm` is true, covering both self-mined and network-received blocks since both flow through the same `process()`) and `run_prism_bridge` (replaced the `sleep(250ms)` loop with `tokio::select! { _ = notify.notified() => {}, _ = sleep(5s) => {} }` -- the 5s branch is the safety-net fallback the plan calls for). **Not yet done:** live measurement of the actual confirmation-to-Substrate-block latency improvement (the plan's own verify step).

**Verify:** confirmation-to-Substrate-block latency drops (log timestamps); no busy-polling of the `PrismChain` lock. `cargo test` green.

### 5. AF-2 + B-3 + B-4: consolidate confirmation in the ledger, real `δ_d`, wire stage-2

These are one work-stream; do in this sub-order.

- [x] **AF-2 (done this session):** Moved `confirmed_leader_at_level(level, k)` out of `prism-chain` into `prism-ledger` (now a free fn `confirmed_leader_at_level(chain: &PrismChain, level, k)` in `client/prism-ledger/src/lib.rs`, right next to `confirmation`). `PrismChain` no longer has this method at all -- everything it read (`level_proposer`, `unvoted_levels`, `votes`, `voter_tips`, `params`) was already public, so no new `PrismChain` API was needed. The 3 tests that exercised it moved from `client/prism-chain/src/tests.rs` to `client/prism-ledger/src/tests.rs` verbatim (just method-call → function-call syntax).
- [ ] **B-3 (NOT done, still a placeholder -- ⚠ needs the user):** `confirmation::placeholder_delta_d` is unchanged. Porting the paper's exact `δ_d` from Appendix E.1 was correctly *not* attempted this session either (same reasoning as before: don't guess consensus-safety math from a rendered PDF). Still needs: `ρ_Δr`, `k_min`, and lemmas E.2-E.4 transcribed with the user's help (paste-from-PDF-text-layer, per the `reference-prism-paper` memory), the same way equations 33-34 were resolved earlier.
- [x] **B-4 (done this session, wired into `PrismLedger::advance` itself, not the bridge):** Rather than adding separate stage-1/stage-2 race logic inside `node/src/bridge.rs`, the check lives one layer down, in `PrismLedger::advance`: at each unconfirmed level it now tries `confirmation::confirm_unique_leader` first and falls back to `confirmed_leader_at_level` (stage-1) if stage-2 hasn't confirmed yet. Since `advance` is called on every bridge wakeup (AF-1), this *is* "whichever fires first in real time" without needing explicit race/select logic -- there's only one call site to change. The bridge itself needed no changes for this part.

**A real interaction was found and fixed by the test suite, not by review:** wiring stage-2 into `advance` made `adversarial_tests::persistent_balancing_attacker_can_stall_confirmation_indefinitely` fail -- correctly. Stage-2 now confirms p1 as a unique leader well before stage-1 ever would, in exactly the scenario the test was designed to have stage-1 fail forever (one shallow dissenting chain against three deep, unanimous honest ones). That's stage-2 doing its job, not a bug; the test was renamed to `persistent_balancing_attacker_stalls_stage1_alone_but_not_the_system` and rewritten to assert *both* halves against the same attack sequence: `confirmed_leader_at_level` (stage-1 in isolation) never confirms, while `ledger.advance` (the system, stage-1+stage-2) does, within the existing round budget. `cargo test --workspace` is green with this change (95 tests, 0 failures, confirmed via a real run -- not assumed from a pipe's exit code, which masked a failure earlier in this same session; see the "note on pipe exit codes" ground rule if you add one).

**Verify:** unit tests for the moved policy pass (done, see above). **Not yet done:** the plan's own live-devnet check ("devnet confirms levels faster than stage-1 alone once stage-2 is live") -- only unit-tested so far, no live timing measurement.

---

## Phase 3 — production-hardening (before a serious multi-node network)

### - [ ] 6. B-7 + AF-5: persist DAG indices atomically; ledger traverses in-memory

**Files:** `client/prism-db`, `client/prism-chain`, `client/prism-ledger`.

**Do:** Populate the declared-but-unused index CFs (`PROPOSER_NODE_LEVEL`, `VOTER_NODE_CHAIN`, `GRAPH_*`, …) inside `insert_block`'s `WriteBatch` (atomic multi-CF commit, §3.3 — a block is never half-indexed), and load from them on startup instead of full-replay. Have `build_ledger_delta` traverse `prism-chain`'s in-memory metadata (`proposer_meta`) for graph structure, hitting the DB only for the tx *bytes* it emits. (Prerequisite for efficient B5 pruning.)

**Verify:** restart a node with a non-trivial DAG; it recovers from indices without full replay and produces identical state.

### - [ ] 7. B-6: initial sync / historical backfill for Prism state

**Files:** `client/prism-network`.

**Do:** Implement §3.6's strategy: request peers' voter tips + proposer best level, then breadth-first backfill by parent refs. Currently only reactive orphan-fetch exists, so a fresh/long-offline node never converges.

**Verify:** start a node against a peer with existing history and an empty `PrismDb`; it catches up to the peer's DAG.

### - [ ] 8. B-9: root-cause the two-honest-miner confirmation stall

**Do:** Build a deterministic repro first (two in-process miners feeding one `PrismChain`, or a scripted two-template race) — see the review B-9 hypothesis (honest miners split votes across siblings, never revisited absent a reorg). Then decide: honest-vote tie-break/switch-toward-plurality, or is stage-2 (item 5) the intended answer? **Do not scale `m` back to 100 for multi-miner runs until this is understood.**

---

## Phase 4 — as convenient (no correctness impact)

- [ ] 9. **AF-3:** finer-grained DAG concurrency (per-voter-chain locks or actor model). Only when measured to matter.
- [ ] 10. **B-5:** per-chain genesis hashes from `(chain_spec_genesis, chain_id)` instead of the shared `H256::zero()` sentinel (`primitives/prism::genesis_hash` already exists, unused).
- [ ] 11. **B-8:** port `client/prism-network`'s sync-request from litep2p's `unimplemented!()` async `request` to the callback-based `start_request`, so `--network-backend libp2p` is no longer mandatory.
- [ ] 12. **B-10:** docker-compose devnet artifact; mirror the default peer set (`add_peers_to_reserved_set`) instead of independent Prism peer slots.
- [ ] 13. **A-3/A-4:** registry-mode transaction-builder tooling (extend `submit_transfer.rs`); size-assertion test (registry-mode ≤ 850 bytes); optional auto-register-on-first-bootstrap-tx.

---

## Then: B5 (pruning)

Only after Phase 3 (B-7 especially — pruning needs persisted, on-disk DAG indices to prune against). B5 scope is in `IMPLEMENTATION_B_PROGRESS.md`'s "Next: B5". Remember `PrismLedger::build_ledger_delta` must treat "ref below the prune frontier" as "already counted," not as a DB-lookup failure.
