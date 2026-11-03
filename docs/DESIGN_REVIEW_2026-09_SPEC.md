# Design Review 2026-09 — findings & remediation spec

**Status: ACTIVE (2026-09-30).** This is the output of a whole-repo design review by five Claude Opus 5.5 reviewers. Each read one area in depth: the Prism consensus core; the node, network and bridge; the runtime and Falcon signing; storage (Implementation D); and cross-cutting design plus the plans. Everything was read-only against commit `f4aa841` plus the uncommitted B-18 work. The coordinator re-checked the most surprising critical claims against the code (R-3, R-4, R-1).

This file is the tracker for the resulting work. Each item has:
- an ID
- a severity
- the flaw
- evidence (file:line as of the review; line numbers will drift)
- the fix
- rough effort
- a **Status**

Items marked **DECISION** change genesis, the block format, or the safety math, so they need the owner's sign-off before implementation (fixplan ground rule 3). Everything else can be implemented directly.

The fixplan's ground rules apply here too: live-verify, commit per item with the ID in the message, and don't trust a piped `cargo test` exit code. See [IMPLEMENTATION_AB_FIXPLAN.md](./IMPLEMENTATION_AB_FIXPLAN.md).

**Note:** every devnet so far is disposable, so a genesis reset is acceptable for any item here. It won't be after E0.

---

## Recommended order

1. **Tier 1 (R-1 … R-6):** consensus safety bugs with clear fixes. Do these now.
2. **B-18,** reworked per N-6. It depends on R-1/R-2: with its current fix, the acceptance run would trigger R-2's self-fork.
3. **Tier 2 decisions (D-1 … D-11).** Decide them together; several set the header format and genesis.
4. **Network robustness (N-*)** and **ops (O-*)**. O-1 is a one-hour fix and can go anytime.
5. **Storage (S-*)** before D4/D5 are called done.
6. **Runtime/wallet (W-*)** before any wallet reaches real users. W-1 and W-4 are breaking wire changes and should land before E0.

---

## Tier 1 — consensus safety (implement now)

### R-1 — Network-synced Substrate blocks are finalized without checking them against Prism — CRITICAL
Three reviewers found this independently.

**Flaw:** `PrismBridgeVerifier::verify` sets `finalized = true` and `ForkChoiceStrategy::Custom(true)` on every network block. Re-execution is its only safety check (`node/src/bridge.rs:98-120`). No block-announce validator is set (`service.rs:164`). Re-execution only proves a block is a *valid* state transition, not the *Prism-ordered* one.

**Scenario:** a peer with no hash power authors blocks containing any extrinsics, in any order, with any timestamp. It announces a higher best block, and victims import those blocks as finalized and best. The honest chain is then rejected, a permanent fork. After a restart, `PrismLedger::recover(best_number)` adopts the fake height as the ledger cursor, so the node's own bridge stops confirming.

**Fix (design):** Prism is the only source of truth. A network block may never become best or finalized on its own.
1. The verifier rejects, with a penalty, any block without exactly one well-formed `prsm` `LeaderDigest` whose `level == header.number`.
2. Otherwise it imports the block as **non-best, non-final** (`Custom(false)`, `finalized = false`). Honest blocks then act as a cache, and malicious ones become dead side forks that are discarded on finality.
3. The bridge builds level N itself. Construction is deterministic, so an honest peer's block has the same hash, and the import returns `AlreadyInChain`. The bridge must therefore import with `import_existing = true`, `finalized = true`, `Custom(true)` so its own derivation promotes the block to best and final.
4. Consequence: a node follows the chain only by running Prism. There is no trusted "follow via Substrate sync" mode; any such mode would have to be an explicit, documented opt-in.

**Effort:** 1–3 days, plus a multi-node live run. **Status:** OPEN.

### R-2 — The bridge builds on `best_hash` instead of the block for level − 1 — CRITICAL
**Flaw:** `build_and_import_block` parents on `client.info().best_hash` (`bridge.rs:213-221`). It never checks that the parent's digest is level − 1, or whether a block for this level already exists. The design silently assumes block number == Prism level.

**Scenario:** sync delivers block N before the local ledger confirms level N. The node then builds level N at height N+1, which is a self-inflicted permanent fork. The B-18 acceptance run hits this directly: node3's sync races far ahead of its Prism backfill.

