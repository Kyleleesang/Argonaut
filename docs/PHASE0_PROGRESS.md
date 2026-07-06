# Phase 0 Progress Tracker

Tracks status against the acceptance criteria in [`ARGONAUT_ARCHITECTURE_GUIDE.md`](./ARGONAUT_ARCHITECTURE_GUIDE.md) §1 ("Phase 0 — Foundation: Migrate & Restructure the Repo"). Lives on the `Base` branch, where all Phase 0 work is happening (`main` is untouched, still on the pre-migration commit `fd25b6c`).

## Status: Phase 0's build/test/run acceptance criteria are met. CI is the one item left.

| Acceptance criterion (§1.2) | Status |
|---|---|
| `cargo build --release` green | ✅ verified — clean release build incl. wasm runtime (`-Z build-std`), 15m43s, exit 0 |
| `--dev` node produces blocks under Aura | ✅ verified — ran the release binary with `--dev`; imported blocks #1–#6, finalizing under GRANDPA, ~6s block time, no errors |
| `cargo test` green | ✅ all crates pass (0 failures) |
| CI added for `wasm32-unknown-unknown` runtime build | ⬜ not started — no CI config exists yet |

## Timeline

- **Commit `0aca3a6`** ("Phase 0: migrate to polkadot-sdk stable2412 + full architecture restructuring") did the bulk of the migration in one pass: repointed every `sp-*`/`sc-*`/`frame-*` dependency at the `stable2412` polkadot-sdk branch, deleted `runtime/src/Consensus/*` and `node/src/Mining/*`, and scaffolded the full target layout from the architecture doc (`primitives/{falcon,prism,agg}`, `client/{prism-db,prism-chain,prism-miner,prism-network,prism-ledger,aggregator}`, `pallets/{falcon-registry,agg-verify}`) — i.e. it combined the Phase 0 migration with a first pass at Implementations A/B/C's scaffolding, ahead of the doc's suggested build order (§6).
- That commit did not compile. This session found and fixed the underlying bugs (below) until the workspace built clean and `cargo test --workspace` passed.

## Bugs found and fixed this session

1. **`parity-scale-codec` / `parity-scale-codec-derive` version skew** (`Cargo.toml`) — root cause of nearly every initial error (`could not find '__private' in 'codec'`). The workspace pinned `parity-scale-codec = "3.6"`, but the derive-macro sub-crate resolved independently to `3.7.5`, whose generated code calls a `codec::__private` module that doesn't exist in `3.6.12`. Fixed by bumping the pin to `"3.7"` (and `scale-info` to `"2.11"`) and re-running `cargo update`. This is the same class of bug the architecture doc warns about in §1.2.3 ("every dependency must come from the same release") — just manifesting in `parity-scale-codec` instead of `sp-core`.
2. **Missing system dependency: `protoc`** — `litep2p`'s build script needs the protobuf compiler; not a code bug, just an unmet host requirement. Installed via `apt-get install protobuf-compiler`.
3. **Missing `wasm32-unknown-unknown` rustup target** — needed to compile the runtime to Wasm. Installed via `rustup target add`.
4. **`client/prism-ledger`**: used `blake3::hash(...)` without declaring `blake3` as a dependency in `Cargo.toml`.
5. **`primitives/falcon`**: `falcon_crypto::verify` called a nonexistent API (`fn_dsa_vrfy::{verify_key_size, verify_signature, FalconVariant}` — none of these exist in the `fn-dsa-vrfy` crate). Rewrote against the real API: `VerifyingKeyStandard::decode(pk)` + `.verify(sig, &DOMAIN_NONE, &HASH_ID_RAW, msg)`.
6. **`primitives/falcon`**: `FalconPublic`/`FalconSignature` were plain `Vec<u8>` newtypes, which don't implement `MaxEncodedLen` — required for any value stored in a `#[pallet::storage] StorageMap`. Switched both to `BoundedVec<u8, ConstU32<897>>` / `ConstU32<752>>` per architecture doc §2.2's original spec (the commit had silently simplified away from the spec).
7. **`primitives/agg`**: `AggBundle` was missing `PartialEq`/`Eq`, but `#[pallet::call]` requires every call argument type to implement them (the generated `Call` enum derives `PartialEq`). Added the derives.
8. **`pallets/falcon-registry`**: two bugs — (a) used `Currency::reserve`/`unreserve`, which only exist on the `ReservableCurrency` subtrait, not the base `Currency` trait; (b) used `who.as_ref()` to get `AccountId` bytes, which isn't guaranteed by `frame_system::Config`'s bounds (attempted fix via a trait-level `where` clause first, but Rust doesn't propagate that bound into the pallet macro's generated code — switched to `who.encode()`, which is guaranteed via the `Encode`/`Codec` bound `frame_system::Config::AccountId` already carries).
9. **`runtime/src/lib.rs`**: several breaking API changes between whatever polkadot-sdk snapshot the code was originally written against and the actual `stable2412` branch:
   - Missing `Vec`/`vec!` import (no_std runtime) — added `use sp_std::prelude::*;`.
   - `frame_system::Config` gained new required associated types in `stable2412` (`RuntimeTask`, `ExtensionsWeightInfo`, `SingleBlockMigrations`, `MultiBlockMigrator`, `PreInherents`, `PostInherents`, `PostTransactions`) — all set to `()`, matching frame_system's own reference config.
   - `pallet_balances::Config` gained `DoneSlashHandler` — set to `()`.
   - `pallet_transaction_payment::Config` gained `WeightInfo` — set to `()`.
