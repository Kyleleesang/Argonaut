# Implementation A & B — Fix Plan (execution checklist)

**You are picking this up in a fresh session. Read this whole file first, then [`IMPLEMENTATION_AB_REVIEW.md`](./IMPLEMENTATION_AB_REVIEW.md) for the *why* behind each item.** This file is the *what to do*; the review is the *why*. Finding IDs (B-1, AF-1, …) refer to the review.

## Ground rules (do not skip)

1. **Work top to bottom.** Items are ordered by priority and dependency. Earlier items unblock later ones.
2. **Live-verify, don't just unit-test.** This project's standard: `cargo test --workspace` is necessary but *not sufficient*. For any item that changes runtime behavior, build the release binary and drive a real devnet (single- or multi-node as noted), because every bug that mattered this project was found live, not by tests. Use `--network-backend libp2p` on every node (see B-8).
3. **Commit after each completed item** (or each coherent group), with a message naming the finding ID. Branch off `Base` if not already on it.
4. **Check the box** (`- [ ]` → `- [x]`) and add a one-line note when an item is done, so the next session sees progress. If you get blocked, leave the box unchecked and write why.
5. **When a fix needs a decision you can't derive** (notably B-3's `δ_d` transcription — see that item), stop and ask the user rather than guessing at consensus-safety math.
6. Update [`IMPLEMENTATION_B_PROGRESS.md`](./IMPLEMENTATION_B_PROGRESS.md) / [`IMPLEMENTATION_A_PROGRESS.md`](./IMPLEMENTATION_A_PROGRESS.md) as items land, so the progress docs stay the source of truth.

---

## Phase 1 — correctness & security (must land before any multi-party network)

### - [ ] 1. B-1 + AF-4: deterministic bridge block, proven byte-identical across nodes

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

### - [ ] 3. B-2: client-side rolling-window difficulty retarget

**Files:** `primitives/prism` (retarget fn + params), `client/prism-chain` or `client/prism-miner` (apply it), `client/prism-db` (persist current target), `node/src/service.rs` (remove the `>>7` hack).

**Do (route 1 from the review — client-side, guide §3.5):**
- Maintain a rolling window of the last N superblock `timestamp_ms` values. Every N blocks, `new_target = old_target * (actual_elapsed / expected_elapsed)`, clamped per step (±4x, Bitcoin-style) to resist timestamp manipulation.
- Retarget the **single combined target only**; keep `f_v : f_t : f_p` as fixed *fractions* of it (they already are in `Params`) so the sortition split is invariant. `Params::testnet()` supplies only the *initial* target + fractions.
- Persist the current target in `PrismDb` (new small column or reuse the metadata pattern) so it survives restart.
- Delete the `>>7` empirical scaling in `node/src/service.rs`; the retarget makes it obsolete.

**Verify:** single-node devnet holds a roughly-stable block interval; then a second miner joins and the interval returns toward target within a window or two (log the target each retarget). This is the paper-model fix — it supersedes all the empirical `>>N` tuning.

---

## Phase 2 — realize the intended design (unblocks Prism-speed confirmation)

### - [ ] 4. AF-1: event-driven bridge (wire up `may_confirm`)

**Files:** `client/prism-network/src/wire.rs`, `node/src/bridge.rs`, `node/src/service.rs`.

**Do:** When `insert_block` returns `may_confirm = true`, have the network worker signal the bridge over a `tokio::sync::Notify` (or a channel) instead of the bridge polling. Bridge `await`s the notify; keep a long (multi-second) fallback tick only as a safety net. Removes the dead `may_confirm` plumbing and the 250ms latency floor.

**Verify:** confirmation-to-Substrate-block latency drops (log timestamps); no busy-polling of the `PrismChain` lock. `cargo test` green.

### - [ ] 5. AF-2 + B-3 + B-4: consolidate confirmation in the ledger, real `δ_d`, wire stage-2

These are one work-stream; do in this sub-order.

- **AF-2:** Move stage-1's `confirmed_leader_at_level(level, k)` policy out of `prism-chain` into `prism-ledger` (alongside `confirmation.rs`), leaving `prism-chain` to expose only raw facts (`proposers_at_level`, per-vote depths). `prism-ledger` then owns both stages, per §3.8.
- **B-3 (⚠ may need the user):** Replace `confirmation::placeholder_delta_d` with the paper's exact `δ_d` from Appendix E.1 (typicality lemma; constants `ρ_Δr`, `k_min`, lemmas E.2–E.4). See the `reference-prism-paper` memory for the PDF-extraction method. **If the dense inline math can't be transcribed with confidence, stop and ask the user to copy-paste the equations from the PDF text layer** — do not guess a consensus-safety margin. Keep the swappable `DeltaDFn` seam.
- **B-4:** In the bridge, run stage-2 (`confirm_unique_leader`) and stage-1 in parallel; act on whichever confirms a level first (stage-2's list provably contains the eventual leader, so a uniquely-confirmed stage-2 leader is safe to act on early). Keep stage-1 as fallback.

**Verify:** unit tests for the moved policy still pass; devnet confirms levels faster than stage-1 alone once stage-2 is live.

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
