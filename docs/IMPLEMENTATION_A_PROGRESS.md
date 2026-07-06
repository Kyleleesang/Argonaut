# Implementation A Progress Tracker — Falcon-512 Signatures + Registry

Tracks status against [`ARGONAUT_ARCHITECTURE_GUIDE.md`](./ARGONAUT_ARCHITECTURE_GUIDE.md) §2 ("Implementation A — Falcon-512 Signatures + Public-Key Registry"). Follows [`PHASE0_PROGRESS.md`](./PHASE0_PROGRESS.md) per the doc's §6 build order. Lives on the `Base` branch.

## Status: bootstrap-mode signing works end-to-end and is wired into the runtime. Registry-mode (the wire-format optimization) and CLI tooling are not started.

This session scoped down to a deliberately smaller slice than all of §2: get real Falcon-512 signatures verifying through the actual runtime signature-checking path first, before building the registry-mode wire-format optimization and CLI tooling on top. Rationale: registry-mode needs the "General transaction" flavor (origin-less extrinsics authorized entirely by a `TransactionExtension`) which is a separate, self-contained chunk of work — bootstrapping on it wasn't necessary to get a real post-quantum-signed devnet running.

| §2.5 acceptance criterion | Status |
|---|---|
| Sign/verify round-trip; corrupted sig/pk/msg each fail; `pk_hash` matches independent blake3 | ✅ `primitives/falcon` — 10 unit tests, using real `fn-dsa` (dev-dependency) keypairs signed and verified through the actual `falcon_crypto::verify` host function |
| Bootstrap tx succeeds and key appears in storage (if auto-register on) | ✅ registry pallet tests cover register/deregister/mismatch/already-registered/insufficient-balance (11 tests); auto-register-on-first-tx not implemented (no TransactionExtension yet — see below) |
| Registry-mode tx ~897 bytes smaller; rejected in pool if unregistered | ⬜ not started — needs `CheckFalconSignature` TransactionExtension (see "Deferred" below) |
| Weight/size assertion test (registry-mode ≤ 850 bytes) | ⬜ not started (depends on the above) |
| wasm build unaffected | ✅ verified — `cargo build --release` (9m18s, incl. wasm) is clean with `ArgonautSignature` as the runtime's `Signature` type; `cargo test --workspace` green (23 unit tests, incl. the 10 new Falcon + 11 new registry tests); `--dev` node authors and finalizes blocks #1–#6 under Aura/GRANDPA with the new signature/account plumbing live |

## What changed this session

