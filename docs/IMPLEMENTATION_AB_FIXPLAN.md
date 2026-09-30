# Implementation A & B — Fix Plan (execution checklist)

> **Implementation A: CLOSED (2026-07-09).** Every A item is done, unit-tested,
> and committed. `docs/IMPLEMENTATION_A_PROGRESS.md` was deleted — its history
> lives in git log and in the "Closed" section below.
>
> **Implementation B: CLOSED for everything with a concrete fix.** All B-1
> through B-16 findings (original review + the 2026-07-10 fresh-eyes audit)
> are done. The two former ⚠️ design decisions were both decided 2026-07-16:
> B-11's real fix is spec'd ([`PRISM_RETARGET_SPEC.md`](./PRISM_RETARGET_SPEC.md),
> implementation deliberately deferred) and B-15 is implemented and
> live-verified (2026-09-30, with B-17 folded in). B-18 (fresh-node backfill
> from a pruning peer) was found live during that run and is open — see
> "Open work" below. `docs/IMPLEMENTATION_B_PROGRESS.md` was
> deleted for the same reason as A's tracker (fully superseded by this doc +
> git log; its milestone table lived on in `ARGONAUT_ARCHITECTURE_GUIDE.md`
> §3.9 anyway).

Read [`IMPLEMENTATION_AB_REVIEW.md`](./IMPLEMENTATION_AB_REVIEW.md) for the *why* behind any finding ID below. This file is the current *to-do list* — history of what was already fixed lives in git log, not here.

## Ground rules (do not skip)

1. **Live-verify, don't just unit-test.** `cargo test --workspace` is necessary but *not sufficient*. For any item that changes runtime behavior, build the release binary and drive a real devnet, because every bug that mattered on this project was found live, not by tests. Use `--network-backend libp2p` on every node (litep2p's request/response was `unimplemented!()`; B-8 worked around this in `prism-network`, but the flag is still the safe default).
2. **Commit after each completed item** (or each coherent group), with a message naming the finding ID. Branch off `Base` if not already on it.
3. **When a fix needs a decision you can't derive** (consensus-safety math, threat-model assumptions), stop and ask the user rather than guessing.
4. Update this doc as items land, so it stays the source of truth.
5. **Don't trust a piped `cargo test`'s exit code.** `cargo test --workspace 2>&1 | tail -N` reports `tail`'s exit code, not `cargo test`'s — reads as `0` even when a test failed. Check the actual `test result: ... FAILED` lines, or redirect to a file and check `$?` there.

---

## Closed (code + unit-tested, and live-verified where runtime behavior changed — see git log for full detail)

**Original review (B-1..B-9, AF-1..AF-5, A-1/A-3/A-4):** B-1+AF-4 (deterministic bridge timestamp), A-1 (sr25519 behind `dev-signatures`, out of production wasm), B-2 (difficulty retargeting), AF-1 (event-driven bridge wakeup, **~44ms confirm→import measured live 2026-07-10**), AF-2 (confirmation policy moved to `prism-ledger`), B-7+AF-5 (restart correctness — full replay + ledger recovery), B-9 (two-honest-miner stall root-caused: exact-tie artifact at small `m`, not a protocol bug), B-6 (initial sync/backfill), B-5 (per-chain genesis hashes), B5 pruning + `--prism-prune`/`--prism-k-prune`, B-8 (litep2p sync port), A-3 (registry-mode transfer tooling), A-4 + its auto-register-on-bootstrap remainder (`CheckFalconSignature::Bootstrap`, `runtime/src/extensions.rs`).