**Fix:**
- Target height is `level`. Parent is the canonical block at `level − 1`, and its digest must say `level − 1`.
- If a block at `level` already exists on our canonical chain:
  - digest leader matches: skip;
  - otherwise: halt loudly (`log::error!`, then stop the bridge task). Never build over it.
- A failed `build_and_import_block` must not advance past the level (see R-6).
- **Missing test:** "sync imports N, then the ledger confirms N" must not produce block N+1.

**Effort:** hours, together with R-1. **Status:** OPEN.

### R-3 — Merkle proofs aren't bound to a slot index or tree size: one PoW can carry many bodies — CRITICAL
**Flaw:** `validate_block` (`primitives/prism/src/lib.rs:427-438`) calls `verify_merkle_proof` (`:349-359`), which only checks that the leaf is *somewhere* in the tree. The paper's Alg. 1 returns `parentMT.proof(i)` / `contentMT.proof(i)` for the sortitioned index *i*.

**Scenario:** block identity is `blake3(SCALE(header))`. An attacker commits several contents under one header, with conflicting `tx_refs` or different `votes`, and serves body A to half the network and body B to the other half. Both halves agree on the hash H but build different ledgers, so the bridge finalizes different blocks: a permanent split. This costs one proposer PoW.

**Fix:**
- Derive the expected index from the sortition result: voter chain c → `c`, tx → `m`, proposer → `m + 1`.
- Require `proof.len() == ceil(log2(m + 2))`, and require the direction bits to encode that index. Do this for both proofs.
- Voter blocks: also require `block.parent == voter_parent`.
- Honest miners already produce correct proofs (`prism-miner/src/lib.rs:137-140,178-179`), so this is validation-only with no wire change. Leaf/node domain separation is optional, would be a format change, and is deferred.

**Effort:** ½ day plus tests. **Status:** OPEN.

### R-4 — No voter-block validity rules; stage-1 keys on each node's first-seen proposer — CRITICAL
**Flaw:**
- `insert_voter`/`add_vote` (`prism-chain/src/lib.rs:546-617`) accept any `votes` list. Nothing enforces one vote per level, contiguous levels, "not already voted by an ancestor", or that the voted proposer is known. `per_chain.insert` lets a later re-vote overwrite depth.
- `confirmed_leader_at_level` (`prism-ledger/src/lib.rs:56`) checks unanimity for `level_proposer[level]`, the *locally first-seen* proposer, not the proposer that holds the votes.

**Scenarios:**
- (a) Split. With proposers P and P′ at one level, the attacker adds double votes so every chain carries votes for both. Nodes that first saw P confirm P; nodes that first saw P′ confirm P′.
- (b) Vote accounting depends on arrival order. `remove_vote` on a duplicate re-vote deletes the chain's vote entirely.
- (c) Cheap stage-1 stall: re-vote to reset depth.
- (d) Memory DoS through unbounded garbage vote lists.

**Fix:**
- **R-4a (stopgap, now):** the stage-1 candidate is "the proposer that holds a vote from *all* m chains", with a deterministic tie-break, never `level_proposer`.
- **R-4b (real fix):**
  - Store `last_voted_level` in `VoterMeta`.
  - A voter block's votes must cover exactly levels `last+1 ..= L`, one *known* proposer per level, sorted by level.
  - Orphan the block until the voted proposers arrive.
  - Index votes as `(chain, level) → (proposer, depth)`.
  - Bump the checkpoint version.
  - Pruning keeps `last_voted_level` at the frontier.
  - Honest miners must already produce votes of this shape; verify that first.

**Effort:** 4a about 1 h; 4b 2–3 days. **Status:** OPEN.

### R-5 — Pruning drops the first-seen proposer, not the confirmed leader; BuildLedger skips missing bodies silently — HIGH
**Flaw:** `prune_confirmed_bodies` (`prism-chain/src/lib.rs:~919`) treats `level_proposer[level]` as the leader. `build_ledger_delta_for_leader` / `expand_proposer_tx_refs` skip missing bodies (`if let Ok(Some(..))`, `prism-ledger/src/lib.rs:245,278`), and the leader timestamp falls back to `unwrap_or(0)` (`:216`).

**Scenario:** stage-2 confirms P while a pruning node first saw P′. That node prunes the wrong subtree, emits fewer transactions, and silently forks.

**Fix:**
- Prune by the ledger's `PROPOSER_LEADER_SEQUENCE`.
- Pass the confirmed leader per level into `prune`; this keeps the crate boundary, since prism-chain still takes it as a parameter.
- A missing body that BuildLedger needs is a hard stop: don't advance, and log an error. Never emit a partial delta.

