# Implementation B Progress Tracker — Prism Consensus

Tracks status against [`ARGONAUT_ARCHITECTURE_GUIDE.md`](./ARGONAUT_ARCHITECTURE_GUIDE.md) §3 ("Implementation B — Prism Consensus") and its milestones in §3.9. Lives on the `Base` branch.

## Status: B0-B4 complete and tested. Fix-plan Phases 1-3 also complete (code + unit tests). B5 (pruning) not started.

| §3.9 milestone | Status |
|---|---|
| **B0** (optional): Aura → PoW rehearsal | ✅ done, then fully superseded — its throwaway PoW chain was deleted and replaced by B3's real bridge; nothing from it remains in the codebase |
| **B1**: primitives + db + chain state machine, fully unit-tested offline; property test vote counts == recount-from-scratch | ✅ done |
| **B2**: miner + network, single-node self-mining loopback, then multi-node convergence | ✅ done — 4-node convergence verified via a manual multi-process devnet with a real kill/restart churn cycle (not docker-compose itself — see backlog) |
| **B3**: ledger stage 1 + Substrate bridge; Aura/GRANDPA removed | ✅ done — live-verified end to end (balance transfer → confirmed leader → Substrate block with a decoded leader-level digest) |
| **B4**: stage-2 fast confirmation + adversarial simulation | ✅ done — confirmation policy ported and tested |
| B5: pruning | ⬜ not started — see [`IMPLEMENTATION_AB_FIXPLAN.md`](./IMPLEMENTATION_AB_FIXPLAN.md)'s "Then: B5" section |

## What's here, for orientation (not a to-do list — read the code for detail)

- **`client/prism-chain`**: the DAG state machine — `voter_meta`/`proposer_meta` in-memory metadata, voter fork-choice + LCA-walk reorg/vote-rollback, orphan resolution for both proposer and voter blocks, per-vote depth tracking (`VoteInfo::per_chain`), the client-side difficulty retarget (fix-plan B-2), full replay-on-restart via `with_db` (fix-plan B-7).
- **`client/prism-miner`**: `build_template` (assembles all m+2 content slots from live chain/mempool state), `try_mine` (nonce search that carries `next_nonce` forward across calls, ruling out the reset-to-zero stall class of bug structurally).
- **`client/prism-network`**: `handle_announcement` (validate → dedupe → insert → gossip/fetch decision), `wire.rs`'s `PrismNetworkWorker` (real `sc-network` transport, independent peer-slot budget for the Prism gossip protocol).
- **`client/prism-ledger`**: `confirmed_leader_at_level` (stage-1, k-deep unanimous rule) and `confirmation.rs` (stage-2, confidence-interval policy per Def. 4.5) both live here; `PrismLedger::advance` tries stage-2 first, falls back to stage-1 (fix-plan B-4); `PrismLedger::recover` rebuilds state on restart (fix-plan B-7).
- **`node/src/bridge.rs`**: the one and only place Prism and Substrate touch — `run_prism_bridge` calls `PrismLedger::advance` on an event-driven wakeup (fix-plan AF-1) and builds one Substrate block per newly confirmed level via `sc_block_builder`, with a deterministic timestamp (fix-plan B-1) and a `"prsm"`-prefixed leader digest pushed via `post_digests`.

## Still-open items originating from B0-B4 (not superseded, not yet tracked elsewhere)

Everything else that was ever flagged as out-of-scope during B0-B4 (backfill/initial sync, docker-compose, peer-slot mirroring, per-chain genesis hashes, difficulty retargeting, mempool integration, per-vote depth tracking) has since either been fixed or is tracked as a numbered item in [`IMPLEMENTATION_AB_FIXPLAN.md`](./IMPLEMENTATION_AB_FIXPLAN.md) — that doc, not this one, is the current to-do list. The one item not already captured there:

- **Full network-level (not single-process-simulated) adversarial run.** The B4 harness drives `PrismChain`/`PrismLedger` directly with attacker-chosen blocks — a real state machine, no mocked adversary — but never through real multi-node gossip/timing. Now tracked in the fixplan's backlog too.

## Next: B5 (pruning)

Deliberately not started — a storage-optimization concern, not a correctness gap. Blocked on fix-plan B-7's index-CF population (still deferred as a scale concern). See the fixplan doc's "Then: B5" section for the interaction `PrismLedger::build_ledger_delta` will need once pruning lands (treat "ref below the prune frontier" as "already counted," not a DB-lookup failure).
