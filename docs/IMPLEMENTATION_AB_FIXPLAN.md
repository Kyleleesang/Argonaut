# Prism consensus (Implementations A & B) — open work

**Implementation A is closed. Implementation B is closed except for the items
below.** Everything else (A-1..A-4, B-1..B-17, AF-1/2/4/5) is done,
unit-tested and live-verified. Its history is in git log: search commit
messages for the finding ID, e.g. `git log --grep B-15`. The original review
doc (`IMPLEMENTATION_AB_REVIEW.md`) and the per-implementation progress
trackers were removed once their findings closed. Code comments that cite
"the fixplan's B-n entry" for a closed item refer to that history.

## Ground rules (still apply to anything below)

1. **Live-verify, don't just unit-test.** `cargo test --workspace` is necessary but not sufficient. For any runtime-behavior change, build the release binary and run a real multi-node devnet: every bug that mattered on this project was found live. Use `--network-backend libp2p` on every node.
2. **Commit after each completed item**, with a message naming the finding ID.
3. **When a fix needs a decision you can't derive** (consensus-safety math, threat-model assumptions), stop and ask the user.
4. **Don't trust a piped `cargo test`'s exit code.** `| tail -N` reports `tail`'s exit code. Redirect to a file and read the `test result:` lines.
5. **Useful live signals:** `prism: confirmation: stage-2 enabled (beta=…, eps=…)` at startup, and `level N confirmed via stage-1|stage-2` (debug, target `prism`) per confirmed level. A node whose ledger is healthy prints the second line; a node that only follows via Substrate sync does not. That difference is how B-18 was found.
6. **Devnet recipe:** use `build-spec --chain local`, set `properties.prismDifficultyShift` to 20, and load the result with `--chain <path>`. Properties don't change the genesis hash. This gives confirmations within minutes. Shift 14 is too aggressive (a reorg storm with zero confirmations); the shipped default of 24 is slow.

---

## B-18 — a fresh node backfilling from a pruning peer never fetches the pruned history (found live 2026-09-30) — BLOCKED on R-1/R-2, rework per N-6

> The 2026-09 design review ([DESIGN_REVIEW_2026-09_SPEC.md](./DESIGN_REVIEW_2026-09_SPEC.md)) found that this fix's acceptance run would trigger R-2 (the bridge building over sync-delivered blocks). It also found that resending orphan refs to pruning peers every tick stalls the network worker (N-1/N-6). The uncommitted `orphan_missing_refs` work is kept, but must not land before R-1/R-2.

**Symptom.** In a 3-node devnet (node1 archive miner, node2 `--prism-prune` miner, node3 fresh non-miner), node3's first Prism backfill went to node2 and pulled only 324 blocks. Node3 ended up missing 1,612 of the ~1,886 historical blocks. Its voter chains stayed disconnected from genesis, so stage-1 unanimity could never hold. Its own `PrismLedger` confirmed **zero** levels, including after a restart. It stayed on the right finalized tip only because Substrate block sync imports the other nodes' bridge blocks. It never verified finality itself.

**Mechanism** (`client/prism-network/src/wire.rs`):
- `backfill` drops a hash after `MAX_ROUNDS_MISSING` rounds when the peer never delivers it, and a pruned peer never will.
- The 30s resync re-seeds only from each peer's *frontier*, filtered by `PrismChain::has_block`, and `has_block` counts orphan-pool entries. Once gossip delivered the tips as orphans, the seed was empty forever, so the orphans' missing ancestors were never requested from the archive node.

**Fix (being implemented).** Add `PrismChain::orphan_missing_refs()`, which returns the unresolved dependencies of every orphan, using the same readiness rules as `drain_orphans` and excluding anything known, orphaned or pruned. `request_status_and_maybe_backfill` adds these to its seed, so every 30s resync re-requests them from every peer until an archive peer serves them.

**Acceptance:** repeat the 3-node run above; node3 must print `level N confirmed via stage-1` on its own and match the miners' finalized hash.

## B-11 real fix — difficulty retargeting: spec'd, implementation deliberately deferred

The design is in [`PRISM_RETARGET_SPEC.md`](./PRISM_RETARGET_SPEC.md). **Do not implement until a multi-machine testnet exists to soak it** (user decision, 2026-07-16). The interim fix (fixed chain-spec difficulty `prismDifficultyShift`, `retarget_enabled: false`) covers every current deployment. The spec's "hard parts" section is the checklist when picking it up.

## Backlog — no correctness impact today

- **AF-3: finer-grained DAG concurrency.** The whole DAG sits behind one `Arc<Mutex<PrismChain>>`, which is taken by every block insert, every miner template rebuild and every bridge wakeup. Fine at devnet rates, but at `m=100` with the paper's block rates it serializes away the parallelism the voter chains exist for. The right direction is per-voter-chain locking or an actor that owns the DAG. Do this only once it's measured to matter.
- **AF-4 leftover: automate the determinism invariant.** "Every honest node derives the identical Substrate block from the same leader sequence" has been verified live but has no standing test. Add a test proving that two `PrismChain`s with the same accepted-block set yield byte-identical bridge blocks for a level.
- **B-7/AF-5 leftover: restart cost is O(history).** The declared index column families are unpopulated, so restart replays every historical block, and `build_ledger_delta_for_leader` walks references via DB reads instead of in-memory `proposer_meta`. Pruning bounds this on pruning nodes, but archive nodes still pay it in full. This is a scale concern.
- **Full network-level adversarial run.** `client/prism-ledger/src/adversarial_tests.rs` drives `PrismChain`/`PrismLedger` directly. Nothing has yet run a real multi-node adversarial scenario with real gossip and timing.

## Operating notes that outlive their findings

- **Stage-2 at `m=100`** (B-15): with the real derived margin (`single_check_margin`, β = `prismBetaPercent`, ε = 10^-`prismEpsExponent`), stage-2 is vacuous below ~400-deep votes. It is an eventual safety backstop for stage-1's unanimity weakness (a silent voter chain), not a latency win. Live runs show stage-1 confirming everything at shallow depth, which is expected. Anything holding real value should raise `prismEpsExponent` to ≥9, because the bridge finalizes with no reorg path.
- **Every devnet needs at least one archive (non-pruning) node.** Backfill has to fetch full history from somewhere; see B-18.