**Effort:** ½ day. **Status:** OPEN.

### R-6 — Transactions are marked "emitted" before execution, and the mark isn't atomic with the Substrate import — HIGH
Three reviewers found this independently.

**Flaw:** `build_ledger_delta_for_leader` durably inserts `blake3(tx)` into a global emitted set before the bridge executes anything (`prism-ledger/src/lib.rs:244-257`). The bridge silently skips failed pushes (`bridge.rs:249-262`) and only logs import failures, then carries on (`:179-196`). `recover` resets the cursor from `best_number` but reloads the whole emitted set (`:145-148`).

**Scenarios:**
- (a) Stuck accounts. Nonces n and n+1 land in different transaction blocks (queue colouring is `blake3(tx) % 8`), so n+1 can be ordered first. It fails `Future` but is blacklisted forever. The pool keeps re-mining it, the ledger keeps dropping it, and the account is stuck until the user re-signs.
- (b) Crash after `advance`, before import. On restart, level N is re-confirmed with all its transactions already deduped, so the node builds an empty block N while its peers' block N has transactions. That is a fork.
- (c) Memory grows O(all transactions).

**Fix:**
- Remove the global persisted emitted set.
- Dedup within one level's delta only. Replay protection is the runtime's job: nonce plus era. A repeated or failed extrinsic is deterministically skipped by every node.
- Import failure is fatal to the bridge loop: stop and log, never advance.
- **Precondition:** confirm the runtime has no replayable bare or unsigned extrinsic path. `ValidateUnsigned` / inherents must not be accepted from ledger transactions.

**Effort:** about 1 day plus a live run. **Status:** OPEN.

---

## Tier 2 — design decisions (DECISION: owner sign-off; they set genesis or the header)

### D-1 — Stage-1 unanimity halts the ledger on ordinary honest proposer forks — CRITICAL (liveness) — DECISION
**Flaw:** the code requires all m chains to vote for *one* proposer (`prism-ledger/src/lib.rs:59-72`). The paper's conservative rule (§4.1, Alg. 2) requires every chain to have *a* k-deep vote on the level, with the leader being the most-voted proposer. Stage-2's `single_check_margin` floor (≈13.8% at ε=1e-6) means a split inside ~43–57 of 100 never resolves at any depth. With two similar miners who saw different proposers first, roughly 80% of forks land in that band. Levels confirm strictly in order, so the whole chain halts. This is probably the old B-9 "two miners never confirm". The `honest_split_tests.rs` header's claim that this is "vanishingly small" covers only exact ties.

**Options:**
- (A) The paper's rule. k becomes a per-vote depth for ε/m on a single chain, about 35–45 at β=0.25 instead of 6.
- (B) A middle rule: confirm P when its k-deep votes exceed (every vote for any rival) + (chains without a k-deep vote).

**Depends on:** R-4b. **Status:** OPEN — needs decision.

### D-2 — Stage-2's ε doesn't bound what the code does — HIGH — DECISION
**Flaw:** `lower_bound_from_depths` (`confirmation.rs:344-354`) takes the maximum over *every* k in `k_min..=max_depth`, while the margin is derived for one fixed k. The check also re-runs on every bridge wakeup. The union bound must count grid size × checks per pending level; the effective per-level bound at 1e-6 comes out above 1.

**Fix:** a fixed small geometric k-grid (about 10 values), rate-limit stage-2 re-evaluation, set ε = lifetime budget / (grid × max checks), and fix the module doc. An independent math review is recommended; the substitution `r′ = k/(2f̄_v)` is also suspect. **Status:** OPEN.

### D-3 — Miners get no reward and the header has no payout or version field — CRITICAL (economics) — DECISION
**Flaw:** `SuperHeader` = `{parents_root, content_root, timestamp_ms, nonce, difficulty}` (`primitives/prism/src/lib.rs:203-212`). There is no version and no coinbase, and fees are burned (`runtime/src/lib.rs:198`). β, the beacon-grinding argument (guide §9.5, D.6.1) and the storage beacon all assume honest hash power exists, but nothing funds it. Adding a field later changes `block_hash` inputs, which means a genesis reset.

**Fix:**
- Add `version: u8` and `payout: AccountId` (or a commitment) to `SuperHeader` before any persistent network.
- Choose the reward model: which block types are paid, and how the bridge credits them. Crediting requires R-1.
- Re-derive β and the grinding analysis.

