//! Manual storage key computation (D.11: "reads storage via `state_getStorage`
//! with manually computed keys"). Uses the real `frame_support::StorageHasher`
//! impls for the concat step so key bytes are guaranteed correct against
//! whatever the pallet macro actually emits -- only the twox128 pallet/item
//! name prefix is hand-assembled (that part is always plain twox128 of the
//! ASCII name, unconditionally, no pallet-specific choice involved).

use frame_support::{Blake2_128Concat, StorageHasher, Twox64Concat};
use parity_scale_codec::Encode;

/// The hasher a `StorageMap`/`StorageDoubleMap` key segment uses. Mirrors the
/// two hashers actually used across the D pallets (verified against their
/// `#[pallet::storage]` declarations, not assumed from the spec text).
#[derive(Clone, Copy, Debug)]
pub enum Hasher {
    Blake2_128Concat,
    Twox64Concat,
}

impl Hasher {
    fn hash(self, encoded_key: &[u8]) -> Vec<u8> {
        match self {
            Hasher::Blake2_128Concat => Blake2_128Concat::hash(encoded_key),
            Hasher::Twox64Concat => Twox64Concat::hash(encoded_key),
        }
    }
}

/// `twox128(pallet) ++ twox128(item)` -- the 32-byte prefix shared by every
/// storage item in `pallet` named `item` (a bare `StorageValue`'s full key).
pub fn storage_prefix(pallet: &str, item: &str) -> Vec<u8> {
    let mut key = Vec::with_capacity(32);
    key.extend_from_slice(&sp_core_hashing::twox_128(pallet.as_bytes()));
    key.extend_from_slice(&sp_core_hashing::twox_128(item.as_bytes()));
    key
}

/// Full key for a `StorageMap<_, H, K, V>` entry.
pub fn map_key<K: Encode>(pallet: &str, item: &str, hasher: Hasher, key: &K) -> Vec<u8> {
    let mut full = storage_prefix(pallet, item);
    full.extend_from_slice(&hasher.hash(&key.encode()));
    full
}

/// Full key for a `StorageDoubleMap<_, H1, K1, H2, K2, V>` entry.
pub fn double_map_key<K1: Encode, K2: Encode>(
    pallet: &str,
    item: &str,
    hasher1: Hasher,
    key1: &K1,
    hasher2: Hasher,
    key2: &K2,
) -> Vec<u8> {
    let mut full = storage_prefix(pallet, item);
    full.extend_from_slice(&hasher1.hash(&key1.encode()));
    full.extend_from_slice(&hasher2.hash(&key2.encode()));
    full
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beacons_prefix_matches_hand_computed_value() {
        // Cross-checked live against a running devnet in the D2 verification
        // pass (docs/IMPLEMENTATION_D_PROGRESS.md, 2026-07-16): querying
        // `state_getKeysPaged` with this exact prefix returned the expected
        // per-epoch Beacon entries.
        let prefix = storage_prefix("StorageProofs", "Beacons");
        assert_eq!(
            hex::encode(&prefix),
            "6c8eca7ffda4bfb1a1a4f526155bd57c72b9066e56ccad8f479017bc59fbe973"
        );
    }

    #[test]
    fn map_key_appends_hashed_encoded_key_after_the_prefix() {
        let prefix = storage_prefix("StorageProofs", "Beacons");
        let full = map_key(
            "StorageProofs",
            "Beacons",
            Hasher::Twox64Concat,
            &1u32,
        );
        assert!(full.starts_with(&prefix));
        assert!(full.len() > prefix.len());
    }
}
