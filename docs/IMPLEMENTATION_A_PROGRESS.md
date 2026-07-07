# Implementation A Progress Tracker — Falcon-512 Signatures + Registry

Tracks status against [`ARGONAUT_ARCHITECTURE_GUIDE.md`](./ARGONAUT_ARCHITECTURE_GUIDE.md) §2 ("Implementation A — Falcon-512 Signatures + Public-Key Registry"). Follows [`PHASE0_PROGRESS.md`](./PHASE0_PROGRESS.md) per the doc's §6 build order. Lives on the `Base` branch.

## Status: Implementation A is functionally complete. Both signing modes work end-to-end, tested, and wired into the runtime and CLI.

| §2.5 acceptance criterion | Status |
|---|---|
| Sign/verify round-trip; corrupted sig/pk/msg each fail; `pk_hash` matches independent blake3 | ✅ `primitives/falcon` — 10 unit tests, using real `fn-dsa` (dev-dependency) keypairs signed and verified through the actual `falcon_crypto::verify` host function |
| Bootstrap tx succeeds and key appears in storage | ✅ registry pallet tests cover register/deregister/mismatch/already-registered/insufficient-balance (11 tests) |
| Registry-mode tx smaller; rejected in pool if unregistered | ✅ `CheckFalconSignature` `TransactionExtension` implemented and tested (5 runtime-level tests): valid sig accepted + origin authorized, unregistered account → `BadSigner`, wrong signature → `BadProof`, already-authorized origin guarded → `BadSigner`, `Disabled` variant is a pure passthrough |
| Weight/size assertion test (registry-mode ≤ 850 bytes) | ⬜ not written as an explicit assertion test, but the mechanism is in place (`CheckFalconSignature::RegistryMode { sig, account }` carries no inline public key, unlike bootstrap mode's `FalconWithKey { sig, pk }` — the ~897-byte saving is structural) |
| wasm build unaffected | ✅ verified repeatedly across every change in this doc (see "Verification" below) |
| Auto-register-on-first-bootstrap-tx (optional, config-gated) | ⬜ not implemented — see "Remaining / possible follow-ups" |

## What changed, in build order

### 1. Real Falcon host functions + `Verify`/`IdentifyAccount` (`primitives/falcon`)
- `falcon_crypto` was a hand-rolled `pub mod` with `#[cfg(feature = "std")]`-gated plain functions — not an actual Substrate host-function interface (no `HostFunctions` type existed, despite a doc-comment claiming one was "registered"). Rewrote it as a real `#[sp_runtime_interface::runtime_interface] pub trait FalconCrypto { fn verify(...); fn pk_hash(...); }`. First use of `runtime_interface` anywhere in the repo — pattern sourced from `sp-io`'s own `Storage`/`Trie` host-function traits in the vendored polkadot-sdk checkout.
- Added `sp-runtime` as a dependency (was missing entirely).
- Added `ArgonautSigner` enum (`Falcon(FalconPublic)` / `#[cfg(feature = "std")] Sr25519(sr25519::Public)`), `impl IdentifyAccount for ArgonautSigner` (`AccountId = AccountId32`, derived via `blake3(pk)` for the Falcon variant), and `impl Verify for ArgonautSignature`, following the same pattern as `sp_runtime`'s own `MultiSignature::verify` (confirmed against `substrate/primitives/runtime/src/lib.rs:449`). Registry-mode (`ArgonautSignature::Falcon { .. }`) always returns `false` from `Verify` by design — it carries no inline key, so acceptance is deferred to `CheckFalconSignature` (see below).
- Added `impl From<sr25519::Public> for ArgonautSigner` so chain-spec dev-account seeding (`get_account_id_from_seed::<sr25519::Public>("Alice")`) keeps working unchanged.
- Added `fn-dsa` (full sign+verify, vs. the verify-only `fn-dsa-vrfy` used at runtime) as a dev-dependency for real keypair-generation-and-signing round-trip tests.

### 2. `KeyProvider` trait + tests (`pallets/falcon-registry`)
- Added the `KeyProvider<AccountId>` trait (`fn key_of(who) -> Option<FalconPublic>`) — previously only referenced in doc-comments, never defined. Implemented for `Pallet<T>` by delegating to the existing inherent `key_of` method.
- Added a mock runtime (`src/mock.rs`) and 11 tests (`src/tests.rs`) — the crate had zero tests before. The mock's `AccountId` must be `AccountId32` (32 bytes), not the usual test-mock-default `u64`, since bootstrap-mode's `blake3(pk) == AccountId` check is meaningless otherwise. Test accounts are derived from each test's own keypair rather than fixed constants.

### 3. Wire `ArgonautSignature` in as the runtime's `Signature` type (`runtime/`)
- `pub type Signature = MultiSignature;` → `pub type Signature = argonaut_primitives_falcon::ArgonautSignature;`. `AccountId`'s derivation line didn't need to change — it resolves through `ArgonautSigner` instead of `MultiSigner`, transparently.
- Added `impl pallet_falcon_registry::Config for Runtime` and `FalconRegistry: pallet_falcon_registry` to `construct_runtime!`.
- Registered the Falcon host functions in the node executor's `HostFunctions` tuple (`node/src/service.rs`) — previously a comment-only placeholder.

### 4. `CheckFalconSignature` `TransactionExtension` — registry-mode (`runtime/src/extensions.rs`)
This is the core piece that makes registry-mode actually work, and the main research investment this session:
- Confirmed (by reading the vendored `stable2412` checkout directly) that `sp_runtime::generic::UncheckedExtrinsic` supports a third `Preamble::General(ExtensionVersion, Extension)` form alongside the classic `Bare`/`Signed` ones — an extrinsic with **no** `Address`/`Signature` field at all. For this preamble, `Checkable::check()` does no signature verification whatsoever; it's entirely delegated to the `TransactionExtension` pipeline, which starts with `origin = RawOrigin::None` and must authorize it itself if it wants the call dispatched as signed.
- This means registry-mode's signature and claimed account can't live in the extrinsic the way bootstrap-mode's do (there's no `Signature` field on a `General` extrinsic to put them in) — they have to live *inside the extension itself*. Modeled `CheckFalconSignature` directly on polkadot-sdk's own `pallet-verify-signature` (`substrate/frame/verify-signature/src/extension.rs`), which solves the identical problem: an enum with a data-carrying variant (`RegistryMode { sig, account }`) and a no-op `Disabled` variant (used for bootstrap-mode `Signed` extrinsics, whose signature was already checked via `Verify::verify` before the extension pipeline even runs).
- `validate()`: guards against running after something already authorized the origin (`AsTransactionAuthorizedOrigin::is_transaction_authorized`), looks up the claimed account's key via `KeyProvider`, hashes the "inherited implication" (call + rest-of-pipeline, via `sp_runtime::traits::{Implication, TxBaseImplication}`) with blake3, calls `falcon_crypto::verify`, and on success authorizes the origin via `OriginTrait::set_caller_from_signed`.
- Placed first in the runtime's `SignedExtra` tuple — required, since every extension after it (`CheckNonce`, `CheckNonZeroSender`, ...) expects an already-authorized signer for `General` extrinsics.
- **Tested with 5 runtime-level tests** (`runtime/src/tests.rs`) using real `fn-dsa` keypairs, calling `TransactionExtension::validate()` directly: valid signature accepted + origin correctly authorized; unregistered account rejected (`BadSigner`); wrong signature rejected (`BadProof`); already-authorized origin rejected (defense-in-depth, `BadSigner`); `Disabled` is a pure passthrough.
- Fallout fix: `node/src/benchmarking.rs` manually constructs a `SignedExtra` tuple for its `RemarkBuilder`/`TransferKeepAliveBuilder` CLI helpers (classic `Signed`/bootstrap-style extrinsics) — had to prepend `CheckFalconSignature::disabled()` there too, plus a matching `()` in the additional-signed-data tuple.