**Status:** OPEN — needs decision.

### D-4 — The leader timestamp has no upper bound — HIGH — DECISION (value of the bound)
**Flaw:** no timestamp rule exists in `validate_block`. The bridge uses `max(leader_ts, prev + 3000)` (`bridge.rs:239-240`), so one leader can move chain time forward by years, permanently. The forward ratchet also drifts ahead of wall-clock time when levels confirm faster than one per 3 s. The dApp catalog (P-6, Vault, Keystone delays) relies on this time.

**Fix:** a deterministic clamp `ts = clamp(leader_ts, prev + min, prev + MAX_STEP)`, optionally with a median-time rule from B-14 and a gossip-time future-drift rejection. Size `MinimumPeriod` to the level rate. Amend the dApp catalog's P-6. **Status:** OPEN.

### D-5 — Consensus params aren't part of network identity — HIGH
**Flaw:**
- m, the difficulty shift, k, β and ε live in chain-spec `properties`, which don't affect the genesis hash.
- `chain_spec_genesis` is the Substrate genesis only (`service.rs:207`).
- Parsing silently falls back to defaults (`chain_spec.rs:106-143`).
- A persisted `CURRENT_DIFFICULTY_TARGET` overrides the spec's shift (`prism-chain/src/lib.rs:287-289`).
- Protocol names (`/argonaut/prism/*/1`) carry no genesis.
- Invalid ranges crash or neuter consensus: m=0 underflows in sortition, k=0 confirms at zero depth, and shift ≥ 256 gives zero difficulty.

**Fix:**
- Hash `SCALE(consensus params)` into the Prism genesis domain (or move the params into runtime genesis state).
- Hard-error on missing keys or bad ranges.
- Refuse to start on a persisted/spec difficulty mismatch while retargeting is off.
- Put the genesis hash in the protocol names.
- Enforce `k_prune ≥ max(k_deep, reorg bound)`.

**Effort:** 2–3 days. The range validation is cheap and can go now. **Status:** OPEN.

### D-6 — The equal 1/(m+2) block-type split caps throughput at ~0.35 tx/s and puts confirmation at ~19 min — HIGH — DECISION
**Flaw:** `f_v = f_t = f_p = difficulty/(m+2)` (`primitives/prism/src/lib.rs:84-91`), so only 1 in 102 mined blocks carries transactions. The plans quote "~seconds" (guide §5), ~70k tx/s and point-of-sale use (dApp catalog), and short inclusion times (frontend).

**Fix:**
- Make `f_t`/`f_p`/`f_v` genesis params, with transaction blocks dominant as in the paper.
- Measure on the multi-machine testnet before quoting any latency.
- Strike the unsupported numbers from the plans now.

**Status:** OPEN — needs decision.

### D-7 — Client-side consensus rules have no upgrade path — HIGH — DECISION
**Flaw:** the Prism rules, BuildLedger and bridge behaviour live in the node binary. There is no header version, no activation-at-level schedule, and the protocol names are unversioned. `set_code` upgrades only the runtime. The bridge hard-codes `argonaut_runtime::SLOT_DURATION / 2` at compile time (`bridge.rs:40`), so a runtime change to `MinimumPeriod` would halt old binaries.

**Fix:**
- Header `version` (shared with D-3).
- A chain-spec "Prism rules schedule" (level → rules version).
- Read runtime-dependent constants through a runtime API.
- A hard-fork runbook in E5.

**Status:** OPEN.

### D-8 — Time units disagree across D, E and the dApp catalog — MEDIUM — DECISION
Each plan uses a different clock:
- D epochs are counted in blocks.
- E2 uses `7 * DAYS` from the Aura-era `MILLISECS_PER_BLOCK = 6000`.
- The dApp catalog's P-6 says "timestamps only".

Block cadence equals Prism confirmation cadence, which is unknown. **Fix:** one decision record (block counts at a measured cadence, or bounded timestamps after D-4), then re-express the constants. **Status:** OPEN.

### D-9 — No light-client or finality-proof story — MEDIUM — DECISION
Blocks carry no justification, so verifying Prism confirmation needs the full 100-chain DAG. This affects Keystone/Anchor verification and the frontend's trusted RPC. **Fix:** decide whether a "Prism confirmation certificate" exists; if not, document trusted-full-node as the permanent model for these apps. **Status:** OPEN.

