//! Pure default constants for `pallet-storage-deals` (D.1 rule 9 / D.5).
//! Epoch-denominated; no balance scaling needed. The runtime may override.

use argonaut_primitives_storage::EpochIndex;

/// Epochs after creation within which shards may still be assigned (D.5.3).
pub const ASSIGN_TIMEOUT_EPOCHS: EpochIndex = 4;
/// Epochs after the accept-beacon within which an accepted shard must activate.
pub const ACTIVATION_TIMEOUT_EPOCHS: EpochIndex = 2;
/// Grace epochs past `end_epoch` before a deal may be permissionlessly closed.
pub const CLOSE_GRACE_EPOCHS: EpochIndex = 8;
/// Early-termination fee, in epochs of price, per still-Activated shard.
pub const CANCEL_FEE_EPOCHS: EpochIndex = 2;