### 5. CLI tooling (`node/src/keygen.rs`, `node/src/falcon_sign.rs`)
Confirmed first that Substrate's stock `sc_cli::KeySubcommand`/`CryptoScheme` enum is closed (hardcoded `Ed25519 | Sr25519 | Ecdsa` in `substrate/client/cli/src/commands/arg_enums.rs`) — can't be extended from outside that crate, so both are bespoke subcommands rather than plugins into the stock one:
- **`falcon-key generate`**: generates a Falcon-512 keypair, derives its `AccountId` (`blake3(pk)`), and writes a keystore file `{ss58-address}.falcon.json`. If `--passphrase <str>` is given, the signing key is encrypted at rest (ChaCha20-Poly1305, key derived from the passphrase via `blake3::derive_key` — both already in the dependency graph transitively, so no new crypto surface was introduced). Without a passphrase, it's written in the clear with a loud warning (fine for throwaway devnet accounts, explicitly not for anything real).
- **`falcon-key inspect <path>`**: prints the address/public key from a keystore file without decrypting it.
- **`falcon-sign <path> --payload-hex <hex> [--passphrase <str>]`**: offline bootstrap-mode signer — decrypts (if needed) the keystore, signs `blake3(payload)`, and prints the hex-encoded SCALE `ArgonautSignature::FalconWithKey { sig, pk }`.
- **Deviation from the doc**, noted per its own guidance (§8): the doc specifies a separate `argonaut-sign` binary/workspace-member; this session folded that functionality into the node CLI as `falcon-sign` instead — same functionality, less scaffolding, shares the keystore format with `falcon-key generate`.
- **Scope cut**: `falcon-sign` only covers bootstrap-mode. Registry-mode signing needs a full `General`-preamble extrinsic assembled against a specific runtime spec/genesis (nonce, era, tip, ...) — a transaction-builder task, not a "sign this payload" one. Left as follow-up tooling.

