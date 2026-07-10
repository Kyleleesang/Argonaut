# Implementation A & B — Deviation & Correctness Review

A standalone audit of Implementations A (Falcon signatures + registry) and B (Prism consensus) against the [`ARGONAUT_ARCHITECTURE_GUIDE.md`](./ARGONAUT_ARCHITECTURE_GUIDE.md) and the source paper ([`Prism.pdf`](./Prism.pdf)). **This document only diagnoses and prescribes — no code was changed to produce it.** Each finding says *what* deviates, *where*, *why it matters*, and *how to fix it*, so a later session can act on them in priority order.

Companion to [`IMPLEMENTATION_B_PROGRESS.md`](./IMPLEMENTATION_B_PROGRESS.md) (Implementation A's own tracker, `IMPLEMENTATION_A_PROGRESS.md`, was retired 2026-07-09 once every A item closed — see the fixplan doc's banner), which already record most of these as "out of scope" / "known limitation." This review pulls them into one place, adds two that the progress docs *don't* yet flag (B-1 bridge timestamp; A-1 sr25519 now in wasm), and — per the specific request that prompted it — spells out the fix for the block-difficulty deviation.

> **Status as of 2026-07-09:** this doc is the original diagnosis and is left unmodified in its "why"; **[`IMPLEMENTATION_AB_FIXPLAN.md`](./IMPLEMENTATION_AB_FIXPLAN.md) is the authoritative source for what's actually been fixed since.** Each finding below now carries a one-line `Status` bullet pointing at the fixplan item that addressed (or deliberately deferred) it, so this document doesn't read as still-all-open. Quick summary: **resolved** (code-complete, live-verification still pending per the fixplan's own ground rules) — A-1, A-3, A-4 (size test + auto-register remainder — **Implementation A is now fully closed**), B-1/AF-4, B-2, B-5, B-6, B-8, AF-1, AF-2, B-4; **root-caused but not "fixed"** (behavior explained/proven, not a bug) — B-9; **partially addressed** — B-3 (paper's exact formula ported but not wired in as the default), B-7/AF-5 (a worse correctness bug than scoped was found and fixed; index-CF persistence itself still deferred); **still open, untouched** — A-2 (n/a, no fix needed), B-10, AF-3.

## Severity legend

- **H (High)** — a correctness or consensus-safety gap; can produce divergent/wrong state between honest nodes, or a real security hole. Fix before any multi-node or non-throwaway use.
- **M (Medium)** — deviates from the paper/guide in a way that matters for real deployment but not for a single-machine devnet; safe to defer, unsafe to forget.
- **L (Low)** — cosmetic, scope-cut, or tooling-gap deviation with no correctness impact.

---

## Implementation A findings

### A-1 (H → M): the sr25519 dev escape hatch is now compiled into the production wasm runtime

