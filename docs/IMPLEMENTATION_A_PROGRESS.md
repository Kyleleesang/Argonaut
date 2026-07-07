# Implementation A Progress Tracker — Falcon-512 Signatures + Registry

Tracks status against [`ARGONAUT_ARCHITECTURE_GUIDE.md`](./ARGONAUT_ARCHITECTURE_GUIDE.md) §2 ("Implementation A — Falcon-512 Signatures + Public-Key Registry"). Lives on the `Base` branch.

## Status: functionally complete. Both signing modes work end-to-end, tested, and wired into the runtime and CLI.

| §2.5 acceptance criterion | Status |
|---|---|
| Sign/verify round-trip; corrupted sig/pk/msg each fail; `pk_hash` matches independent blake3 | ✅ `primitives/falcon` — 10 unit tests against real `fn-dsa` keypairs |
| Bootstrap tx succeeds and key appears in storage | ✅ registry pallet tests (11 tests) |
| Registry-mode tx smaller; rejected in pool if unregistered | ✅ `CheckFalconSignature` `TransactionExtension` (5 runtime-level tests) |
| Weight/size assertion test (registry-mode ≤ 850 bytes) | ⬜ not written — mechanism is in place (no inline `pk` in registry-mode), just not asserted |
| wasm build unaffected | ✅ |
| Auto-register-on-first-bootstrap-tx (optional, config-gated) | ⬜ not implemented |

## What's here, for orientation (not a to-do list — read the code for detail)

- `primitives/falcon`: `ArgonautSignature`/`ArgonautSigner` (real `sp_runtime_interface` host functions for Falcon verify/pk_hash), `dev-signatures`-gated `Sr25519` escape hatch for dev accounts.
- `pallets/falcon-registry`: `KeyProvider<AccountId>` trait, register/deregister extrinsics, benchmarks.
- `runtime/src/extensions.rs`: `CheckFalconSignature` — the registry-mode `TransactionExtension`, modeled on `pallet-verify-signature`'s `Preamble::General` pattern. First in the `SignedExtra` tuple.
- `node/src/keygen.rs`, `node/src/falcon_sign.rs`: `falcon-key generate/inspect`, `falcon-sign` CLI subcommands (bootstrap-mode only; keystore optionally passphrase-encrypted).

## Remaining / possible follow-ups

- ~~**`ArgonautSignature::Sr25519` gating**~~ — **done**: behind a real `dev-signatures` Cargo feature, off by default (fixplan closed-item A-1). **Still open:** an explicit live check that a production wasm build (no `dev-signatures`) actually rejects an sr25519-signed extrinsic.
- **Weight/size assertion test** (fixplan A-4): assert the encoded byte-size delta between bootstrap-mode and registry-mode extrinsics (doc §2.5 point 3, "≤ 850 bytes"). Trivial once someone sits down to write it — `encode().len()` on a built extrinsic of each mode.
- **Auto-register-on-first-bootstrap-tx** (fixplan A-4): doc §2.3's optional, config-gated behavior. Would live inside `CheckFalconSignature`'s currently-no-op `Disabled`/bootstrap path.
- **Registry-mode transaction builder** (fixplan A-3): nothing assembles a full `General`-preamble `UncheckedExtrinsic` end-to-end (genesis hash, nonce, era, tip) for registry mode — `submit_transfer.rs` only covers bootstrap mode. `CheckFalconSignature` itself is correctness-tested at the `validate()` level; this is purely missing tooling.

See [`IMPLEMENTATION_AB_FIXPLAN.md`](./IMPLEMENTATION_AB_FIXPLAN.md) for the consolidated, current to-do list across Implementation A and B.
