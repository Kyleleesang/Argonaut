//! Pure default constants for `pallet-storage-proofs` (D.1 rule 9 / D.6).
//! Balance defaults are ED multiples the runtime scales (spec D.8).

use argonaut_primitives_storage::EpochIndex;

/// Epoch length in blocks (dev default; lower it on a compressed devnet).
pub const EPOCH_DURATION_BLOCKS: u32 = 100;
/// How far back from the epoch boundary the beacon block hash is snapshotted.
/// Must stay `< BlockHashCount (2400)` so the hash is still retained.
pub const BEACON_DELAY_BLOCKS: u32 = 10;
/// Epochs of beacons retained. Must be `≥ ReportWindowEpochs + 2` (D.6.4).
pub const BEACON_RETENTION_EPOCHS: EpochIndex = 8;
/// How many epochs back a miss may still be reported.
pub const REPORT_WINDOW_EPOCHS: EpochIndex = 4;
/// Consecutive misses before a shard is escalated to `Faulted`.
pub const MAX_CONSECUTIVE_MISSES: u8 = 3;

/// Per-miss slash, in ED multiples (split 50/50 reporter/client).
pub const MISSED_PROOF_SLASH_ED_MULTIPLE: u128 = 50;
/// Fault-escalation slash, in ED multiples (funds the deal's repair_pot).
pub const FAULT_SLASH_ED_MULTIPLE: u128 = 500;
