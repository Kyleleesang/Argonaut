# Prism Difficulty Retarget — Design Spec (B-11 real fix)

**Status: PROPOSED (2026-07-16, user-approved as spec-only). NOT implemented.**
`Params::retarget_enabled` stays `false` and the interim fixed-difficulty fix
stays in force until a genuinely multi-machine testnet exists to soak this
design (≥ hours, adversarial timing). Nothing currently deployed needs
retargeting: B-16 made fixed difficulty a chain-spec parameter
(`prismDifficultyShift`), which covers every single-box and LAN devnet.

Read `docs/IMPLEMENTATION_AB_REVIEW.md` B-11 and the fixplan's B-11 entry for
the root cause this fixes: the old retarget mutated `params.difficulty` in
**local block-acceptance order**, which is not consensus, so two honest nodes
could disagree on the current difficulty around a retarget boundary and
permanently split on `SortitionMismatch`.

## Design principle

Difficulty must be a **pure function of canonical state**. The one piece of
state that is byte-identical on every node by construction is the confirmed
leader sequence (`columns::PROPOSER_LEADER_SEQUENCE`, written by
`PrismLedger::advance`, kept forever even under pruning). Everything below
derives from it deterministically.

## Core scheme

1. **One scalar.** Retarget the single combined sortition target only, never
   per-chain. Leader cadence is the observable that matters for confirmation
   latency; per-voter-chain retargeting would be m× the complexity and attack
   surface for no benefit.

2. **Epochs keyed to confirmed leader levels.**
   `epoch(L) = floor(L / E)` for a chain-spec constant `E` (default 20,
   matching the existing `RETARGET_WINDOW`; add `prismRetargetEpochLevels`
   when implementing).

3. **One-epoch activation lag.** The difficulty of epoch `e+2` is a pure
   function of the leader timestamps of epoch `e`'s confirmed levels
   (`[e·E, (e+1)·E)`), computable the moment level `(e+1)·E − 1` confirms.
   Because confirmation is irreversible and identical across nodes, every
   honest node computes the identical value; the lag only guarantees they've
   all *done so* long before any block of epoch `e+2` needs validating.
   Epochs 0 and 1 use the chain-spec seed difficulty (`prismDifficultyShift`).

4. **Timestamp discipline (B-14, folded in).** Before entering the retarget
   function, clamp each leader timestamp to
   `max(ts, median(previous 11 leader timestamps))` — deterministic, no local
   clock (local-clock rules can't be consensus). Keep the existing ±4x
   per-window clamp on the adjustment itself.

5. **Rate floor (empirical, from the 2026-07-14 shift-14 run).** Never let
   retarget push the expected block interval below a configured multiple of
   assumed gossip latency. A live run at `prismDifficultyShift: 14` produced
   thousands of accepted blocks and **zero confirmed levels** — block arrival
   outpaced voter-chain stabilization entirely. A retarget that chases a
   too-fast target interval would drive the chain into that regime and stall
   confirmation, which then stalls the epoch clock itself (see failure mode
   below). Encode the floor as a hard clamp on the target, not advice.

## The hard parts (why this is a spec and not a patch)

These four are load-bearing; skipping any of them re-opens a consensus hole.

- **Fork choice must become cumulative-work, not longest-chain.** The moment
  difficulty varies across a chain's history, "longest" and "most-work"
  diverge, and an attacker who forks from a cheap old epoch can out-*length*
  honest chains at a fraction of the work. Voter fork choice (currently depth
  comparison via `voter_tips`) and any depth-based reorg logic must compare
  accumulated work. This is the single largest implementation cost.

- **Epoch binding must come from block position, not miner declaration
  alone.** Each block carries a new header field: the leader level `L` the
  miner's node had confirmed when the template was built. Validation checks
  the block's sortition against `difficulty(epoch(L))` — deterministic for
  any receiver that has confirmed through epoch(L)'s source data. To prevent
  mining "in the past" at stale-cheap difficulty, `L` must be **monotone
  along parent edges** (`block.L ≥ parent.L`). Combined with cumulative-work
  fork choice, declaring a stale `L` buys nothing: the block passes sortition
  only at its declared epoch's difficulty and contributes only that much
  work.

- **Blocks whose epoch isn't locally computable yet must buffer, not
  reject.** A receiver whose confirmed tip lags the sender's cannot compute
  `difficulty(epoch(L))` for a bleeding-edge `L`. Rejection would split
  slow-but-honest nodes; buffer these like orphans instead. Keep them
  distinguishable from unknown-parent orphans so B-13's oldest-first eviction
  can't starve them (or size/track them separately). Backfilling nodes (B-6)
  replay history in roughly causal order, so their confirmed tip advances as
  they go and old blocks validate against their own (old) epochs — validation
  must therefore look difficulty up **by the block's epoch**, never from a
  mutable `self.params.difficulty`. That field becomes epoch-0 seed only.

- **The epoch table must be persisted and replay-derivable.** New CF mapping
  `epoch → difficulty`, written when each epoch closes, re-derivable from the
  leader sequence on B-7 restart replay. Pruning-safe by construction — the
  leader sequence is already retained forever.

## Known failure mode to accept explicitly

The epoch clock advances only when levels confirm. If confirmation stalls
entirely (the B-15 scenario), difficulty freezes at the last computed epoch —
which is the safe direction (it can't runaway-adjust on garbage), but it means
retarget cannot itself rescue a confirmation stall. That rescue is stage-2's
job (B-15/B-3, now default-on as the eventual backstop). The two mechanisms
compose: stage-2 guarantees the leader sequence keeps growing; the leader
sequence feeds the epoch clock.

## Depth semantics caveat for the confirmation math

Stage-1/stage-2 vote-depth counting implicitly assumes uniform per-block work
(the paper fixes difficulty globally). With epochs, cross-epoch depth counts
mix block weights. The ±4x clamp bounds the distortion per epoch; either
accept this as an approximation (document it in `confirmation.rs`) or
normalize depths by per-epoch work when the margin is evaluated. Decide at
implementation time; default to "accept + document" unless the soak test
shows otherwise.

## Rollout

1. Implement behind the existing `retarget_enabled` flag, default `false`.
2. Unit coverage: epoch derivation from a scripted leader sequence
   (including MTP clamping and the ±4x/floor clamps), cross-node determinism
   (two chains fed identical sequences produce identical epoch tables),
   buffering of epoch-ahead blocks, monotone-`L` rejection.
3. Soak on a ≥3-machine testnet for hours with deliberately varied hash rate
   (add/remove a miner), grepping for `SortitionMismatch` (must be zero) and
   verifying byte-identical `best:` hashes throughout — the same acceptance
   bar as B-11's original verify step.
4. Only then discuss flipping any network's chain-spec to enable it.
