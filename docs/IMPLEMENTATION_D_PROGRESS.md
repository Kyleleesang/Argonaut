# Implementation D — Open Work, Concerns & Decisions

The running counterpart to the spec of record,
[`IMPLEMENTATION_D_SPEC.md`](./IMPLEMENTATION_D_SPEC.md) (decentralized
storage platform). Trimmed 2026-09-30 to what is still open or still binding.
The build/bug history of the closed milestones (D1–D3, the D2 live beacon
check, and the D4 push-protocol work) is in git log: `git log -- docs/IMPLEMENTATION_D_PROGRESS.md` shows every version of this file.

## Milestone status

| Milestone | State |
|---|---|
| **D1** — `primitives/storage` + `argonaut-store encode` + host fn | ✅ done |
| **D2** — pallets (registry/deals/proofs/channels) + runtime wiring | ✅ done, live-verified (beacon snapshot byte-exact, 2026-07-16) |
| **D3** — chain-client + provider daemon | ✅ done, live-verified end to end (2026-07-17) |
| **D4** — iroh shard-push + gated retrieval, `deal-create`/`fetch` CLI | 🟡 **code done, partially live-verified** (see below) |
| **D5** — repair | not started. Builds on D4's `fetch` |

---

## D4 — what's left

**Live-verified 2026-07-22** against a real 2-provider devnet:
- provider registration
- `deal-create` (it waits until `create_deal` is mined)
- `accept-shard`
- the real iroh shard-push protocol: bytes landed in the assignee's blob store under the exact root computed at `encode` time.

**Not yet live-verified: `activate_shard` and `argonaut-store fetch`.** They are covered by unit and integration tests only. Two non-code blockers stopped the last run; plan around both:

1. **The assignment window vs. devnet extrinsic latency.**
   - The test-only `EPOCH_DURATION_BLOCKS=4` × `ASSIGN_TIMEOUT_EPOCHS=4` gives a 16-block window.
   - Single-node PoW devnet extrinsics sometimes take 20+ minutes to land, so an unlucky `accept_shard` missed the window (`AssignWindowClosed`, reproduced twice).
   - Use 20–40 blocks/epoch next time, or the production 100 (a 400-block window).
2. **Provider daemons don't pick up new assignments.** `responder.rs`'s `held` list is populated once at startup from `--assignment deal_id:shard_idx`.
   - Restart the daemon with that flag right after `accept-shard` mines.
   - Alternatively, build auto-discovery of newly accepted assignments. That's a real improvement nobody has attempted yet.

**Security gap: `RetrievalHandler` never verifies `RetrievalRequest::sig`.**
- The per-stride metering works.
- But anyone who knows a `channel_id` can send a garbage signature with a large enough `cumulative` and get served for free.
- **Fix:** before calling `falcon_crypto::verify`, resolve the channel's `from` account and its Falcon key on-chain. Mirror `settle`'s inline-pk / registry-lookup, as a chain-backed authority trait parallel to `ShardAuthority`.
- **Do not treat retrieval as production-safe until this is closed.**