### D-10 — No key rotation or scheme migration: account = `blake3(fn-dsa 0.2 pk)` forever — HIGH — DECISION
**Flaw:** the registry only accepts `blake3(pk) == caller` (`pallets/falcon-registry/src/lib.rs:96-111`), and there's no rotate call. The E spec's E.4 benchmarks an `update` call that doesn't exist. The FIPS 206 "dual-scheme window" promised in E.0 rule 9 has no mechanism, and a leaked key can't be rotated.

**Latent issue:** `CheckFalconSignature::RegistryMode.account` isn't covered by the signature. That's safe only while one key per account holds.

**Fix (before E0):**
- Registry `account → (scheme_id, pk)`, with `blake3(pk)` only as the *creation* rule.
- `rotate_key(new_scheme, new_pk, proof_by_new_key)` authorized by the current key.
- A per-account scheme tag so a v2 host function can be selected.

**Status:** OPEN.

### D-11 — The signature message doesn't bind the mode, the account, or the chain — MEDIUM — DECISION (breaking wire change)
**Flaw:**
- `Bootstrap` and `RegistryMode` sign the same message (`runtime/src/extensions.rs:124,139`). A third party can re-wrap a registry-mode transaction as Bootstrap. The new transaction hash evades dedup, and the victim pays about 897 extra length bytes.
- All Falcon signing uses `DOMAIN_NONE`.
- Payment vouchers have no genesis and no parties (`primitives/storage/src/lib.rs:133-143`), so they replay across networks.

**Fix:** sign `blake3(DOMAIN ‖ genesis ‖ mode ‖ account ‖ implication)`. Use fn-dsa's `DomainContext` for off-chain objects, or at least add genesis and both parties to `Voucher`. Do this together with D-10 and W-4. **Status:** OPEN.

---

## Network & node robustness (implement)

| ID | Sev | Flaw → fix | Status |
|---|---|---|---|
| **N-1** | HIGH | **Flaw:** the Prism network worker is one `select!` loop that awaits remote round-trips inline, with a 20 s timeout (`prism-network/src/wire.rs:341-393`). One slow or Sybil peer freezes all gossip and sync serving. **Fix:** spawn fetch/backfill tasks with per-peer concurrency limits and feed results back over a channel. | OPEN |
| **N-2** | HIGH | **Flaw:** `GetBlocks` has no cap on hash count and no dedup (`wire.rs:199-205`): one ~32k-hash request can make a peer allocate about 1 GB. `reputation_changes` is always empty and every substream is accepted. **Fix:** cap at 128 hashes, dedup, apply a byte budget to the response, and add reputation penalties for invalid blocks, bad requests and timeouts. | OPEN |
| **N-3** | MED-HIGH | **Flaw:** `seen.insert` runs *before* `validate_block` (`prism-network/src/lib.rs:85-93`). A header relayed with a corrupt proof censors the genuine block for the rest of the process's life, and evicted orphans can never be re-fetched. `seen` is also unbounded. **Fix:** insert only after the block validates, remove on orphan eviction, and make it an LRU. | OPEN |
| **N-4** | HIGH (suspected) | **Flaw:** `drain_orphans ↔ insert_block` recurses, and each drain scans the whole orphan map (`prism-chain/src/lib.rs:521,632-711`). During tip-first backfill this is O(n²) with deep recursion, a possible stack overflow. FIFO eviction also evicts tips first. **Fix:** an iterative worklist plus a missing-dependency → waiters index. Confirm the overflow with a reverse-inserted 5k-block voter chain test. | OPEN |
| **N-5** | HIGH (suspected) | **Flaw:** BuildLedger costs O(history) per level and recurses O(history) deep. The proposer `parent` is never removed from `unreferenced_proposers` (`prism-chain/src/lib.rs:468-471`), so a `prop_refs` chain runs back to genesis. `expand_proposer_tx_refs` (`prism-ledger/src/lib.rs:269-288`) recurses with a fresh `visited` set and reads from the DB. Suspected stack overflow on archive nodes around level 10⁴. **Fix:** a durable already-ledgered set and an iterative walk, or treat the parent as referenced (as in the paper). Confirm with a 20k-level test. | OPEN |
| **N-6** | — | **B-18 rework.** Keep `orphan_missing_refs`, but route those refs only to peers that can serve them: advertise `pruned_below`/archive status in `Status`, or keep a per-peer "can't serve" negative cache. Don't resend about 1.6k hashes to every pruning peer every 30 s. Blocked on R-1/R-2. | OPEN |
| **N-7** | LOW | **Flaw:** `proposers_at_level`/`leader_at_level` scan all of `proposer_meta` and rebuild `vote_totals()` (`prism-chain/src/lib.rs:759-783`), and stage-2 costs O(P²·depth·m) per wakeup. **Fix:** a level → proposers index. | OPEN |
| **N-8** | LOW | **Flaw:** the header's `difficulty` is miner-declared and never compared against `params` (`primitives/prism/src/lib.rs:394`). Harmless today because the sortition ranges enforce the real threshold. **Fix:** compare it to the params, and to the epoch value once B-11 lands. | OPEN |
| **N-9** | LOW | **Flaw:** `PrismDb` lives outside the per-chain database directory. `purge-chain` doesn't delete it, and a rebuilt chain then diverges. Switching `--chain` on the same base path panics on the genesis `assert_eq!`. **Fix:** keep it under the chain directory, purge it with the chain, and replace the assert with a clean error. | OPEN |
| **N-10** | LOW | **Flaw:** miner queue rotation is phase-correlated across miners (`service.rs:366-384`), where §4.4 intends randomized queues, and every miner starts at nonce 0. **Fix:** a random start queue and a random starting nonce. | OPEN |

