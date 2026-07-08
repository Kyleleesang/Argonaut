# Argonaut

```
###  ####   ####  ###  #   #  ###  #   # #####
#   # #   # #     #   # ##  # #   # #   #   #
#   # ####  # ### #   # # # # #   # #   #   #
##### # #   #   # #   # #  ## ##### #   #   #
#   # #  #  #   # #   # #   # #   # #   #   #
#   # #   #  ####  ###  #   # #   #  ###    #

     a post-quantum secure, scalable blockchain for the decentralized internet
```

Argonaut is a [Substrate](https://substrate.io/)-based blockchain built around two ideas that
don't normally travel together: **post-quantum signatures by default**, and **consensus that
runs entirely client-side, on many parallel chains at once, decoupled from block execution** —
built to scale as the settlement and data layer for a genuinely decentralized internet, not just
a faster ledger.

It's one machine, built up as a stock Substrate runtime with two working subsystems today and two
more planned:

| Subsystem | What it does | Status |
|---|---|---|
| Falcon-512 signatures + registry | Every account signs with a lattice-based, post-quantum signature scheme instead of sr25519/ed25519. An on-chain registry lets repeat senders drop the ~897-byte public key from the wire format. | **Done** — functional, tested, live-verified |
| Prism consensus | Replaces Aura/GRANDPA with a client-side port of the [Prism](https://arxiv.org/abs/1810.08092) protocol: ~100 parallel voter chains vote on a proposer tree, decoupling transaction throughput from confirmation latency. Substrate becomes a deterministic *output* of Prism, not the consensus engine itself. | **Done** — functional, tested, live-verified |
| Succinct proof aggregation | Aggregate many transactions' proofs into one succinct proof to shrink long-term chain history. | Scaffolded (`primitives/agg`, `pallets/agg-verify`, `client/aggregator`), not yet built out |
| Decentralized storage | An [iroh](https://iroh.computer/)-backed data plane with an on-chain control plane (provider registry, deal lifecycle, availability proofs, payment channels). | Specified (`docs/IMPLEMENTATION_D_SPEC.md`), not yet started |

This README covers building, running, and navigating the repository. For the detailed design
and as-built behavior of the signature and consensus subsystems, see
**[`docs/ARCHITECTURE.md`](./docs/ARCHITECTURE.md)**. The remaining two subsystems will get the
same treatment once they're built out.

---

## How the pieces fit together

```
                                   ┌──────────────────────────┐
                                   │   Users / wallets         │
                                   │   (Falcon-512 keypairs)   │
                                   └────────────┬─────────────┘
                                                │ signs an extrinsic
                                                │ (bootstrap or registry mode)
                                                ▼
 ┌────────────────────────────┐   ┌─────────────────────────────────────┐
 │ Signatures & accounts        │   │ Consensus                             │
 │ Falcon-512 + registry       │──▶│ Prism consensus (client-side)        │
 │ (runtime + pallet)          │   │  mining · DAG · confirmation         │
 └────────────────────────────┘   └────────────────┬──────────────────────┘
                                                    │ confirmed leader level
                                                    │ (ordered, deduped txs)
                                                    ▼
                                   ┌─────────────────────────────────────┐
                                   │ node/src/bridge.rs                    │
                                   │ the ONE place Prism meets Substrate    │
                                   └────────────────┬──────────────────────┘
                                                    ▼
                                   ┌─────────────────────────────────────┐
                                   │ Substrate runtime (FRAME)              │
                                   │ pallet-balances, pallet-falcon-registry│
                                   └─────────────────────────────────────┘

                    ┌ ─ ─ ─ ─ ─ ─ ─ ─ ─┐         ┌ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ┐
                     Proof aggregation                Decentralized storage
                     (planned)                          (planned)
                    └ ─ ─ ─ ─ ─ ─ ─ ─ ─┘         └ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ┘
```

The binding architectural rule (see `docs/ARGONAUT_ARCHITECTURE_GUIDE.md` §3.1 and
`docs/ARCHITECTURE.md`): **Prism decides transaction *order*; Substrate decides transaction
*effect*.** Nothing about a transaction's validity or state effect is ever decided by Prism, and
nothing about ordering is ever decided by Substrate/Aura-style block production — there is none.
The bridge is a one-way funnel: confirmed Prism leader → exactly one Substrate block.

---

## Repository layout

```
Argonaut/
├── node/                        binary crate: argonaut-node
│   ├── src/
│   │   ├── service.rs             wires the client, bridge, miner, and network together
│   │   ├── bridge.rs               the one place Prism and Substrate touch
│   │   ├── chain_spec.rs          --dev and --chain local genesis configs
│   │   ├── cli.rs                 --prism-prune / --prism-k-prune + subcommands
│   │   ├── command.rs             subcommand dispatch
│   │   ├── keygen.rs               falcon-key subcommand (keystore generate/inspect)
│   │   └── falcon_sign.rs          falcon-sign subcommand (bootstrap-mode offline signing)
│   └── examples/
│       ├── submit_transfer.rs           bootstrap-mode transaction builder
│       └── submit_transfer_registry.rs  registry-mode transaction builder
│
├── runtime/                      FRAME runtime crate: argonaut-runtime
│   └── src/
│       ├── lib.rs                  construct_runtime!, SignedExtra tuple
│       └── extensions.rs           CheckFalconSignature TransactionExtension
│
├── primitives/
│   ├── falcon/                    signature types, host functions, dev-signatures gate
│   ├── prism/                     block types, sortition, Merkle helpers, retargeting
│   └── agg/                       proof aggregation, scaffolded, not built out yet
│
├── pallets/
│   ├── falcon-registry/           on-chain Falcon public-key registry
│   └── agg-verify/                proof aggregation, scaffolded, not built out yet
│
├── client/                        the Prism consensus engine — entirely client-side
│   ├── prism-db/                    RocksDB column families, checkpoints, pruning storage
│   ├── prism-chain/                 the DAG state machine (votes, fork-choice, orphans)
│   ├── prism-miner/                  mining loop / template assembly / sortition
│   ├── prism-network/                gossip + sync protocol over sc-network
│   ├── prism-ledger/                 confirmation stages, BuildLedger, restart recovery
│   └── aggregator/                  proof aggregation, scaffolded, not built out yet
│
├── docs/
│   ├── ARCHITECTURE.md                  detailed design + as-built behavior of the machine
│   ├── ARGONAUT_ARCHITECTURE_GUIDE.md   original design spec for the whole system
│   └── IMPLEMENTATION_D_SPEC.md         decentralized storage subsystem's spec (not yet built)
│
└── scripts/                       devnet helper scripts (currently unmodified template boilerplate)
```

---

## Building

Argonaut pins one `polkadot-sdk` release (`stable2412`) for every workspace crate — see
`docs/rust-setup.md` for toolchain setup, then:

```sh
cargo build --release
```

This builds the `argonaut-node` binary, including the wasm runtime. A full release build takes
several minutes; `cargo check` / `cargo build` (debug) are much faster for iteration.

Run the test suite:

```sh
cargo test --workspace
```

> **A note on trust in test output:** don't pipe `cargo test` through `tail` (or similar) and
> trust the exit code — the pipe reports its own last command's exit code, not cargo's, and a
> real test failure can silently read as success. Redirect to a file and check cargo's own exit
> code, or read the `test result: ... FAILED` lines directly.

## Running a single development node

```sh
./target/release/argonaut-node --dev --validator
```

`--dev` uses a single well-known genesis with pre-funded dev accounts. `--validator` is required
for this node to actually mine Prism blocks — without it, a node will sync and gossip but never
produce anything of its own (mining is gated on `config.role.is_authority()` in
`node/src/service.rs`).

Because Prism confirmation depends on ~100 independent voter chains reaching k-deep agreement
(see `docs/ARCHITECTURE.md`), a **single** node confirms levels far more slowly than a
healthy multi-node network — this is expected, not a bug.

## Running a local two-node devnet

Prism's request/response sync protocol is best-tested under the `libp2p` network backend in this
`polkadot-sdk` pin.

```sh
# Node 1
./target/release/argonaut-node --chain local --validator \
  --base-path /tmp/argonaut/node1 --port 30401 --rpc-port 9945 \
  --network-backend libp2p --rpc-cors all \
  --node-key 0000000000000000000000000000000000000000000000000000000000000001

# Node 2 (separate terminal) -- bootnode address uses node 1's fixed-node-key peer id
./target/release/argonaut-node --chain local --validator \
  --base-path /tmp/argonaut/node2 --port 30402 --rpc-port 9946 \
  --network-backend libp2p --rpc-cors all \
  --node-key 0000000000000000000000000000000000000000000000000000000000000002 \
  --bootnodes /ip4/127.0.0.1/tcp/30401/p2p/12D3KooWEyoppNCUx8Yx66oV9fJnriXwCcXwDDUA2kj6vnc6iDEp
```

Fixed `--node-key`s make the peer ID deterministic across restarts, which is convenient for
scripting bootnode addresses — don't reuse these two keys for anything beyond a local devnet.

Check that both nodes agree on the chain, byte-for-byte:

```sh
curl -s -H "Content-Type: application/json" \
  -d '{"id":1,"jsonrpc":"2.0","method":"chain_getBlockHash","params":[1]}' \
  http://127.0.0.1:9945
# compare against the same call against :9946 -- must match exactly
```

## Key management and transactions

```sh
# Generate a Falcon-512 keystore (optionally passphrase-encrypted)
./target/release/argonaut-node falcon-key generate --output alice.falcon.json

# Inspect a keystore (prints the derived AccountId)
./target/release/argonaut-node falcon-key inspect alice.falcon.json

# Sign a payload offline (bootstrap mode)
./target/release/argonaut-node falcon-sign alice.falcon.json <hex-payload>
```

Two example binaries build and submit full transactions end to end against a running node:

```sh
cargo run --release --example submit_transfer -- alice.falcon.json
cargo run --release --example submit_transfer_registry -- alice.falcon.json
```

`submit_transfer` uses bootstrap mode (signature + inline public key, no registry lookup needed).
`submit_transfer_registry` uses registry mode (signature only, ~850 bytes smaller) — this
requires the signer's key to already be registered on-chain via a bootstrap-mode
`falcon_registry::register_key` call first.

## Documentation map

- **[`docs/ARCHITECTURE.md`](./docs/ARCHITECTURE.md)** — thorough, as-built design documentation
  for the machine as it stands today: every crate's role, the Prism DAG structure, sortition,
  confirmation, the Substrate bridge, networking, pruning, and the transaction lifecycle end to
  end. Start here to understand *how the system actually works*.
- **[`docs/ARGONAUT_ARCHITECTURE_GUIDE.md`](./docs/ARGONAUT_ARCHITECTURE_GUIDE.md)** — the
  original design spec covering the whole system, including the subsystems not yet built. Useful
  for intent and for their eventual scope.
- **[`docs/IMPLEMENTATION_D_SPEC.md`](./docs/IMPLEMENTATION_D_SPEC.md)** — the decentralized
  storage subsystem's detailed spec, ready to build from when its turn comes.

## Project conventions worth knowing

- **Live-verify, don't just unit-test.** Every consensus-relevant bug found on this project so
  far was found by actually running a multi-node devnet, not by a passing test suite. Unit tests
  are necessary but treated as insufficient on their own for anything touching block production,
  confirmation, or networking.
- **One `polkadot-sdk` pin (`stable2412`) for the whole workspace.** Never mix release branches
  in the same link unit.
- **Prism consensus is entirely client-side.** `client/prism-*` never depends on the Substrate
  runtime; the runtime never reaches back into Prism state. `node/src/bridge.rs` is the only
  seam, and it is one-directional: confirmed leader → Substrate block, never the other way.

## License

Public domain — see [`LICENSE`](./LICENSE) (Unlicense).