- **Where:** `primitives/falcon/src/lib.rs` — `ArgonautSignature::Sr25519(..)`.
- **Deviation:** Guide §2.1/§2.2 specify sr25519 as an *optional, dev-only* variant "feature-gated out for release builds." During B3 this session, the `#[cfg(feature = "std")]` gate was **removed** from this variant to fix a genuine wasm/native SCALE wire-format mismatch (the gate made native and wasm disagree on the enum's variant count, so every sr25519-signed tx was undecodable on-chain — see B3 progress notes). The removal was correct *as a bug fix*, but it overshot: the variant is now unconditionally present in every build, including the release wasm runtime. A dev-only signature scheme (sr25519, non-post-quantum — the whole point of Argonaut is Falcon's PQ security) is now a permanently-accepted signature type in production. Anyone can transact with a classical sr25519 key, defeating the PQ threat model for those accounts.
- **Why it matters:** Security-relevant. It's the difference between "dev accounts use sr25519 during bring-up" and "the mainnet runtime forever accepts non-PQ signatures."
- **Fix:** Put the variant behind a real Cargo *feature* (e.g. `dev-signatures`), propagated identically to **both** the wasm and native builds of the crate (so the enum shape stays in sync — that's what the `cfg(std)` gate got wrong), and *off* by default. `runtime/Cargo.toml` enables it only for dev/test profiles; production runtime builds omit it. The `impl Verify` match arm and `ArgonautSigner::Sr25519` gate move to the same feature. Downgrades to M once this is done. Until then it's H because it's a live hole in the release artifact.
- **Status:** RESOLVED (fixplan item 2). `dev-signatures` feature added to `primitives/falcon`, forwarded from `runtime`, unconditionally enabled by `node`. Confirmed via `runtime/build.rs`'s wasm build being a `--no-default-features` subprocess, so the feature never reaches the wasm blob. Live confirmation that a production wasm build actually rejects an sr25519-signed extrinsic is the one remaining open step.

### A-2 (L): `argonaut-sign` binary folded into the node CLI as `falcon-sign`

- **Where:** `node/src/falcon_sign.rs`.
- **Deviation:** Guide §2.4 asks for a separate `argonaut-sign` binary/workspace member; it's a node subcommand instead. Already self-documented in the file and A-progress §5.
- **Fix:** None needed — functionally equivalent, explicitly permitted by the guide's own §8 "adapt the internals, noting the deviation." Listed only for completeness.

### A-3 (L): registry-mode signing is not covered by any tooling

- **Where:** `node/src/falcon_sign.rs` covers bootstrap mode only.
- **Deviation:** No CLI/script path assembles a full `General`-preamble registry-mode extrinsic (nonce/era/tip/genesis) end-to-end. Guide §2.4 implies a signer that can produce a submittable registry-mode tx.
- **Why it matters:** Registry mode is the whole point of Impl A (the ~897-byte/tx saving), but it can't be *used* end-to-end from tooling yet — only its `TransactionExtension::validate()` is exercised, in tests. `node/examples/submit_transfer.rs` (added in B3) is a working template for the transaction-builder plumbing; a registry-mode variant of it would close this.
- **Fix:** Extend `submit_transfer.rs` (or a new example) to build a `Preamble::General` extrinsic carrying `CheckFalconSignature::RegistryMode { sig, account }` instead of a `Signed` preamble.
- **Status:** RESOLVED (fixplan). `node/examples/submit_transfer_registry.rs` builds a full `General`-preamble extrinsic with `CheckFalconSignature::RegistryMode` from an existing keystore.

### A-4 (L): missing acceptance tests (size assertion, auto-register)

- **Where:** `pallets/falcon-registry`, `runtime/src/extensions.rs`.
- **Deviation:** Guide §2.5 point 3 (registry-mode encoded extrinsic ≤ 850 bytes — asserted in a test) and §2.3's optional auto-register-on-first-bootstrap-tx are both unimplemented. Both already flagged in A-progress "Remaining / follow-ups."
- **Fix:** Size test is trivial (`encode().len()` on a built registry-mode vs bootstrap-mode extrinsic, assert the delta ≈ 897). Auto-register is a config-gated branch inside `CheckFalconSignature`'s currently-no-op `Disabled`/bootstrap path.
- **Status:** Both RESOLVED (fixplan). Size test — `runtime/src/tests.rs`, real Falcon-512 keypairs, registry-mode ≤ 850 bytes. **Auto-register — done 2026-07-09**, via a new `CheckFalconSignature::Bootstrap { sig, pk }` variant + a config-gated `prepare()` (`AUTO_REGISTER_ON_BOOTSTRAP` const). This was the last open Implementation-A item; **Implementation A is now fully closed.**

---

## Implementation B findings

### B-1 (H): bridge block timestamp is non-deterministic → Substrate chains are NOT byte-identical across nodes

- **Where:** `node/src/bridge.rs::build_and_import_block` — `sp_timestamp::InherentDataProvider::from_system_time()`.
- **Deviation:** Guide §3.1 (binding): "every honest node must derive the identical Substrate block from the same leader sequence." The B3 acceptance criterion (§3.9) is explicitly "all nodes' Substrate chains byte-identical." But the bridge stamps each block with the **local wall-clock time of whichever node confirmed the level**, via `from_system_time()`. Two honest nodes confirming the same Prism leader level a few hundred ms apart will produce blocks with different `timestamp` inherents → different `extrinsics_root` → different block hashes. The chains diverge immediately, byte-for-byte, even with zero adversary and identical transaction sets.
- **Why it matters:** This is the single most important correctness gap in B. It directly violates the "never reorgs / deterministic output of Prism" design contract, and it means the B3 acceptance criterion is **not actually met across nodes** (this session's live test verified block *production* and the digest on one node, then ran single-miner after an unrelated crash — the two-node byte-identical diff was never actually performed, which is how this slipped through).
- **Fix:** Derive the timestamp deterministically from Prism data, not local time. The confirmed leader's `PrismBlock` already carries `SuperHeader.timestamp_ms`; use that (or, more robustly, a deterministic function of the confirmed level's blocks — e.g. the median timestamp of the leader + its referenced tx blocks, which resists a single miner lying). Feed that fixed value into a `sp_timestamp::InherentDataProvider::new(ts.into())` instead of `from_system_time()`. Then add the missing acceptance test: run 2 nodes, confirm `chain_getBlockHash(n)` is identical on both for every `n`.
- **Status:** RESOLVED (fixplan). `ConfirmedLevel` carries the leader's own `timestamp_ms`, fed via `InherentDataProvider::new`, clamped forward against on-chain `pallet_timestamp::Now`. **Live-verified 2026-07-07:** two-node devnet, byte-identical `chain_getBlockHash` for levels 0–5. This run also found and fixed a second, previously-undiscovered bug in the same area — see AF-4's entry below. **Still open:** the standing AF-4 choke-point assert/test (the property was verified live, but no permanent test encodes it).

### B-2 (H, paper-model): block difficulty is a hardcoded constant with no retargeting

**This is the deviation the review was requested to solve.**

- **Where:** `primitives/prism/src/lib.rs::Params::testnet()` (base values) and `node/src/service.rs` (the `>>7` devnet scaling). No `DifficultyApi`, no rolling-window retarget anywhere.
- **Deviation:** Paper §4.2/§4.5.1 does **not** treat difficulty as a free constant. It *derives* the per-voter-chain mining rate `f̄_v` from physical network parameters — bandwidth-delay product `C·D/B_v`, block size, and the target security parameter `ε` — subject to the stability constraint `f_p·B_p + m·f_v·B_v + f_t·B_t < C` (eq. 10). Guide §3.5 makes this concrete: "Retarget like Bitcoin (rolling window over superblock timestamps) — one shared retarget; thresholds are fixed *fractions* of the target so the split survives retargeting." The current code has *no* feedback loop at all: the target is a fixed empirical constant, so if hash rate changes (more miners, faster hardware), block rate drifts arbitrarily and never corrects — unlike Bitcoin, and unlike what the paper's security analysis assumes (the `ε`-security bounds are only valid at the calibrated `f_v`).
- **Why it matters:** The paper's safety/liveness guarantees are stated *as a function of* `f_v` sitting at its calibrated value. A drifting, un-retargeted rate means the actual `ε` is unknown and unbounded. Fine for a single-machine devnet (fixed, known hash rate); unsafe for anything multi-party or long-running.
- **How to fix (two viable routes; the guide endorses either):**
  1. **Client-side rolling-window retarget (simpler, matches guide §3.5's phrasing).** Keep difficulty entirely client-side (where all Prism consensus already lives). Maintain a rolling window of the last *N* superblock timestamps (`SuperHeader.timestamp_ms`); every *N* blocks, compute `new_target = old_target · (actual_elapsed / expected_elapsed)`, clamped per-step (Bitcoin uses ±4x) to resist timestamp manipulation. Crucially, retarget the **single combined target** only — `f_v : f_t : f_p` stay fixed *fractions* of it (as they already are in `Params`), so the sortition split is invariant under retargeting, exactly as the guide requires. Store the current target in `PrismDb` (a new small column, or reuse the metadata pattern) so it survives restart. `Params::testnet()` provides only the *initial* target and the fixed fractions.
  2. **On-chain `DifficultyApi`-backed retarget (more work, stronger determinism).** Expose a runtime API that returns the current target as a function of recent confirmed-block timestamps already in Substrate state; the miner reads it. Heavier (runtime involvement, and Prism blocks aren't Substrate blocks so the coupling is awkward), and not clearly better than (1) for this architecture. Route (1) is recommended.
- **Note:** This is a *pre-existing, already-documented* limitation (flagged in B0 and B2 progress notes), not something introduced this session. The two difficulty *bugs* fixed this session (the `m`/`f_v` mismatch and the overcorrected shift) were arithmetic errors *within* the empirical-tuning hack, not this deeper model deviation. Fixing B-2 (real retargeting) is what actually closes the gap with the paper; it supersedes all the empirical `>>N` tuning.
- **Status:** RESOLVED, route 1 (fixplan item 3). Rolling-window retarget (±4x clamp/step) added to `primitives/prism`, fed once per accepted block in `client/prism-chain`, persisted across restart. Two related staleness bugs (miner's and network worker's frozen `params` copies) found and fixed along the way. **Still open:** live confirmation that retargeting actually moves the observed block interval toward `target_interval_ms` on a running devnet — only unit-tested so far.

### B-3 (M, paper-model): stage-2 confirmation's `δ_d` is a placeholder, not the paper's derived constant

- **Where:** `client/prism-ledger/src/confirmation.rs::placeholder_delta_d`.
- **Deviation:** The confidence-interval *policy* (paper Def. 4.5, eqs. 33–36) is ported faithfully, but `δ_d` — the per-depth confidence margin the bounds depend on — is an explicit stand-in with the right qualitative shape, not the paper's exact closed form (which requires Appendix E.1's "typicality" lemma: a multi-page concentration bound over the Poisson voter-chain process, with constants `ρ_Δr`, `k_min`, and lemmas E.2–E.4). Already loudly flagged in-code and in B4 progress notes.
- **Why it matters:** The confirmation *mechanism* is correct and testable, but the numeric *safety margin* is not the paper's, so stage-2's actual error probability is unquantified. Safe as long as stage-2 isn't relied on (it currently isn't — the bridge uses stage-1 only; see B-4).
- **Fix:** Port Appendix E.1. This is real math-transcription work — see the [`reference-prism-paper`] memory for the PDF-extraction method (and the lesson that dense inline math should be copy-pasted from the PDF's text layer, not read off a rendered image). Keep the swappable `DeltaDFn` seam that's already there so only `placeholder_delta_d` needs replacing.
- **Status:** PARTIALLY ADDRESSED (fixplan item 5, B-3). Eq. (31)'s `k_min`/`δ_k` were ported faithfully as `paper_k_min`/`paper_delta_d` and unit-tested — but a real finding surfaced: `k_min` is in the *thousands* for any sane β (an asymptotic-proof artifact), so using it literally would make stage-2 strictly *slower* than stage-1, the opposite of the paper's intent. `ConfirmationParams::default()` therefore still deliberately uses the practical placeholder; the faithful port exists but isn't wired into anything live. Treat this finding as "the math is now available, not yet the operating default."

### B-4 (M): stage-2 confirmation is implemented but not wired into the live bridge

- **Where:** `node/src/bridge.rs::run_prism_bridge` calls only `PrismLedger::advance` (stage-1 k-deep).
- **Deviation:** B4 built `confirm_proposer_list` / `confirm_unique_leader` (stage-2, the *fast* path the paper's whole §4.5 exists to provide) but nothing calls them in production. The chain still confirms at Bitcoin-like latency (stage-1), not Prism's headline "propagation-delay-bounded" latency.
- **Why it matters:** It's the difference between "Prism-shaped code" and "Prism-speed chain." Not a correctness gap (stage-1 is strictly safe), purely unrealized performance. Gated behind B-3 anyway (don't trust stage-2 numerically until `δ_d` is real).
- **Fix:** Once B-3 lands, run both rules and take whichever confirms a level first (stage-2's list must contain the eventual leader, so it's safe to act on a uniquely-confirmed stage-2 leader before stage-1 unanimity). Keep stage-1 as the fallback/safety net.
- **Status:** RESOLVED (fixplan item 5, B-4). `PrismLedger::advance` now tries `confirm_unique_leader` (stage-2) first and falls back to stage-1 per level, on every bridge wakeup — no separate race logic needed since AF-1 made `advance` event-driven. Caught a real interaction: this made an existing adversarial liveness test fail *correctly* (stage-2 doing its job); the test was rewritten to check both stage-1-alone and the system as a whole. **Still open:** the plan's own live-devnet timing check ("devnet confirms faster than stage-1 alone") — only unit-tested so far.

### B-5 (M): single shared genesis sentinel instead of per-chain genesis hashes

- **Where:** `client/prism-chain` seeds every voter chain and the proposer tree from `H256::zero()` (`genesis_sentinel`).
- **Deviation:** Guide §3.2: geneses should be "hashed deterministically from `(chain_spec_genesis_hash, chain_id)` so every network has distinct Prism geneses." `primitives/prism::genesis_hash()` exists but is unused. Already flagged in B1 progress notes.
- **Why it matters:** Cross-network replay / chain-id confusion once more than one Argonaut network exists — a block mined on network X is structurally valid on network Y. Irrelevant for a single devnet.
- **Fix:** Seed `PrismChain::new` from `genesis_hash(chain_spec_genesis, chain_id)` per chain instead of the shared zero sentinel; thread the chain-spec genesis hash in from `node/src/service.rs`.
- **Status:** RESOLVED (fixplan). `Params::chain_spec_genesis` plumbed through `PrismChain`/`PrismMiner`/`node/src/service.rs`; `insert_block` rejects a block whose parent is some *other* chain's genesis. DB compat handled via purge-and-remine (hard error on genesis mismatch), not a migration. **Still open:** the live two-node mine-and-confirm + restart verification.

### B-6 (M): no historical backfill / initial sync for Prism state

- **Where:** `client/prism-network` — sync protocol only answers reactive orphan-fetch (`GetBlocks` for a referenced-but-unknown hash), no "sync from genesis / from height."
- **Deviation:** Guide §3.6 specifies an initial-sync strategy (request peers' voter tips + proposer best level, breadth-first backfill by parent refs). Only the orphan-resolution half exists. Already flagged in B2 progress.
- **Why it matters:** A fresh node, or one offline longer than peers keep relevant state orphan-reachable, silently ends up with a divergent (internally-consistent) DAG and never converges. Compounds badly with B5 pruning if that's ever added.
- **Fix:** Implement the §3.6 tip-exchange + BFS backfill. Non-trivial but self-contained in `client/prism-network`.
- **Status:** RESOLVED (fixplan). `SyncRequest::GetStatus`/`SyncResponse::Status` wire messages; `request_status_and_maybe_backfill` filters a peer's frontier through `PrismChain::has_block` and feeds unknowns into a batched (64/round), bounded-retry backfill queue. **Still open:** the live two-node backfill run (fresh node2 against a non-trivial history, including a stop/restart mid-backfill).

### B-7 (M): DAG indices are declared in the DB but never persisted (crash-consistency gap)

- **Where:** `client/prism-db` declares all the §3.3 column families (`PROPOSER_NODE_LEVEL`, `VOTER_NODE_CHAIN`, `GRAPH_*`, …) but `insert_block` only writes `BLOCKS` + `BLOCK_STATUS`; all traversal metadata lives in memory (`voter_meta`/`proposer_meta`).
- **Deviation:** Guide §3.3 wants `WriteBatch`-based atomic multi-CF commit ("a block must never be half-indexed"), and §3.9 lists "making `insert_block` atomic w.r.t. crash" as a known hard spot. Currently, on restart the entire DAG index is rebuilt from scratch by replaying `BLOCKS` — which works only because everything is in-memory and recomputed, but means there's no O(1) recovery and the declared index CFs are dead weight.
- **Why it matters:** Restart cost scales with total history (full replay), and there's no on-disk DAG state to prune against (blocks B5). Not a *correctness* bug today (replay is deterministic), but the guide's crash-consistency model isn't realized.
- **Fix:** Either (a) actually populate the index CFs inside `insert_block`'s `WriteBatch` and load from them on startup, or (b) formally document that Prism DAG state is memory-only + replay-recovered and delete the unused CF declarations. (a) is the paper-faithful route and a prerequisite for efficient pruning.
- **Status:** PARTIALLY ADDRESSED (fixplan item 6, B-7/AF-5) — scoped down after finding a *worse* bug than this finding anticipated: restart didn't just lack fast recovery, it silently re-confirmed every already-settled level from 1 and double-built Substrate blocks (a correctness bug, not a performance one). Fixed: `with_db` now full-replays every persisted block through `insert_block` before returning; `PrismLedger::recover` rebuilds the confirmed cursor + tx-dedup state from the Substrate client's own height. **Still open, now a pure scale concern:** the declared index CFs remain unpopulated; replay is O(total historical blocks) at every restart; `build_ledger_delta_for_leader` still traverses via DB reads rather than in-memory `proposer_meta` (AF-5 itself). Revisit before B5 pruning.

### B-8 (M): `litep2p` network backend crashes on Prism sync requests (upstream stub)

- **Where:** polkadot-sdk `stable2412` `litep2p/service.rs` — `NetworkRequest::request` is `unimplemented!()`. `client/prism-network` calls it.
- **Deviation:** Not a guide deviation per se — an upstream gap. But `litep2p` is the CLI's *default* backend, so any node run without `--network-backend libp2p` panics the moment it needs to fetch a missing Prism block. Found live this session.
- **Fix:** Port `client/prism-network`'s sync-request call from the async `request` wrapper to the lower-level callback-based `start_request` (which *is* implemented for litep2p), or pin `--network-backend libp2p` in docs/tooling until upstream implements it. Recorded in the [`project-implementation-b-status`] memory.
- **Status:** RESOLVED (fixplan). `client/prism-network/src/wire.rs` gained a free function `sync_request` using `start_request`/oneshot instead of the async `request` wrapper, so sync-protocol calls work under litep2p too. `--network-backend libp2p` is no longer a hard requirement, though it remains the tested default.

### B-9 (M): unresolved two-honest-miner confirmation stall

- **Where:** `client/prism-chain` fork-choice / honest-vote strategy interaction; observed live, not root-caused.
- **Deviation:** Two honest `--validator` nodes accepted 100+ blocks without stage-1 confirming a single level (single miner confirmed in ~60). Hypothesis: two miners independently mine the first proposer at a level, each votes its own locally-first-seen sibling, and since a chain's `unvoted_levels` marker is never revisited absent a *reorg*, votes can permanently split. Structurally the same as the balancing-attack liveness weakness the B4 harness exercises — but with *honest* miners.
- **Why it matters:** If real, honest multi-miner networks could stall stage-1 confirmation at small `m` — a liveness problem, not safety. Stage-2 (B-4) would likely paper over it, which is suggestive. Not confirmed as *the* cause.
- **Fix:** First, a deterministic repro (two in-process miners feeding one `PrismChain`, or a scripted two-template race). Then decide whether the honest-vote rule needs a tie-break/switch-toward-plurality mechanism, or whether stage-2 is the intended answer. Do **not** scale `m` back to 100 for multi-miner runs until this is understood.
- **Status:** ROOT-CAUSED, not a bug fix (fixplan item 8, B-9). Deterministic in-process repro built (`honest_split_tests.rs`): an *exact* even split between two honest miners is provably unresolvable forever — a structural fact (every quantity stage-2 computes for one side mirrors the other), not a numerical shortfall. Any real skew, however small, resolves via stage-2 within a bounded round count. Conclusion: don't scale `m` down to ~4 for devnet runs expecting fast guaranteed confirmation (an exact tie becomes likely at small `m`); at the paper's intended `m` (100+) a sustained exact tie is vanishingly improbable. No tie-break mechanism was built — not called for by the honest protocol, and this is a devnet-testing convenience concern, not a safety one.

### B-10 (L): docker-compose, peer-slot mirroring, weight tuning

- **Where:** various; all already flagged in B2 progress as out-of-scope.
- **Deviation:** 4-node convergence used plain OS processes, not the `docker-compose.yml` the guide's phrasing suggests; the Prism notification protocol uses independent peer slots instead of mirroring the default set (`add_peers_to_reserved_set`); bridge/ledger weights are nominal.
- **Fix:** Cosmetic/infra; do when handing the devnet to someone else. No correctness impact.
- **Status:** STILL OPEN (fixplan item 12). Not attempted.

---

## Fix order — what's actually left

All Implementation-A items (A-1, A-3, A-4 size test + auto-register) and B-3→B-4, B-5, B-6, B-8 are resolved (code + unit tests; see each finding's `Status` line above). **Implementation A has nothing left.** What remains, all Implementation B (or backlog):

1. **Live-devnet verification** of every "resolved" item above — only `cargo test --workspace` has run so far. See [`IMPLEMENTATION_AB_FIXPLAN.md`](./IMPLEMENTATION_AB_FIXPLAN.md)'s "Live-verification debt" list for the exact runs needed (two-node byte-diff, wasm rejecting sr25519 live, retarget convergence, restart recovery, pruning end-to-end with a fresh backfilling node, etc.). This is the real gate before treating this as an actual multi-party network.
2. **B-10** (docker-compose devnet artifact) — as convenient.
3. **AF-3** (finer-grained DAG concurrency) — deferred by design until measured to matter.
4. **Full network-level adversarial run** (real multi-node gossip/timing, not the single-process harness) — as convenient.

---

# Part 2 — Architectural fidelity assessment

The findings above are *point deviations* (a wrong constant, a missing gate). This part steps back and asks a different question: **does the shape of the code match the shape of the design?** Not "is this line right" but "did the implementation build the architecture the guide describes, with the right boundaries, data flow, and invariants." Grades here are about structure, not bugs.

## Overall verdict

**Strong structural fidelity, one broken invariant, and one piece of intended machinery that was built and then not connected.** The crate decomposition, the client-side-consensus / Substrate-as-output separation, and the determinism discipline are all genuinely faithful to §3.1's binding architecture — this is not a case of the code drifting into a different shape than the design. The defects are specific and localized: the pipeline's core determinism invariant has exactly one hole (the timestamp, B-1), and the event-driven confirmation→bridge wakeup the design implies was left as polling. Neither is a "wrong architecture"; both are "architecture 95% wired up."

## What matches the design well (and is worth *not* regressing)

These are the load-bearing design decisions the implementation got right — listed because a future refactor should preserve them, not just because they're praise.

- **Crate decomposition is 1:1 with the guide.** `prism-db` / `prism-chain` / `prism-miner` / `prism-network` / `prism-ledger` map exactly onto §3.3–§3.8, each owning what the guide assigns it. The dependency direction is clean (db ← chain ← ledger; miner and network depend on chain; nothing depends upward into the node). No god-crate, no circular deps. This is the single most important thing to have gotten right and it's right.
- **The §3.1 boundary is real, not notional.** Prism consensus genuinely runs client-side and the Substrate chain is genuinely downstream of it — `node/src/bridge.rs` is the *one and only* place the two worlds touch, and it's a one-directional funnel (confirmed leader level → one Substrate block). There's no back-channel where Substrate state leaks into Prism consensus. Deleting the B0 PoW chain (rather than leaving it alongside) was the right call: it removes any ambiguity about "which chain is authoritative."
- **The hardest specified code exists and is tested.** §3.9 names vote-rollback-on-voter-reorg and orphan/dependency resolution as *the* hard spots. Both are implemented as designed — the LCA-walk vote unwind/replay, and the "re-drain the orphan pool after every insert" discipline — and covered by the recount-invariant property test the guide explicitly asks for. This is the part most likely to have been faked with a happy-path stub; it wasn't.
- **Determinism discipline is mostly enforced by construction.** BuildLedger's ordering (DFS over refs + hash-dedup) and the bridge's skip-rule (apply-extrinsic against prior state) are deterministic functions of the confirmed DAG, exactly as §3.1/§3.2 require. Crucially, the local mempool only influences what a node *mines*, never what it *confirms* (confirmation reads gossiped, confirmed proposer refs) — so per-node mempool differences correctly don't cause chain divergence. That separation is subtle and the implementation gets it right everywhere **except the timestamp** (B-1).
- **The miner matches §3.5's shape.** ArcSwap template hand-off, dedicated OS threads (not async), queue-coloring, `try_mine` carrying `next_nonce` forward so the reset-to-zero stall is structurally impossible. Faithful.

## Architecture-level flaws (distinct from the Part 1 findings)

### AF-1 (M): the confirmation→bridge path was designed to be event-driven and shipped as polling

- **Design intent:** §3.8's bridge pseudocode is triggered "on new confirmed leader level ℓ." §3.4's `insert_block` returns `InsertOutcome { missing, may_confirm }` — `may_confirm` exists for exactly one purpose: to signal "a new level might now be confirmable, wake the ledger/bridge." The plumbing for an event-driven bridge was deliberately built into the core type.
- **What was built:** `may_confirm` is set correctly by `insert_block` (lib.rs:188, 207) — and then **consumed by nothing**. Neither the network worker nor the bridge reads it. `run_prism_bridge` instead spins a `tokio::time::sleep(250ms)` poll loop, locking `PrismChain` and re-running `PrismLedger::advance` every quarter-second regardless of whether anything changed.
- **Why it's an architectural flaw, not just an inefficiency:** (1) It's dead design intent — a field on the central `InsertOutcome` type that the whole system computes and nobody uses reads as a bug to future maintainers. (2) It injects up-to-250ms of avoidable latency per level, which directly undercuts the *entire point* of stage-2 fast confirmation (B-4): there's no reason to make confirmation propagation-delay-bounded and then gate it behind a 250ms poll. (3) It's needless lock contention on the single `PrismChain` mutex (see AF-3) — a busy-wait on a lock that's also on the hot path of every block insert.
- **Fix:** Wire `may_confirm` through. The network worker (which owns block insertion and already has channels) should push a lightweight "confirmation may have advanced" signal to the bridge over a `tokio` notify/channel whenever `insert_block` returns `may_confirm = true`; the bridge `await`s that instead of sleeping. Keep a long (e.g. multi-second) fallback tick only as a safety net. This is the design the `may_confirm` field was added for.
- **Status:** RESOLVED (fixplan item 4). A shared `Arc<tokio::sync::Notify>` replaces the 250ms poll loop; the bridge now `select!`s between the notify and a 5s safety-net fallback. **Still open:** live measurement of the actual latency improvement.

### AF-2 (L→M): stage-1 confirmation lives in the wrong crate (layering)

- **Design intent:** §3.8 assigns confirmation to `prism-ledger` ("`client/prism-ledger` — confirmation + bridge"). `prism-chain` (§3.4) is "the DAG state machine" — vote tallies, fork choice, traversal — not confirmation *policy*.
- **What was built:** Stage-1's `confirmed_leader_at_level(level, k)` lives in `prism-chain` (lib.rs), while stage-2's `confirmation.rs` correctly lives in `prism-ledger`. So one confirmation concern is split across two crates, and the crate the guide names as the confirmation owner only holds half of it.
- **Why it matters:** Minor today, but it's a layering smell: `prism-chain` now knows about the `k`-deep *policy* (a ledger/confirmation concern), not just the DAG *facts* (vote depths) that policy consumes. It also makes the two stages harder to unify (B-4) since they read state through different crates' APIs.
- **Fix:** Move the *policy* (`confirmed_leader_at_level`'s "every chain voted AND all ≥k deep" rule) into `prism-ledger` alongside stage-2, leaving `prism-chain` to expose only the raw facts it already has (`proposers_at_level`, per-vote depths via `votes`). `prism-ledger` then owns *both* confirmation stages, as §3.8 intends. Low effort, and it's a natural companion to the B-4 stage-2 wiring.
- **Status:** RESOLVED (fixplan item 5, AF-2). `confirmed_leader_at_level` moved to a free function in `prism-ledger`; `PrismChain` no longer has this method at all. `prism-ledger` now owns both confirmation stages as designed.

### AF-3 (M): the entire DAG is behind one global `Mutex`, on the hot path

- **Design intent:** The guide doesn't mandate a concurrency model, but the data-flow diagram (§3.1) shows the miner, the network/insert path, and the ledger/bridge all operating against the shared DAG concurrently — three independent producers/consumers.
- **What was built:** `Arc<Mutex<PrismChain>>`, taken by: every inbound/self-mined block insert, every 500ms miner template rebuild, and (per AF-1) every 250ms bridge poll. At §3.7's target `m=100` with the paper's block rates, this single lock is on the critical path of all three and will be the first scalability wall.
- **Why it matters:** It's fine for a single-digit-block-per-second devnet, but it structurally caps throughput well below what the paper's parallel-voter-chain design is *for* — the whole point of `m` independent voter chains is parallelism, which a global lock serializes away at the node level.
- **Fix:** Not urgent, but the right direction is finer-grained locking (per-voter-chain state is largely independent — the reorg/vote logic already operates per-chain-index) or an actor/channel model where one task owns the DAG and others message it (which also subsumes AF-1's wakeup cleanly). Defer until it's measured to matter; note it so the `Mutex` isn't mistaken for a considered final design.
- **Status:** STILL OPEN, by design (fixplan item 9, Phase 4 — "only when measured to matter"). Not attempted.

### AF-4 (H): the determinism invariant is stated as a contract but enforced with one hole

- This is the architectural framing of **B-1**. §3.1 is *binding*: "every honest node must derive the identical Substrate block from the same leader sequence." The implementation treats this as a real contract almost everywhere (see "what matches well" above) — which is exactly why the one violation (local-wall-clock timestamp) is dangerous: the surrounding discipline makes it *look* deterministic, so the hole doesn't announce itself. An architecture that's 95% deterministic with one silent non-deterministic input is arguably worse than one that's obviously non-deterministic, because it passes single-node testing and byte-diverges only under multi-node scrutiny that hasn't been run yet.
- **Fix:** as B-1 (deterministic timestamp from Prism data) — but the architectural lesson is broader: there should be a single, tested, enforced choke-point that asserts "every input to block construction is a pure function of confirmed DAG state." The bridge is the natural place for a `debug_assert`/test that re-deriving the same level on two `PrismChain`s with the same accepted-block set yields byte-identical `BlockImportParams`.
- **Status:** The timestamp hole is RESOLVED via B-1's fix. **A second, distinct hole in the same "one silent non-deterministic/non-reproducible input" category was found live on 2026-07-07 and is also RESOLVED**: the leader digest was attached via `BlockImportParams::post_digests`, which bypasses re-execution and so only worked on the authoring node's own trusted self-import. Any *other* node importing the identical block through normal Substrate sync re-executes it (`PrismBridgeVerifier`'s default `StateAction::Execute`), and `frame_executive`'s digest-count assert panicked, because `initialize_block` only preserves `DigestItem::PreRuntime` items and silently drops the `Other`-tagged one used here — meaning a node that ever fell behind could never safely rejoin. This is exactly the failure mode this finding predicted ("passes single-node testing and byte-diverges only under multi-node scrutiny that hasn't been run yet") — confirmed by running that scrutiny for the first time. Fixed by reclassifying the digest as `PreRuntime`, attached before block-building via `with_inherent_digests` instead of after via `post_digests`; re-verified live (kill/restart/reconnect/backfill a node, confirm no panic, confirm byte-identical hash). **Still open:** the broader choke-point assertion/test this finding specifically calls for — a `debug_assert`/test proving two `PrismChain`s with the same accepted-block set yield byte-identical `BlockImportParams` — has not been written as a standing test; the property has now been verified live but not automated.

### AF-5 (L): BuildLedger re-reads block bodies from RocksDB on every confirmation

- **Design intent:** §3.8 BuildLedger expands the leader's ref graph; the guide is silent on caching.
- **What was built:** `build_ledger_delta_for_leader` does a DFS issuing a `db.get_block` per visited proposer/tx block, every time a level confirms. At steady state this re-reads and re-decodes a growing ref graph from disk repeatedly.
- **Why it matters:** Low — correctness is fine, it's a constant-factor efficiency thing — but it compounds with AF-1's over-polling and interacts with B-7 (indices not persisted): the DAG traversal metadata the DFS needs is *already in memory* in `prism-chain` (`proposer_meta`), yet the ledger goes back to disk for it.
- **Fix:** Let the ledger consume `prism-chain`'s in-memory metadata for graph traversal and only hit the DB for the actual tx *bytes* it needs to emit. Pairs naturally with AF-2 (moving confirmation into the ledger gives it direct access to that state).
- **Status:** STILL OPEN (fixplan item 6, deliberately deferred alongside B-7's index-CF population — see that item's "not done" note). `build_ledger_delta_for_leader` still traverses via DB reads.

## How the layering and data-plane / control-plane split hold up

- **Control plane (ordering) vs data plane (execution)** — cleanly separated, as designed. Prism decides *order*; the Substrate STF decides *validity/effect*; the bridge's skip-rule is the correct seam (structural dedup in Prism, account-model conflicts at execution — exactly §3.1's division). No leakage in either direction.
- **The one coupling that's slightly off** is AF-2 (confirmation policy leaked down into `prism-chain`). Everything else respects the guide's crate ownership.
- **Bottom line on fidelity:** the implementation built the *right machine*. Its defects are a disconnected wire (AF-1), a mislabeled part (AF-2), a single leaky input (AF-4/B-1), and some parts sized for a prototype not production (AF-3, AF-5, B-7). None require re-architecting; all are "finish wiring what's already the correct design." That's a good place to be — the expensive, hard-to-reverse decisions (client-side consensus, crate boundaries, determinism-by-construction, vote-rollback correctness) were made correctly.

## What's left (folding in Part 2)

All numbered findings (B-1 through B-10, AF-1/AF-2/AF-4/AF-5, A-1/A-3/A-4 incl. auto-register) are resolved or partially addressed by design (B-3, B-7/AF-5's index-CF perf gap — see their `Status` lines). **Implementation A is fully closed — every remaining item below is Implementation B or backlog:**

1. **Live-devnet verification** across the board — the single biggest remaining gap; see the fixplan doc.
2. **AF-3** — finer-grained DAG concurrency. Only when measured to matter. (deferred by design)
3. **B-10, full network-level adversarial run** — as convenient. (L)