---

## Storage — Implementation D

| ID | Sev | Flaw → fix | Status |
|---|---|---|---|
| **S-1** | CRITICAL | **Flaw:** proofs don't show that each provider stores its own shard. RS shards are linear functions of one another, data shards are plaintext, the challenge is 1 KiB, and the window is about 210 blocks. One Sybil operator can hold a single copy and pass every challenge for all n shards, and data-shard holders can read the file. This makes guide §9.1-2 and §9.10's claim that "RS gives distinctness" false. **Fix:** a per-deal, per-shard client-side keyed transform after RS encoding (`shard_i ⊕ ChaCha20(K_deal, i)`), with roots taken over the transformed bytes and a manifest version bump. **DECISION on repair:** D5 then needs a delegated repair key or client-run repair. Longer term, client-directed provider selection. | OPEN — DECISION |
| **S-2** | CRITICAL/HIGH | **Flaw:** slashes are fixed multiples of ED (`runtime/src/lib.rs:249-252`), so deleting data pays roughly 200,000× the maximum penalty. `Misses` resets on any pass (`storage-proofs/src/lib.rs:218`), the 2× `StakeLocked` is never slashed, and nothing in `client/` calls `report_missed`, `claim_earnings` or `settle`. **Fix:** slash in proportion to the assignment (a miss forfeits that epoch's vesting plus a share of escrow; a fault slashes the assignment's `StakeLocked` share), count misses over a sliding window, and ship a client watchtower. | OPEN |
| **S-3** | HIGH | **Flaw:** a `Lost` deal keeps vesting to survivors until `end_epoch` with no challenges possible (`storage-deals/src/lib.rs:397-436,589-599,617`). **Fix:** store `lost_epoch`, cap vesting at `lost_epoch − 1`, and allow `close_deal` at `lost_epoch + CloseGrace`. | OPEN |
| **S-4** | HIGH | **Flaw:** the only exit from `Assigning` is a client cancel, so provider stake can be locked indefinitely. Slot squatting is free, including repeatedly squatting a repair slot. The guide's claimed "Assigning timeouts" don't exist. **Fix:** a permissionless `abandon_deal` after assign + activation timeouts, an accept bond forfeited on expiry, and a bar on re-accepting after expiry. Longer term, a client allowlist in `DealParams`. | OPEN |
| **S-5** | HIGH/MED | **Flaw:** a faulted provider can re-accept its own slot and win back its fault slash through `repair_pot`, which one activation proof pays out in full. Self-reporting (`reporter == provider`) is also allowed. **Fix:** record faulted providers per deal and bar them, vest the repair bounty over M proofs, and reject self-reports. | OPEN |
| **S-6** | MED/HIGH | **Flaw:** beyond the known missing signature check, the retrieval handler never checks channel existence, `to == me`, the deposit bound or expiry. Vouchers are never persisted or settled, so revenue is zero. `settle` rejects `cumulative > deposit` instead of capping it. Clients predict `channel_id` before `open` is mined. There's no settle grace after expiry. `settle` advances `claimed` on a short payment. **Fix:** a chain-backed `ChannelAuthority`, fsync'd vouchers plus a periodic settle, `settle` paying `min(cumulative, deposit) − claimed`, a two-phase `close`, and genesis in the voucher payload (D-11). | OPEN |
| **S-7** | MED | **Flaw:** `close_deal` prunes proof records with no bound on deal length (`storage-proofs/src/lib.rs:309-315`, duration is unbounded); `terminate` never prunes; terminal `Deals` records are never deleted; zero-price deals are allowed. **Fix:** a per-(deal, shard) ring bitmap of recent epochs, prune on terminate, delete terminal records or charge a storage deposit, and set a minimum price. | OPEN |
| **S-8** | MED | **Flaw:** `slash` isn't capped at `p.stake` (`provider-registry/src/lib.rs:217-229`), so it can drain reserves held for other pallets (a shared unnamed reserve). Payout cursors advance even when the payment falls short. **Fix:** cap the slash, advance cursors by the amount actually paid, and migrate to `fungible::MutateHold` with per-pallet `HoldReason` (W-5 is the same fix for falcon-registry). | OPEN |
| **S-9** | MED | **Flaw:** the daemon exits on one transient RPC error (`responder.rs:402`); each epoch gets a single fire-and-forget attempt with no retry during e+1; there's no `claim_earnings` loop. **Fix:** per-iteration error handling, a retry state machine for each (deal, shard, epoch), and a claim loop. | OPEN |
| **S-10** | LOW/MED | **Flaw:** `ShardPushHandler` reads up to 256 MiB into memory before the authority check. `bao_slice` rebuilds the outboard for every proof. The proof hot key is also the staking key. The passphrase is passed on argv. `iroh_secret.key` is written with default permissions. `iroh_node_id` isn't bound to the account. **Fix:** a header-first push, persisted outboards, an operator/stash key split, 0o600 key files, and a signed iroh-id binding. | OPEN |
| **S-11** | LOW | **Flaw:** the `storage-proofs` module doc calls the beacon "un-grindable". A leader miner knows it before publishing. **Fix:** correct the doc now, and plan an unbiasable beacon (VDF/drand) if deal value grows. The beacon must not be treated as general randomness: correct the dApp catalog's anti-catalog note. | OPEN |

