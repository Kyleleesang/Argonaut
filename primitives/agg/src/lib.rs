//! XMSS/STARK aggregation bundle types and shared statement-hash construction.
//!
//! `statement_hash` is the single source of truth used by BOTH the aggregator
//! (prover side) and the runtime host function (verifier side). Any divergence
//! between the two = proof never verifies.
//!
//! The `AggVerify` host function is a stub until leanMultisig is pinned at a
//! stable revision. Wrap behind `trait AggregationBackend` so it's swappable.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use alloc::vec::Vec;
use parity_scale_codec::{Decode, Encode};
use scale_info::TypeInfo;

/// 32-byte XMSS key root reference — either a registry lookup key or an inline root.
pub type XmssRootRef = [u8; 32];

/// Versioned scheme identifier: (vm_id, hash_id, param_set).
/// Bump to enable hash agility or leanVM upgrades without a hard fork;
/// the on-chain allow-list in pallet-agg-verify controls which are active.
#[derive(Encode, Decode, TypeInfo, Clone, PartialEq, Eq, Debug)]
pub struct AggSchemeId {
    pub vm_id: u8,
    pub hash_id: u8,
    pub param_set: u8,
}

/// An aggregated signature bundle submitted as a single extrinsic.
/// One STARK proof attests that all inner txs were signed by their respective XMSS keys.
#[derive(Encode, Decode, TypeInfo, Clone, Debug)]
pub struct AggBundle {
    pub scheme: AggSchemeId,
    /// Inner extrinsics in canonical (= proof circuit) order.
    pub txs: Vec<Vec<u8>>,
    /// XMSS key roots for each signer (registry refs or inline roots).
    pub signer_keys: Vec<XmssRootRef>,
    /// XMSS leaf epoch used by each signer. On-chain bitfield rejects reuse.
    pub epochs: Vec<u32>,
    /// Opaque leanVM proof blob. Verified via the `AggVerify` host function.
    pub proof: Vec<u8>,
}

/// Compute the statement hash that the STARK proof must attest.
///
/// **SHARED** between the aggregator (prover) and the runtime (verifier).
/// `statement_hash = blake3(SCALE((scheme, txs_root, keys_root, epochs_root)))`.
///
/// Both sides MUST call this function — never reimplement it inline.
pub fn statement_hash(
    scheme: &AggSchemeId,
    txs: &[Vec<u8>],
    signer_keys: &[XmssRootRef],
    epochs: &[u32],
) -> [u8; 32] {
    let txs_root = merkle_root_of_vecs(txs);
    let keys_root = merkle_root_of_fixed(signer_keys);
    let epochs_bytes: Vec<u8> = epochs.iter().flat_map(|e| e.to_le_bytes()).collect();
    let epochs_root = *blake3::hash(&epochs_bytes).as_bytes();

    let mut buf = scheme.encode();
    buf.extend_from_slice(&txs_root);
    buf.extend_from_slice(&keys_root);
    buf.extend_from_slice(&epochs_root);
    *blake3::hash(&buf).as_bytes()
}

fn merkle_root_of_vecs(items: &[Vec<u8>]) -> [u8; 32] {
    if items.is_empty() {
        return [0u8; 32];
    }
    let leaves: Vec<[u8; 32]> = items.iter().map(|v| *blake3::hash(v).as_bytes()).collect();
    merkle_root_fixed(&leaves)
}

fn merkle_root_of_fixed(items: &[[u8; 32]]) -> [u8; 32] {
    merkle_root_fixed(items)
}

fn merkle_root_fixed(leaves: &[[u8; 32]]) -> [u8; 32] {
    if leaves.is_empty() {
        return [0u8; 32];
    }
    let mut level: Vec<[u8; 32]> = leaves.to_vec();
    while level.len() > 1 {
        let mut next = Vec::with_capacity((level.len() + 1) / 2);
        let mut i = 0;
        while i < level.len() {
            let right = if i + 1 < level.len() { level[i + 1] } else { level[i] };
            let mut buf = [0u8; 64];
            buf[..32].copy_from_slice(&level[i]);
            buf[32..].copy_from_slice(&right);
            next.push(*blake3::hash(&buf).as_bytes());
            i += 2;
        }
        level = next;
    }
    level[0]
}

/// Host-function interface for STARK bundle verification (native-only, deterministic).
///
/// Verifier must be pinned at the same revision across all nodes — treat a verifier
/// bump as a runtime upgrade (gate via AggSchemeId allow-list in pallet-agg-verify).
pub mod agg_verify {
    /// Verify a leanVM proof: attests XMSS_verify for all (pk_root_i, epoch_i, tx_i).
    ///
    /// Currently a stub pending leanMultisig being pinned at a stable revision.
    /// Wrap actual prover/verifier behind `trait AggregationBackend` in client/aggregator.
    pub fn verify_bundle(_scheme_id: u8, _statement_hash: &[u8; 32], _proof: &[u8]) -> bool {
        false // stub — wire leanMultisig verifier here at C1 milestone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statement_hash_is_deterministic() {
        let scheme = AggSchemeId { vm_id: 1, hash_id: 0, param_set: 0 };
        let txs = vec![vec![1u8, 2, 3], vec![4u8, 5, 6]];
        let keys = vec![[0u8; 32], [1u8; 32]];
        let epochs = vec![0u32, 1u32];
        let h1 = statement_hash(&scheme, &txs, &keys, &epochs);
        let h2 = statement_hash(&scheme, &txs, &keys, &epochs);
        assert_eq!(h1, h2);
    }

    #[test]
    fn statement_hash_sensitive_to_epoch() {
        let scheme = AggSchemeId { vm_id: 1, hash_id: 0, param_set: 0 };
        let txs = vec![vec![1u8]];
        let keys = vec![[0u8; 32]];
        let h1 = statement_hash(&scheme, &txs, &keys, &[0]);
        let h2 = statement_hash(&scheme, &txs, &keys, &[1]);
        assert_ne!(h1, h2);
    }
}
