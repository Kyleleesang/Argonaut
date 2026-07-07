# Phase 0 Progress Tracker

Tracks status against the acceptance criteria in [`ARGONAUT_ARCHITECTURE_GUIDE.md`](./ARGONAUT_ARCHITECTURE_GUIDE.md) §1 ("Phase 0 — Foundation: Migrate & Restructure the Repo"). Lives on the `Base` branch (`main` is untouched, still on the pre-migration commit `fd25b6c`).

## Status: build/test/run acceptance criteria are met. CI is the one item left.

| Acceptance criterion (§1.2) | Status |
|---|---|
| `cargo build --release` green | ✅ clean release build incl. wasm runtime |
| `--dev` node produces blocks | ✅ (originally verified under Aura/GRANDPA; superseded since by B0-B3's swap to Prism consensus — not re-verified against this criterion's original wording, but the node demonstrably produces and imports blocks under the current consensus, see `IMPLEMENTATION_B_PROGRESS.md`) |
| `cargo test` green | ✅ all crates pass |
| CI added for `wasm32-unknown-unknown` runtime build | ⬜ **not met, and the existing config is misleading.** `.github/workflows/check.yml` exists but (a) triggers only on `main`, while all work happens on `Base` — it never runs; (b) runs `cargo check --release` with `SKIP_WASM_BUILD=1`, i.e. explicitly *skips* the one build this criterion asks to check; (c) predates `dev-signatures` and every crate added since Phase 0. |

## Remaining

- [ ] Fix/rewrite `.github/workflows/check.yml`: trigger on `Base` (not just `main`), run `cargo build --release` and `cargo test --workspace` without `SKIP_WASM_BUILD`, and add a dedicated `wasm32-unknown-unknown` runtime build check. This is the only unmet item from §1.2's acceptance criteria.
- [ ] Clean up lingering warnings: unused imports in `client/prism-chain`, `client/prism-miner`, `client/aggregator`, `node/src/chain_spec.rs`; deprecated `create_runtime_str!` macro in `runtime/src/lib.rs` (should move to `Cow::Borrowed`).
- [ ] `cargo report future-incompatibilities` flagged `subxt v0.37.0` (transitive, via `frame-benchmarking-cli`) as containing code a future Rust version will reject. Not urgent — likely resolved by a future polkadot-sdk bump.
