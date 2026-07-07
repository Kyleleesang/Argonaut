//! Column-family names.
//!
//! B5 pruning design decision (2026-07-07): this crate originally declared
//! 13 additional CF names (`PROPOSER_NODE_LEVEL`, `GRAPH_VOTE_NEIGHBOR`, ...)
//! kept verbatim from prism-rust "for cross-repo compatibility." They were
//! never populated or read anywhere in this port, and no encoding was ever
//! designed for them -- the compatibility they promised was illusory (no
//! tool reads these CFs, and this port's actual data model, e.g.
//! `VoteInfo::per_chain`'s depth map, has no prism-rust equivalent to be
//! compatible with). Removed rather than half-implemented against an
//! undefined schema; `PrismChain`'s `ChainCheckpoint` (see `prism-chain`)
//! is the real prerequisite pruning needed, and is a from-scratch design,
//! not a population of these names. Existing devnet DBs predate this and
//! must be purged and remined (same migration answer as B-5's genesis
//! change) rather than migrated.

pub const PROPOSER_LEADER_SEQUENCE: &str = "PROPOSER_LEADER_SEQUENCE";
pub const PROPOSER_LEDGER_ORDER: &str = "PROPOSER_LEDGER_ORDER";
pub const BLOCKS: &str = "BLOCKS";
pub const BLOCK_STATUS: &str = "BLOCK_STATUS";
/// Current combined difficulty target (B-2: client-side rolling-window
/// retarget), so it survives a node restart instead of resetting to
/// `Params::testnet()`'s initial guess every time.
pub const CURRENT_DIFFICULTY_TARGET: &str = "CURRENT_DIFFICULTY_TARGET";
/// Every tx hash `PrismLedger` has ever emitted in a `BuildLedger` delta
/// (key-only marker CF, empty value) -- the durable half of
/// `PrismLedger::emitted_tx_hashes`. B5 pruning prerequisite: once tx-block
/// *bodies* become prunable, `PrismLedger::recover`'s old approach (replay
/// every confirmed level's `BuildLedger` DFS purely to repopulate this set)
/// stops working, since the bodies it would read may already be gone. This
/// CF makes dedup state itself durable instead of derived, independent of
/// whether bodies still exist.
pub const EMITTED_TX_HASH: &str = "EMITTED_TX_HASH";
/// Single-key CF holding `PrismChain`'s serialized `ChainCheckpoint` (B5
/// pruning prerequisite) -- lets `PrismChain::with_db` restart in
/// O(live window) instead of O(total historical blocks), and is what makes
/// restart possible at all once old blocks are pruned out of `BLOCKS`.
pub const CHAIN_CHECKPOINT: &str = "CHAIN_CHECKPOINT";

pub const ALL_COLUMNS: &[&str] = &[
    PROPOSER_LEADER_SEQUENCE,
    PROPOSER_LEDGER_ORDER,
    BLOCKS,
    BLOCK_STATUS,
    CURRENT_DIFFICULTY_TARGET,
    EMITTED_TX_HASH,
    CHAIN_CHECKPOINT,
];