### 6. Benchmarking (`pallets/falcon-registry/src/benchmarking.rs`)
Added `#[benchmarks]`/`#[benchmark]` (v2 macro) benchmarks for `register_key` and `deregister_key`, following the pattern in `substrate/frame/sudo/src/benchmarking.rs`. Wired into `runtime/src/lib.rs`'s `define_benchmarks!` list and both crates' `runtime-benchmarks` feature. Verified via `impl_benchmark_test_suite!`, which runs as real tests under `cargo test --features runtime-benchmarks` (both pass).

## Remaining / possible follow-ups

- ~~**`ArgonautSignature::Sr25519` gating**~~ — **done**, post-B3 fix-plan session (fixplan item 2 / review finding A-1): the dev-only escape hatch is now behind a real `dev-signatures` Cargo feature, propagated identically to wasm and native builds and off by default, replacing the `cfg(feature = "std")` gate that had caused a real wasm/native SCALE mismatch bug. **Still open:** an explicit live check that a production wasm build (no `dev-signatures`) actually rejects an sr25519-signed extrinsic — reasoning is solid but not manually reproduced from the "now fixed" side.
- **Weight/size assertion test** (fixplan item 13 / review A-4): no test explicitly measures and asserts the encoded byte-size delta between bootstrap-mode and registry-mode extrinsics (doc §2.5 point 3, "≤ 850 bytes for a balance transfer"). The structural saving exists (no inline `pk` in registry-mode); just not asserted in a test. Not implemented.
- **Auto-register-on-first-bootstrap-tx** (fixplan item 13 / review A-4): doc §2.3's optional, config-gated behavior ("the first transaction doubles as registration") isn't implemented. Would live inside `CheckFalconSignature`'s handling of the `Signed`/bootstrap path, which today is a pure no-op (`Disabled`).
- **Registry-mode transaction builder** (fixplan item 13 / review A-3): nothing yet assembles an actual `General`-preamble `UncheckedExtrinsic` end-to-end (genesis hash, nonce, era, tip, encoding the whole thing for submission via RPC) — `CheckFalconSignature` and its tests operate at the `TransactionExtension::validate()` level, which is the correctness-critical part, but there's no CLI/script path yet from "I have a keystore" to "a registry-mode transaction landed in a block." Not implemented.

See [`IMPLEMENTATION_AB_FIXPLAN.md`](./IMPLEMENTATION_AB_FIXPLAN.md)'s "Outstanding TODOs" section for the current, consolidated to-do list across both Implementation A and B.

## Verification performed this session

Repeated after every meaningful change (codec/host-function rewrite, runtime signature-type swap, extension addition, CLI addition, benchmarking addition):
- `cargo check --workspace` / `cargo check -p <crate>` — clean throughout, only pre-existing warnings (unused imports elsewhere, deprecated `create_runtime_str!`).
- `cargo test --workspace` — green throughout, final run: 10 (falcon primitives) + 11 (registry pallet) + 2 (registry benchmarking suite) + 7 (runtime, incl. 5 `CheckFalconSignature` tests) + others unaffected, all passing.
- `cargo test -p pallet-falcon-registry --features runtime-benchmarks` — green (13 tests, incl. `bench_register_key`/`bench_deregister_key`).
- `cargo check -p argonaut-runtime --features runtime-benchmarks` (incl. the wasm build) — clean.
- `cargo build --release -p argonaut-node` (includes the `wasm32-unknown-unknown` runtime build) — clean, 9m10s.
- **CLI tools exercised directly against the release binary**, not just compiled: `falcon-key generate` (both with and without `--passphrase`), `falcon-key inspect`, and `falcon-sign` — including negative cases (wrong passphrase → rejected with a clear error; missing passphrase on an encrypted keystore → rejected; correct passphrase → signs successfully). Caught and fixed a test-script mixup along the way (an `ls | tail -1` picked the wrong of two keystore files by alphabetical sort, not a bug in the tool itself).
- A `--dev` node built from the final state of all these changes authors and finalizes blocks #1–#6 under Aura/GRANDPA with everything live (the new `Signature` type, `CheckFalconSignature` in the extension pipeline, the registry pallet, benchmarking wiring).