### `primitives/falcon`
- `falcon_crypto` was a hand-rolled `pub mod` with `#[cfg(feature = "std")]`-gated plain functions — not an actual Substrate host-function interface (no `HostFunctions` type existed, despite a doc-comment claiming one was "registered"). Rewrote it as a real `#[sp_runtime_interface::runtime_interface] pub trait FalconCrypto { fn verify(...); fn pk_hash(...); }`. This is the first use of `runtime_interface` anywhere in the repo — no in-tree precedent existed, so the pattern was sourced from `sp-io`'s own `Storage`/`Trie` host-function traits in the vendored polkadot-sdk checkout.
- Added `sp-runtime` as a dependency (was missing entirely — without it, `Verify`/`IdentifyAccount` couldn't be implemented here at all).
- Added `ArgonautSigner` enum (`Falcon(FalconPublic)` / `#[cfg(feature = "std")] Sr25519(sr25519::Public)`) plus `impl IdentifyAccount for ArgonautSigner` (`AccountId = AccountId32`, derived via `blake3(pk)` for the Falcon variant) and `impl Verify for ArgonautSignature`. The `Verify` impl follows the same pattern as `sp_runtime`'s own `MultiSignature::verify` (`let who: [u8;32] = *signer.as_ref();` then dispatch per-variant) — confirmed against `substrate/primitives/runtime/src/lib.rs:449` in the vendored checkout. Registry-mode (`ArgonautSignature::Falcon { .. }`) always returns `false` from `Verify`, by design (§2.2): it carries no inline key, `Verify` has no storage access, so acceptance is deferred entirely to the not-yet-built `CheckFalconSignature` extension.
- Added `impl From<sr25519::Public> for ArgonautSigner` so chain-spec dev-account seeding (`get_account_id_from_seed::<sr25519::Public>("Alice")`) keeps working unchanged.
- Added `fn-dsa` (the full sign+verify crate, vs. the verify-only `fn-dsa-vrfy` used at runtime) as a dev-dependency, enabling real keypair-generation-and-signing round-trip tests rather than only hardcoded KAT vectors.

### `pallets/falcon-registry`
- Added the `KeyProvider<AccountId>` trait (`fn key_of(who) -> Option<FalconPublic>`) that the doc's §2.3 spec called for — it was referenced in doc-comments but never actually defined anywhere in the repo. Implemented for `Pallet<T>` by delegating to the existing inherent `key_of` method.
- Added a mock runtime (`src/mock.rs`) and 11 tests (`src/tests.rs`) — the crate had **zero** tests or mock runtime before this session. Notable wrinkle: the mock's `AccountId` must be `AccountId32` (32 bytes), not the usual test-mock-default `u64` — bootstrap-mode's `blake3(pk) == AccountId` check is meaningless otherwise. Test accounts are derived from each test's own keypair (`AccountId32::from(pk_hash(pk))`) rather than fixed constants, since real accounts here are never arbitrary — they're always a hash of the key.

### `runtime/src/lib.rs` / `runtime/Cargo.toml`
- `pub type Signature = MultiSignature;` → `pub type Signature = argonaut_primitives_falcon::ArgonautSignature;`. `AccountId`'s derivation line (`<<Signature as Verify>::Signer as IdentifyAccount>::AccountId`) didn't need to change — it now resolves through `ArgonautSigner` instead of `MultiSigner`, transparently.
- Added `impl pallet_falcon_registry::Config for Runtime` (`Currency = Balances`, `KeyDeposit = 1 × EXISTENTIAL_DEPOSIT`) and `FalconRegistry: pallet_falcon_registry` to `construct_runtime!`.

### `node/`
- `service.rs`: `type HostFunctions = sp_io::SubstrateHostFunctions;` → `(sp_io::SubstrateHostFunctions, argonaut_primitives_falcon::falcon_crypto::HostFunctions)` — this was a comment-only placeholder before ("Phase A: extend with Falcon host functions"); now it's real.
- `Cargo.toml`: added the `argonaut-primitives-falcon` dependency needed for the above.

## Deferred (explicitly out of scope this session)

Per the investigation this session did into the vendored polkadot-sdk `stable2412` checkout (`substrate/primitives/runtime/src/traits/transaction_extension/`):

- **`CheckFalconSignature` TransactionExtension (registry-mode).** `TransactionExtension` is the mandatory replacement for the deprecated `SignedExtension` in this SDK version; there's no derive macro, extensions are hand-written (`impl_tx_ext_default!` macro only stubs trivial default methods). The closest real precedent in the vendored checkout is `substrate/frame/verify-signature/src/extension.rs`'s `VerifySignature` extension — it authorizes the origin itself from inside `validate()` via `OriginTrait::set_caller_from_signed`, exactly the shape `CheckFalconSignature` needs for the registry-mode case (look up the key via `KeyProvider`, reject `InvalidTransaction::BadSigner` if unregistered, otherwise verify and authorize). This is a well-scoped follow-up, not a research gap — the pattern is now understood, just not yet written.
- **`keygen` CLI subcommand.** Confirmed the stock `sc_cli::KeySubcommand`/`CryptoScheme` enum is closed (hardcoded `Ed25519 | Sr25519 | Ecdsa`, in `substrate/client/cli/src/commands/arg_enums.rs`) — cannot be extended from outside that crate. A bespoke subcommand is needed, not an extension of the existing one.
- **`argonaut-sign` offline signer binary.** Not started; doesn't exist as a workspace member yet.
- **Benchmarking** for `pallet-falcon-registry` (doc §2.3 mentions it) — no `frame-benchmarking` dependency in the pallet yet.
- **Auto-register-on-first-bootstrap-tx** (doc §2.3's "config-gated" optional behavior) — depends on the TransactionExtension existing first.

## Notes / judgment calls

- `ArgonautSignature::Sr25519` (the dev-only escape hatch) is gated on `#[cfg(feature = "std")]`, not a dedicated feature flag. This means it's compiled out of every wasm build (wasm never sets `std`) — the safety property the doc cares about — but it *is* still available in every native/std build, including release. A dedicated `dev-signatures` feature would be more precise; deferred as a nice-to-have, not a correctness issue for now.
- `FalconPublic`/`FalconSignature` use `BoundedVec` (not plain `Vec`) so they satisfy `MaxEncodedLen`, required by `#[pallet::storage]`. This matches the doc's original §2.2 spec — the Phase 0 scaffolding commit had silently simplified this away to plain `Vec<u8>`, which was one of the Phase 0 build-blocking bugs.