---

## Runtime, signing & wallet

| ID | Sev | Flaw → fix | Status |
|---|---|---|---|
| **W-1** | HIGH | **Flaw:** the keystore KDF is a single `blake3::derive_key(salt‖passphrase)` (`wallet/signer-core/src/keystore.rs:148-152`), so a stolen keystore can be cracked at GPU speed. **Fix:** Argon2id with stored params (m ≥ 64 MiB, t ≥ 3), a versioned `-v2` file format, and v1 kept read-only. Also bind the pk as AEAD associated data and recompute the address on load (the displayed address is currently unauthenticated). | OPEN |
| **W-2** | HIGH (devnet) | **Flaw:** genesis endowments and sudo go only to sr25519 `//Alice` accounts (`node/src/chain_spec.rs:181-233`), which the default wasm (no `dev-signatures`) can't verify. So funds are unusable and sudo is dead: no `set_code`. Building with `--features dev-signatures` embeds an sr25519-accepting wasm under the same `spec_version`, and the Cargo comments claim the opposite. **Fix:** deterministic dev *Falcon* seeds for endowment and sudo; force `spec_name = "argonaut-dev"` (or a compile error) under that feature; correct the comments. | OPEN |
| **W-3** | MED | **Flaw:** auto-register in `prepare` reserves `KeyDeposit` before the fee is withdrawn (`runtime/src/extensions.rs:149-171`), so a pool-valid first transaction can fail at apply and be silently dropped. After `deregister_key`, the next Bootstrap transaction re-registers. **Fix:** include the deposit in the `validate` affordability check, or drop auto-register in favour of an explicit `register_key`. | OPEN |
| **W-4** | MED | **Flaw:** the signing-payload contract is implicit and untested. The General-mode message is reverse-engineered from SDK tuple internals, and no runtime test round-trips a real `SignedExtra` through `Executive`. `falcon-sign` and `signer-wasm::sign_payload` sign `blake3(raw)`, but payloads over 256 bytes are blake2-prehashed first, so their signatures are **invalid for `register_key` and any large call**. `signer-core` has no transaction builder. **Fix:** an Argonaut-owned payload format with a builder and golden vectors in `signer-core`, an `Executive` round-trip test, and deprecation of the Signed-preamble `FalconWithKey` path. Pairs with D-11. | OPEN |
| **W-5** | MED | **Flaw:** the registry deposit is an unnamed reserve at the *current* `KeyDeposit` (`falcon-registry/src/lib.rs:105,121`), so a price change unreserves the wrong amount. **Fix:** a hold with `HoldReason::FalconKey`, and store the deposit amount alongside the key. | OPEN |
| **W-6** | LOW | **Flaw:** `falcon_crypto::pk_hash` is a redundant host function; neither host function is `#[version]`ed; fn-dsa picks the AVX2 or portable path per machine. **Fix:** compute `pk_hash` in wasm (keep the host function registered), mark `#[version(1)]`, and add a CI differential test of AVX2 vs portable. | OPEN |
| **W-7** | (unwired) | **Flaw:** agg-verify isn't safe to wire as-is. It never dispatches, allows XMSS leaf reuse within a bundle, lets `zip` truncate unbounded vectors, has a malleable Merkle tree, no `provides` tag (suspected: the pool rejects every bundle), is fee-free and unsigned, and its "host function" is a plain `fn`. **Fix:** redesign before wiring (Implementation C). | OPEN |
| **W-8** | LOW | Passphrases are passed on argv (`keygen.rs`, `falcon_sign.rs`), and keystores are written without 0o600. | OPEN |

