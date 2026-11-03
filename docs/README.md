# docs/ — what's here and what's left

Closed work is removed from this folder once it's done; its history lives in
git log. Everything below is either current reference or still open.

## In progress

| Doc | What's left |
|---|---|
| [DESIGN_REVIEW_2026-09_SPEC.md](./DESIGN_REVIEW_2026-09_SPEC.md) | **Start here.** Whole-repo Opus 5.5 design review: Tier 1 consensus-safety fixes (R-1…R-6), design decisions (D-*), network (N-*), storage (S-*), wallet/runtime (W-*), ops (O-*) |
| [IMPLEMENTATION_AB_FIXPLAN.md](./IMPLEMENTATION_AB_FIXPLAN.md) | Prism consensus: **B-18** (fresh-node backfill from a pruning peer), B-11 retarget (deferred), small backlog |
| [IMPLEMENTATION_D_PROGRESS.md](./IMPLEMENTATION_D_PROGRESS.md) | Storage platform: finish **D4** (live `activate_shard` + `fetch`, retrieval signature check), then **D5** repair |
| [IMPLEMENTATION_D_SPEC.md](./IMPLEMENTATION_D_SPEC.md) | Spec of record for D |

## Upcoming (spec'd, not started)

| Doc | Scope |
|---|---|
| [PRISM_RETARGET_SPEC.md](./PRISM_RETARGET_SPEC.md) | B-11 real difficulty retarget. Waits for a multi-machine testnet |
| [IMPLEMENTATION_E_SPEC.md](./IMPLEMENTATION_E_SPEC.md) | Production bootstrap E0–E6: genesis, weights/fees, governance, utilities, ink!, ops, Falcon tooling |
| [FRONTEND_DAPP_SPEC.md](./FRONTEND_DAPP_SPEC.md) | Functional frontend. F0/F1 done, F2+ next |
| [DAPP_CATALOG_SPEC.md](./DAPP_CATALOG_SPEC.md) | App layer (Anchor, Keystone, Press, …) on top of D/E |

## Reference

| Doc | Purpose |
|---|---|
| [ARCHITECTURE.md](./ARCHITECTURE.md) | How the chain works as built |
| [ARGONAUT_ARCHITECTURE_GUIDE.md](./ARGONAUT_ARCHITECTURE_GUIDE.md) | Original design intent + roadmap |
| [FRONTEND_SOLARSYSTEM_SPEC.md](./FRONTEND_SOLARSYSTEM_SPEC.md) | The `web/` WebGL experience (largely built) |
| [Prism.pdf](./Prism.pdf) | The Prism consensus paper |
| [rust-setup.md](./rust-setup.md) | Toolchain install (upstream Substrate boilerplate) |
