# Implementation A Progress Tracker — Falcon-512 Signatures + Registry

Tracks status against [`ARGONAUT_ARCHITECTURE_GUIDE.md`](./ARGONAUT_ARCHITECTURE_GUIDE.md) §2 ("Implementation A — Falcon-512 Signatures + Public-Key Registry"). Lives on the `Base` branch.

## Status: functionally complete. Both signing modes work end-to-end, tested, and wired into the runtime and CLI.

| §2.5 acceptance criterion | Status |
|---|---|
| Sign/verify round-trip; corrupted sig/pk/msg each fail; `pk_hash` matches independent blake3 | ✅ `primitives/falcon` — 10 unit tests against real `fn-dsa` keypairs |
| Bootstrap tx succeeds and key appears in storage | ✅ registry pallet tests (11 tests) |
| Registry-mode tx smaller; rejected in pool if unregistered | ✅ `CheckFalconSignature` `TransactionExtension` (5 runtime-level tests) |
| Weight/size assertion test (registry-mode ≤ 850 bytes) | ✅ `runtime/src/tests.rs` — real Falcon-512 extrinsics, encoded-size assertion |
| wasm build unaffected | ✅ live-verified 2026-07-07 (a real leak was found and fixed — see fix plan A-1) |
| Auto-register-on-first-bootstrap-tx (optional, config-gated) | ⬜ not implemented |

## What's here, for orientation (not a to-do list — read the code for detail)

- `primitives/falcon`: `ArgonautSignature`/`ArgonautSigner` (real `sp_runtime_interface` host functions for Falcon verify/pk_hash), `dev-signatures`-gated `Sr25519` escape hatch for dev accounts.
- `pallets/falcon-registry`: `KeyProvider<AccountId>` trait, register/deregister extrinsics, benchmarks.
- `runtime/src/extensions.rs`: `CheckFalconSignature` — the registry-mode `TransactionExtension`, modeled on `pallet-verify-signature`'s `Preamble::General` pattern. First in the `SignedExtra` tuple.
- `node/src/keygen.rs`, `node/src/falcon_sign.rs`: `falcon-key generate/inspect`, `falcon-sign` CLI subcommands (bootstrap-mode only; keystore optionally passphrase-encrypted).
- `node/examples/submit_transfer_registry.rs`: registry-mode transaction builder (assumes the signer is already registered on-chain via a bootstrap-mode `register_key` call).

## Remaining

- **Auto-register-on-first-bootstrap-tx** (fix plan item 3): doc §2.3's optional, config-gated behavior. Would live inside `CheckFalconSignature`'s currently-no-op `Disabled`/bootstrap path.

See [`IMPLEMENTATION_AB_FIXPLAN.md`](./IMPLEMENTATION_AB_FIXPLAN.md) for the consolidated, current to-do list across Implementation A and B.