**2026-07-10 fresh-eyes audit (B-11..B-16):** B-11 interim fix (retarget disabled by default, `Params::retarget_enabled`), B-12 (stage-2 gated off by default behind `--prism-stage2`, `ConfirmationParams::stage2_enabled`), B-13 (orphan pool bounded at 10,000 with LRU-style eviction, `PrismChain::insert_orphan`), B-14 (subsumed by B-11's interim fix — moot while retarget is disabled), B-16 (consensus params moved from compile-time to chain-spec `properties`, `node/src/chain_spec.rs`).

**B-4 (stage-2 vs stage-1 latency) — done 2026-07-14 via deterministic unit tests, not live hardware.** Confirmation on this hardware is mining-dominated (~40s–3min/level even single-miner), which swamps the sub-second rule-latency difference in any live run. `client/prism-ledger/src/stage2_latency_tests.rs` isolates the *rule* latency by driving `PrismChain::insert_block` directly with controlled timestamps at m=100, mining removed from the picture: stage-2 confirms a 92/100 majority in round 1 while stage-1 never confirms within 200 rounds against 8 permanently-silent chains (unanimity, see B-15); against merely-slow stragglers (vote at round 30 instead of never), stage-2 still confirms round 1 vs. stage-1's round 36 — a 35-round head start even when every chain eventually votes.

**B-3 (practical confirmation margin) — derived 2026-07-14, not wired in as the default.** The paper's exact `δ_d`/`k_min` (Appendix E.1 eq. 31, `paper_k_min`/`paper_delta_d`) is an asymptotic-proof artifact — thousands-of-blocks-deep for any sane β, would make stage-2 *slower* than stage-1. Instead, `confirmation::single_check_margin(depth, m, beta, eps_target)` re-derives a real, usable margin directly from the paper's own Chernoff bound (eq. 32) for a single confirmation check rather than the paper's union-bound-over-all-pairs derivation. Monte Carlo cross-validated (200k trials × 4 (m,k) combinations, `single_check_margin_empirically_holds_at_several_m_k_combinations`, 330s runtime, all within the expected slack). Exposed via `example_single_check_confirmation_params_beta025_eps1e6()` — a ready-to-use non-placeholder `ConfirmationParams` — but `ConfirmationParams::default()` still uses the placeholder. See B-15 in "Open work" for why wiring this in as the operational default is a still-open design decision, not a mechanical follow-up.

**B-10 (docker-compose devnet artifact) — done 2026-07-14, real build + real 3-node live run, two real bugs found and fixed along the way.** Replaced the unmodified `substrate-node-template`/`paritytech/ci-linux` boilerplate with a real multi-stage `Dockerfile` (builder + slim runtime) and a 3-service `docker-compose.yml` (node1 archive/bootnode, node2 `--prism-prune`, node3 fresh non-mining backfiller), sharing one image via a YAML anchor, with committed devnet-only node keys for fixed PeerIds.
- **Bug 1 — toolchain skew broke the wasm link.** The floating `rust:1-bookworm` tag resolved to rustc 1.97.0 inside the container, vs. 1.94.0 on the host (which had built this exact runtime successfully all session). The newer `rust-lld` treated every host-function extern (`ext_storage_get_version_1`, `ext_falcon_crypto_verify_version_1`, ...) as an unresolved symbol instead of a wasm import, failing the runtime wasm link with "undefined symbol" for every single host function. Fixed by pinning `FROM rust:1.94.0-bookworm` — bump only in lockstep with a verified host-toolchain bump.
- **Bug 2 — `/data` permission denied.** The image runs as non-root `argonaut` (uid 1000) but never created/chowned `/data` before the `USER` switch. Docker seeds a fresh named volume's initial ownership from whatever's at that mount path in the image; since nothing was there, the volume came up root-owned and RocksDB's `--base-path` failed with `Permission denied` on first `docker compose up`. Fixed with `RUN mkdir -p /data && chown -R argonaut:argonaut /data` before `USER argonaut`.
- **Live verification (post-fix):** brought up node1+node2, both mined and converged to an identical `best` hash with zero errors/panics/`SortitionMismatch`; started node3 fresh afterward and it logged `prism: backfill: 102 unknown hash(es)` (real B-6 DAG backfill, not just Substrate's own block sync) before converging to the same byte-identical tip as the other two. To keep this bounded in wall-clock time, the run used a hand-lowered `prismDifficultyShift` in a `build-spec`-generated chain-spec JSON loaded via `--chain <path>` (properties don't affect the genesis hash, so this is fully compatible with the same on-disk state as the real default) — the shipped `docker-compose.yml` itself still uses the real, unmodified default difficulty (shift 24) via the built-in `--chain local` preset; nothing about the artifact itself was changed for this. This same run also closes B-11 interim's and B-16's outstanding live-verification gaps: retarget stayed off throughout with zero `SortitionMismatch`, and the chain-spec-driven difficulty override was visibly picked up (mining rate changed with the override, proving `prism_params_from_properties` reads it end-to-end on a live node) — B-16's original "verify: a chain spec with a non-default value produces different behavior" ask, satisfied with `prismDifficultyShift` instead of `m`.
- **Note for later, not a bug:** an even more aggressive difficulty (shift 14) produced thousands of accepted blocks over several minutes with zero confirmed levels, unlike shift 20 which confirmed quickly. Plausibly a reorg-storm interaction between very-high block-arrival-rate and stage-1's unanimity requirement (more near-simultaneous forks per voter chain than the network can settle between checks) — not root-caused further, just noted here as a live data point relevant to B-15's design work below.

**Deliberately not "fixed," by design, not open work:**
- B5 pruning's two accepted bounded leaks (documented in `PrismChain::prune`'s doc comment): abandoned-fork `voter_meta` predating the frontier isn't swept, and non-canonical sibling proposers at a confirmed level keep their bodies. Both would need a reverse index this port doesn't maintain.

