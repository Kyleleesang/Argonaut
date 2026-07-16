# wallet/ — Falcon signing outside the node

The chain's Falcon-512 signatures mean no existing wallet (extension or CLI)
can sign Argonaut transactions. These crates close that gap per
`docs/FRONTEND_DAPP_SPEC.md` §4 / binding D2: **one Rust signing
implementation, multiple frontends, zero drift**.

## Crates

- **`signer-core`** — pure-Rust Falcon-512 keystore + signing. Deliberately
  Substrate-free so it compiles unchanged for `wasm32-unknown-unknown`. The
  node's `falcon-key` / `falcon-sign` subcommands and the
  `submit_transfer_registry` example are thin shells over it.
- **`signer-wasm`** — wasm-bindgen wrapper, the seed of the
  `@argonaut/signer` browser package. Stateless string API at CLI parity:
  `generate_keystore` / `keystore_address` / `derive_address` /
  `sign_payload` (bootstrap mode).

## Build

```bash
cargo test -p argonaut-signer-core                              # byte-compat pins
cargo build -p argonaut-signer-wasm --target wasm32-unknown-unknown
wasm-pack build wallet/signer-wasm --target web                 # JS package (needs wasm-pack)
```

## How byte-compat with the chain is guaranteed

`signer-core` can't link `sp-core` (wasm target), so its tests pin every
encoding against the real thing natively:

- signature envelope bytes vs `ArgonautSignature::encode()`
- SS58 addresses vs `sp_core::crypto::Ss58Codec` (both prefix forms)
- end-to-end: a signed envelope decoded as `ArgonautSignature` passes
  `Verify::verify` — the runtime's exact acceptance path, through the
  `falcon_crypto::verify` host function
- golden vectors (seeded RNG → hardcoded hashes) that fail loudly if an
  fn-dsa bump changes key/signature bytes — the FN-DSA pre-standard
  migration tripwire (spec §4.4/§10)

## Not here yet (next milestones)

- Extrinsic builder: nonce/era/tip/genesis assembly + registry-mode signing
  in wasm (F2; reference construction lives in
  `node/examples/submit_transfer_registry.rs`)
- In-memory key handles, approval popups, secret zeroization — custody
  ladder Tier 1 (spec §9.1)