---

## Ops, CI & docs

| ID | Sev | Flaw → fix | Status |
|---|---|---|---|
| **O-1** | MED (cheap) | **Flaw:** CI uses a floating `stable` toolchain, and 1.97's rust-lld breaks the runtime wasm link (see the Dockerfile's pin). **Fix:** `rust-toolchain.toml` pinned to 1.94.0 with `wasm32-unknown-unknown` and `rust-src`. | OPEN |
| **O-2** | MED | CI never builds `wallet/signer-wasm` for wasm32 and doesn't cover `web/`. There's no multi-node or determinism job. | OPEN |
| **O-3** | LOW-MED | **Flaw:** live verification can't be reproduced. The compose devnet uses shift 24, and the shift-20 recipe is manual. `node3-data` is a named volume, so the compose comment's claim of a fresh backfill on each `up` is false. **Fix:** `scripts/devnet-acceptance.sh`, which generates the spec, brings up compose, asserts `confirmed via stage-1` on every node plus identical finalized hashes, and exits non-zero on failure. | OPEN |
| **O-4** | LOW | Doc drift. `ARCHITECTURE.md` §4.10/4.11/4.13 describe live retargeting, §6 says there's no compose devnet, and §3 says auto-register isn't implemented. The root README says storage is "not started", and its `submit_transfer` usage is wrong. The guide still describes `DigestItem::Other`, "~seconds", and §9.5 grinding that assumes rewards. The E spec references a nonexistent registry `update` call. The fixplan's AF-4 item overlooks the existing `bridge.rs` determinism test, which doesn't cover the block hash. | OPEN |
| **O-5** | LOW | **Flaw:** the ledger's silent fallbacks (missing tx body skipped, missing leader body giving timestamp 0) hide determinism hazards. **Fix:** halt instead. Covered by R-5. | OPEN |

---

## Keep (reviewers agreed these are sound; don't churn)

- **The seam between ordering and execution:** Prism orders opaque bytes, the runtime executes them, and the bridge is the only place they meet. The `LeaderDigest` is `PreRuntime`, and the deterministic leader-derived timestamp is the right anchor for R-1/R-2.
- **Falcon verification as a host function:** strict canonical decoding; the registry invariant `blake3(pk) == account` (which stops registration front-running); the `CheckFalconSignature` already-authorized guard; replay protection through genesis, spec, era and nonce.
- **Stage-2's withheld-proposer bound (eq. 35),** and its unique-winner structure, which doesn't depend on order.
- **Per-chain, network-separated Prism geneses,** and `validate_block` as a pure no_std check that does the cheap PoW/sortition work before the Merkle work.
- **The retarget principle:** difficulty as a pure function of the confirmed leader sequence. Only its timing is at issue.
- **Storage:** lazy O(1) transitions, the epoch beacon snapshot, single-sourced selection formulas, the daemon reading the beacon from chain, activation tied to `beacon(accepted + 1)`, and the payment-channel `settle` inline-pk fallback.
- **Signing only in Rust/wasm,** never in JS.