---

## Open work

### B-11 real fix — decided 2026-07-16: spec written, implementation deliberately deferred

The design is written up in [`PRISM_RETARGET_SPEC.md`](./PRISM_RETARGET_SPEC.md) (epoch-based difficulty derived from the canonical confirmed-leader sequence, one-epoch activation lag, MTP timestamp clamping folding in B-14, cumulative-work fork choice, position-bound epoch declaration, rate floor from the shift-14 finding). **Do not implement until a multi-machine testnet exists to soak it** — the user approved this as spec-only; the interim fix (fixed chain-spec difficulty, `retarget_enabled: false`) covers every current deployment. When picking it up, the spec's "hard parts" section is the checklist; every item there is load-bearing.

### B-15 — DONE 2026-07-28, live-verified 2026-09-30 (code + `cargo test --workspace` green, 214 tests).

Implemented as specified below, with one **unplanned consensus-safety fix folded in** (found while reviewing the finished B-15 diff, see B-17 immediately after this section — do not read B-15 as landed without it). Summary of what shipped:

- `MarginPolicy` enum replaces the bare `DeltaDFn` in `ConfirmationParams`; `Default` is now `{ k_min: 1, margin: SingleCheck { beta: ASSUMED_ADVERSARY_FRACTION, eps_target: 1e-6 }, stage2_enabled: true }`.
- `prismBetaPercent`/`prismEpsExponent` added to the chain-spec properties, **and clamped in `node/src/service.rs`** (β to ≤49%, ε exponent to 1..=300) — neither `gamma()` nor the `as i32` cast is safe outside its intended domain, and `prism_params_from_properties` deliberately falls back rather than erroring, so an out-of-range JSON value arrives intact. Out-of-range values log a warning and clamp toward *more* conservative, never less.
- `--prism-stage2` → `--prism-no-stage2`, threaded as a whole `ConfirmationParams` struct into `run_prism_bridge`. `scripts/docker-entrypoint.sh`'s `PRISM_STAGE2` opt-in became `PRISM_NO_STAGE2` opt-out (the script still passed the removed flag — it was missed by the original step 4).
- Test triage went as predicted except for the two tests named under B-17.

**Live verification (2026-09-30):** native 3-node run mirroring the compose topology (node1 archive miner, node2 `--prism-prune --prism-k-prune 3` miner, node3 fresh non-miner joined ~6 min later), `--network-backend libp2p`, shift-20 chain spec as in B-10 (the docker image predated B-15, so a native release build was used instead of a 20–40 min image rebuild). To make criterion (c) observable, two log lines were added: `prism: confirmation: stage-2 enabled (beta=25%, eps=1e-6)` at startup (`service.rs`, shows chain-spec β/ε are read) and a per-level `level N confirmed via stage-1|stage-2` debug line in `PrismLedger::advance_with_params`. Result over ~25 min: (a) node1/node2 each confirmed 57 levels, all three nodes on an identical finalized `#57 (0x56e1…223f)`; (b) zero `SortitionMismatch`/panics on any node; (c) stage-2 enabled on every node, fired **0** times, stage-1 fired all 57 on each miner — exactly the expected shallow-depth behavior. Node3's own ledger confirmed nothing — that's a pre-existing B-6 gap unrelated to B-15, see B-18.

### B-17 — `confirm_unique_leader` was missing eq. (35)'s private-block bound (found + fixed 2026-07-28)