**Documented scope cuts (deliberate, not bugs):**
- `fetch` needs a local `manifest.scale`; there's no "obtain the manifest from any provider" step yet.
- `price_per_stride` is agreed out-of-band; nothing prices retrieval on-chain.
- It opens one payment channel per shard fetched, instead of reusing one per session.
- `deal-create`'s provider discovery is just "first n active".
- `blobs.rs::read_range` serves raw ranges, not per-stride bao-verified slices (see `protocol.rs`'s module doc).

---

## Open concerns (watch these)

- **C-D1.2 — BLAKE3 root == iroh-blobs root.** On-chain `ShardSpec.root` is `blake3::hash(shard)`. D4's push confirmed that the provider stores bytes under that root, but the provider uses its own `DiskBlobStore`. Nothing has yet asserted `blake3::hash(shard) == iroh-blobs add_bytes root`. Close this if the daemon ever serves via a real iroh-blobs store.
- **C-D1.3 — Reed–Solomon shard size.** `reed-solomon-simd` needs shard sizes to be a multiple of 64 bytes (≥64). This holds only because `shard_size` is a multiple of `CHUNK_SIZE = 1024`. Keep those two coupled.
- **C-D1.4 — `verify_slice` trusts its bounds.** Pallets must enforce `chunk_len == 1` and `chunk_start < num_chunks` *before* calling the host function. Every new proof-checking call site has to do the same.
- **C-D1.5 — `saw_leaf` is the success signal.** A 0-length range returns false. That is acceptable and defensive; just don't be surprised by it.
- **C-D2.3 — Slash to a dead account is silently lost** (`repatriate_reserved` won't pay into a non-existent account and returns the remainder, which is ignored). `report_missed` splits the slash 50/50 to reporter and client. Before mainnet, handle the remainder, e.g. send it to the other party or a treasury.
- **C-D2.4 — A leftover `repair_pot` on `terminate`/`close` stays in the pallet sub-account.** Sweep it at close.
- **C-D2.5 — A repair can overwrite a faulted provider's unclaimed earnings.** `accept_shard` on a `Faulted` slot replaces the assignment. Document "claim before you're replaced", or snapshot the amount owed at fault time.
- **C-D2.6 — Stake can drop below the 2× invariant after a slash.** The rule is only enforced at `accept`. This is accepted, since the slash is itself the punishment.

---

## Decisions (recorded so they aren't silently reversed)

- **DEC-D1.a** — `argonaut-store` is native/std-only and must never become a runtime dependency. Only `primitives/storage` (no_std, host-fn body behind `std`) crosses into the runtime.
- **DEC-D1.b** — The manifest stores `blake3::hash` shard roots computed without iroh. Encoding is pure.
- **DEC-D1.c** — On-disk layout is `shard_{i:02}.bin` + `manifest.scale`, where `i` is both the `ShardIndex` and the RS shard index (data `0..k`, parity `k..n`).
- **DEC-D1.d** — `decode` takes `&[Option<Vec<u8>>]` of length n. `fetch` builds that vector from whatever ≥k shards it retrieves.
- **DEC-D1.e** — Where libraries differed from the spec sketch, the spec's frozen signatures were kept and only the internals adapted.
- **DEC-D2.a** — Cross-pallet interface traits live in `primitives/storage::interfaces` and are frame-free. This avoids a deals↔proofs crate cycle and keeps the primitives light for the daemon/SDK.
- **DEC-D2.b** — `EpochProvider` exposes `beacon(epoch)`, so `storage-deals` never imports the proofs crate.
- **DEC-D2.c** — A fault decrements `activated_count` and a repair increments it, keeping the invariant `activated_count == n ⇔ all shards live`. "First full activation" is detected by `state == Assigning`, so a repair never re-triggers vesting.
- **DEC-D2.d** — `Lost` keeps escrow reserved until `close_deal`. Surviving providers can still claim, and the remainder is unreserved at close.
- **DEC-D2.e** — A repair pays the whole `repair_pot` to the first repairer. Acceptable for v1.
- **DEC-D2.f** — `note_fault` runs twice on an escalating miss. Harmless, because `faults` is informational only.
- **DEC-D2.g** — The storage pallets are left out of `define_benchmarks!` until they have real benchmarks (`submit_proof`/`settle` need live deal/voucher state). They will need real ones for E1's weights.
- **DEC-D3.a** — The daemon reads the beacon from on-chain `StorageProofs::Beacons` rather than recomputing it, so its copy of the formula can never drift from the pallet's.
- **DEC-D4.a** — Deal creation is two steps: `create_deal` (poll until mined), then `push_shards_for_deal` (poll each `ShardAssignments` until `Accepted`, then push to the *real* assignee). `author_submitExtrinsic` is fire-and-forget, and `accept_shard` is first-come-first-served, so the client can't pick the holder.
- **Runtime indices** — The 4 storage pallets come after `Sudo` in `construct_runtime!`. Any manual storage-key or call-index computation must use live metadata, not hard-coded indices.