10. **`node/`**: four bugs —
    - `command.rs` called `Cli::parse()` without `use clap::Parser;` in scope.
    - `command.rs` imported `argonaut_runtime::Block` (the concrete, non-opaque block type) and passed it to `service::new_full::<sc_network::NetworkWorker<Block, _>>`, but `service.rs`'s `new_full` is defined in terms of `argonaut_runtime::opaque::Block` — two same-named-but-different types, causing a `NetworkBackend` trait-bound mismatch. Fixed the import to use `opaque::Block`, matching `service.rs`.
    - `chain_spec.rs` referenced `sc_chain_spec::GenericChainSpec` but `node/Cargo.toml` never declared `sc-chain-spec` as a dependency. Added it.
    - `node/Cargo.toml` pinned `jsonrpsee = "0.22"` directly from crates.io, while every polkadot-sdk `stable2412` crate that touches RPC (`sc-rpc`, `pallet-transaction-payment-rpc`, `substrate-frame-rpc-system`) pulls `jsonrpsee 0.24.3` — another version-skew bug, same pattern as #1. Bumped to `"0.24"`.

## Remaining before Phase 0 can be called fully done

- [ ] Add CI (e.g. GitHub Actions) that runs `cargo build --release`, `cargo test --workspace`, and a dedicated `wasm32-unknown-unknown` runtime build check. This is the only unmet item from §1.2's acceptance criteria.
- [ ] Clean up lingering warnings: unused imports in `client/prism-chain`, `client/prism-miner`, `client/aggregator`, `node/src/chain_spec.rs`; deprecated `create_runtime_str!` macro in `runtime/src/lib.rs` (should move to `Cow::Borrowed`).
- [ ] `cargo report future-incompatibilities` flagged `subxt v0.37.0` (a transitive dep, pulled in via `frame-benchmarking-cli`) as containing code that a future Rust version will reject. Not urgent, but worth tracking — likely resolved by a future polkadot-sdk bump.

## Notes on scope vs. the architecture doc's build order

The doc's §6 build order says Phase 0 should land before any of Implementations A/B/C. In practice the single migration commit already scaffolded working (if buggy) code for all of A (Falcon), B (Prism client-side primitives/db/chain/miner/network/ledger — stubs, not yet wired to a running node), and C (agg-verify, aggregator — stub verifier). None of A/B/C's own acceptance tests (§2.5, §3.9, §4.6) have been run yet; this session only got the workspace to compile and its existing unit tests to pass. Treat A/B/C as "compiles, minimally unit-tested" rather than "milestone-complete."