`confirm_proposer_list` gates on `best_lower > v_bar_private` where `v_bar_private = m − Σ lower bounds` (eq. 35: the votes a *withheld* proposer could still collect, since it's absent from `proposers_at_level` by construction). `confirm_unique_leader` never computed that term — it compared a candidate only against *visible* rivals, and with a single visible proposer `rivals_max_upper` degenerated to `max().unwrap_or(0)`, so **any** lower bound ≥ 1 "uniquely" confirmed the level.

Why it mattered enough to fix before closing B-15: the bridge imports every confirmed level `finalized: true` with no reorg path, and B-15 is precisely the change that promotes stage-2 from dormant (`stage2_enabled: false` since B-12) into the default finality path. The regime it fires in — few chains voted, votes deep — is the silent-chain backstop case B-15 exists to serve.

Fix: `(lo > rivals_max_upper.max(v_bar_private))`. Regression test: `confirmation/tests.rs`'s `a_unique_leader_is_never_confirmed_while_a_private_proposer_could_still_beat_it`, which pins the load-bearing invariant that the two rules can never disagree about whether a level is confirmable at all.

**Two existing tests were passing *because of* this bug** and were re-sized (user sign-off 2026-07-28), not deleted — each still proves exactly what it was written to prove:
- `honest_split_tests::any_real_skew_in_an_honest_split_still_resolves_via_stage_2`: m=4/3-1 → m=8/7-1. At m=4 a 3-vote majority's lower bound tops out at `floor(3 − margin) = 2`, exactly `v_bar_private = 4 − 2` — correctly unconfirmable at *any* depth.
- `adversarial_tests::persistent_balancing_attacker_stalls_stage1_alone_but_not_the_system`: m=4 with 1 attacker chain → m=8 with 3. Both ends of that test constrain the sizing: a lone dissenter at m=8 leaves 7 of 8 chains locking in p1, which is genuinely decisive, so stage-2 would (correctly) confirm *before* the attack starts and the test's first assertion would fail. 5-3 gives `lo(p1)=4` vs `v_bar_private=4` while the attacker feints, then `v_bar_private` drops to 2 once the attacker's own votes lock in — which p1's 4 clears.

### B-18 — a fresh node that backfills from a pruning peer never gets the pruned history, so its own ledger never confirms (found live 2026-09-30, OPEN)

Found during B-15's live run. Node3 (fresh, non-mining) happened to open its first Prism substream with node2 (the `--prism-prune` peer) and backfilled only 324 blocks; node1 had ~1,886 blocks of history at that point, and node3 ended up missing 1,612 of them. Its voter chains are therefore disconnected from genesis, stage-1 unanimity can never hold, and its `PrismLedger` confirmed **zero** levels even after a restart (B-7 recovery moved the cursor to the Substrate best, #33, and it still confirmed nothing through #57). It stayed on the correct finalized tip only because Substrate block sync + `PrismBridgeVerifier` import the other nodes' bridge blocks — its own Prism view never independently confirms anything. The B-10 run's "node3 converged" was very likely this same situation, unnoticed because there was no per-level confirmation log then.

Mechanism (`client/prism-network/src/wire.rs`): `backfill` drops a hash after `MAX_ROUNDS_MISSING` rounds when the peer never delivers it (a pruned peer never will), and never retries it against another peer. The 30s resync in `run` re-seeds only from each peer's *frontier* (`voter_tips`/`unreferenced_*`) filtered by `has_block` — once gossip has delivered the tips, the seed is empty, so the missing deep ancestors are never re-requested from the archive node.

Consequence: harmless while any node's bridge is producing blocks for it to sync, but such a node can never independently verify finality, and if it ever did confirm a level it would build on a best block it didn't derive. Candidate fix (not yet decided): track orphan-pool missing parents and re-request them from *all* peers on the resync tick (not just the peer that failed), and/or have pruning peers advertise their prune frontier so backfill prefers archive peers. Needs a live repro that the fix makes node3 print `confirmed via stage-1`.

### B-15 — decided 2026-07-16 (user sign-off): make the derived margin the default; EXECUTE (Sonnet-ready) — original plan, kept for reference

**Decision:** keep stage-1 (`confirmed_leader_at_level`) unanimous and untouched — it stays the fast path. Flip stage-2 to **on by default with B-3's real margin** (`single_check_margin`), replacing the placeholder in the operational path. β and ε become chain-spec properties.

**Corrected framing (matters for expectations, discovered 2026-07-16 re-reading the margin's own docs):** at m=100 the derived margin is *vacuous below ~400-deep votes* (`ρ≥1`; the curve self-gates — see `example_single_check_confirmation_params_beta025_eps1e6`'s doc comment). So with the real margin, stage-2 is **not** a latency win at m=100 — it's a provably-safe *eventual backstop*: when stage-1 stalls forever on silent chains (the B-15 hole), stage-2 confirms once the surviving chains' votes get a few hundred blocks deep (hours, not never). When stage-1 works, it fires first and stage-2 costs nothing — the existing race in `advance` already picks whichever confirms first. The B-4 tests' "round-1 stage-2 confirmation" came from the *placeholder* margin; do not expect that from the real one. (Faster stage-2 at fixed ε needs larger m or a smaller assumed β — both chain-spec choices now.)

**Steps (restate ground rules; batch the edits, one test pass at the end):**

1. `client/prism-ledger/src/confirmation.rs`: replace `pub delta_d: DeltaDFn` in `ConfirmationParams` with a policy enum so runtime β/ε can parametrize it (a bare `fn` pointer can't capture chain-spec values):
   ```rust
   pub enum MarginPolicy {
       Custom(DeltaDFn),                            // test hook; placeholder_delta_d lives on here
       SingleCheck { beta: f64, eps_target: f64 },  // B-3 derived margin
   }
   ```
   `lower_bound_from_depths` computes the margin via `match`. New `Default`: `{ k_min: 1, margin: SingleCheck { beta: ASSUMED_ADVERSARY_FRACTION, eps_target: 1e-6 }, stage2_enabled: true }`. Rewrite the `stage2_enabled` field's doc comment — its current text ("off by default", placeholder rationale) becomes false with this change. Update `example_single_check_confirmation_params_beta025_eps1e6` to construct via the enum (it stops needing its nested `fn`).
2. **Test triage, explicit:** shallow unit tests are naturally unaffected (the real margin is vacuous at their depths, and stage-1 fallback still fires), but any test that *specifically exercises stage-2 confirming at shallow depth* must pin `margin: MarginPolicy::Custom(placeholder_delta_d)` with a one-line comment that it tests the rule structure, not the calibrated margin: the 4 `confirm_unique_leader` tests in `confirmation/tests.rs`, both `stage2_latency_tests.rs` tests, `persistent_balancing_attacker_stalls_stage1_alone_but_not_the_system`, `any_real_skew_in_an_honest_split_still_resolves_via_stage_2`. Expect exactly these; investigate any *other* failure as real.
3. `node/src/chain_spec.rs`: add `prismBetaPercent` (u64, default 25) and `prismEpsExponent` (u64, default 6 ⇒ ε=1e-6) to `prism_property_keys`/`PrismChainSpecParams`/`prism_properties`/`prism_params_from_properties`, mirroring the existing four properties (integers, not floats, for JSON robustness). Extend the 4 existing chain_spec tests to cover them.
4. CLI: replace `--prism-stage2` with `--prism-no-stage2` (repo is pre-release; the old flag is one day old — swap it, don't alias). `service.rs` builds the full `ConfirmationParams` from chain-spec β/ε + `stage2_enabled: !cli.prism_no_stage2` and passes the *struct* into `run_prism_bridge` (replacing today's lone `stage2_enabled: bool` parameter and the bridge's internal `ConfirmationParams` construction).
5. **ε sizing note (put in `confirmation.rs`'s module doc):** ε is a per-check permanent-split probability (the bridge finalizes with no reorg path). At 1e-6/check it's fine for a devnet; anything holding real value should move `prismEpsExponent` to ≥9 (the margin floor grows only logarithmically: `−ln(ε)/m` = 13.8%→20.7% at m=100 going 1e-6→1e-9). Optional hardening, separate commit if done: skip re-evaluating stage-2 for a level unless the DAG actually changed (monotone insert counter on `PrismChain`, last-probed marker in `PrismLedger`) — tightens the union-bound constant, doesn't change the exponent; don't block on it.
6. **Verify:** `cargo test --workspace` green; then re-run the B-10 3-node compose devnet once and confirm (a) levels still confirm (stage-1 path, unchanged latency), (b) zero `SortitionMismatch`/panics, (c) grep shows stage-2 evaluated without firing at shallow depth. The definitive stage-2-backstop live test (silence 8 chains, wait hours) is impractical here; the Monte Carlo + `stage2_latency_tests` (pinned to `Custom`) remain its guards.

### Backlog — as convenient, no correctness impact

- **AF-3:** finer-grained DAG concurrency (per-voter-chain locks or actor model). Deliberately skipped — its own text says "only when measured to matter," and nothing has measured the global `Mutex` to matter yet.
- **Full network-level adversarial run:** the adversarial harness (`client/prism-ledger/src/adversarial_tests.rs`) drives `PrismChain`/`PrismLedger` directly with attacker-chosen blocks — but nothing has run an actual multi-node adversarial scenario with real gossip/timing the way the honest-case live devnets have been run.
