# Implementation B Progress Tracker — Prism Consensus

Tracks status against [`ARGONAUT_ARCHITECTURE_GUIDE.md`](./ARGONAUT_ARCHITECTURE_GUIDE.md) §3 ("Implementation B — Prism Consensus") and its milestones in §3.9. Lives on the `Base` branch.

## Status: B0-B5 complete and tested, including pruning. Fix-plan phases 1-3 also complete (code + unit tests). Only live-devnet verification and backlog items remain.

| §3.9 milestone | Status |
|---|---|
| **B0** (optional): Aura → PoW rehearsal | ✅ done, then fully superseded — its throwaway PoW chain was deleted and replaced by B3's real bridge; nothing from it remains in the codebase |
| **B1**: primitives + db + chain state machine, fully unit-tested offline; property test vote counts == recount-from-scratch | ✅ done |
| **B2**: miner + network, single-node self-mining loopback, then multi-node convergence | ✅ done — 4-node convergence verified via a manual multi-process devnet with a real kill/restart churn cycle (not docker-compose itself — see backlog) |
| **B3**: ledger stage 1 + Substrate bridge; Aura/GRANDPA removed | ✅ done — live-verified end to end (balance transfer → confirmed leader → Substrate block with a decoded leader-level digest) |
| **B4**: stage-2 fast confirmation + adversarial simulation | ✅ done — confirmation policy ported and tested |
| B5: pruning | ✅ done (code + unit tests + `--prism-prune`/`--prism-k-prune` bridge wiring) — see [`IMPLEMENTATION_AB_FIXPLAN.md`](./IMPLEMENTATION_AB_FIXPLAN.md) |

## What's here, for orientation (not a to-do list — read the code for detail)

- **`client/prism-chain`**: the DAG state machine — `voter_meta`/`proposer_meta` in-memory metadata, voter fork-choice + LCA-walk reorg/vote-rollback, orphan resolution for both proposer and voter blocks, per-vote depth tracking (`VoteInfo::per_chain`), client-side difficulty retarget, full replay-on-restart via `with_db`, per-chain genesis hashes, `prune()` + `ChainCheckpoint`.
- **`client/prism-miner`**: `build_template` (assembles all m+2 content slots from live chain/mempool state), `try_mine` (nonce search that carries `next_nonce` forward across calls, ruling out the reset-to-zero stall class of bug structurally).
- **`client/prism-network`**: `handle_announcement` (validate → dedupe → insert → gossip/fetch decision), `wire.rs`'s `PrismNetworkWorker` (real `sc-network` transport over both libp2p and litep2p backends, independent peer-slot budget for the Prism gossip protocol), tip-exchange + batched backfill for initial sync.
- **`client/prism-ledger`**: `confirmed_leader_at_level` (stage-1, k-deep unanimous rule) and `confirmation.rs` (stage-2, confidence-interval policy per Def. 4.5) both live here; `PrismLedger::advance` tries stage-2 first, falls back to stage-1; `PrismLedger::recover` rebuilds state on restart from a persisted checkpoint + emitted-tx-hash CF.
- **`node/src/bridge.rs`**: the one and only place Prism and Substrate touch — `run_prism_bridge` calls `PrismLedger::advance` on an event-driven wakeup and builds one Substrate block per newly confirmed level via `sc_block_builder`, with a deterministic timestamp and a `"prsm"`-prefixed `PreRuntime` leader digest set before block-building via `with_inherent_digests`; also drives `PrismChain::prune` on its fallback tick when `--prism-prune` is set.

## Remaining

Everything still open is tracked in [`IMPLEMENTATION_AB_FIXPLAN.md`](./IMPLEMENTATION_AB_FIXPLAN.md) — that doc is the current to-do list. Summary:

- **Live two-node devnet run: done 2026-07-07** — byte-identical chains across 6 confirmed levels, a restart, and DAG backfill, all live-verified in one session. This also caught and fixed a real bug: the bridge's leader digest (`node/src/bridge.rs`) was attached via `post_digests`, which only survives a self-authoring node's trusted import; any other node importing the same block through normal Substrate sync (e.g. on restart) hit a hard panic in `frame_executive`'s digest-count check. Fixed by switching to `DigestItem::PreRuntime`, set before block-building.
- **Retarget convergence: done 2026-07-08** — a single-miner fresh-DB run showed the rate visibly correct (a burst of 13-14 blocks/sec, then oscillating toward target) once the first `RETARGET_WINDOW` elapsed, recovering from an initial ~50x-below-target rate.
- **Remaining live-verification**: stage-2/AF-1 latency measurement, pruning end-to-end with a fresh backfilling node.
- **B-10**: docker-compose devnet artifact (peer-slot mirroring, replacing the still-unmodified `substrate-node-template` boilerplate).
- **Full network-level (not single-process-simulated) adversarial run.**
- **AF-3** (finer-grained DAG concurrency): deliberately deferred, "only when measured to matter."
