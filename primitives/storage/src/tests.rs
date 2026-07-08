//! D1 unit tests (spec §D.12): deterministic selection formulas + bao round-trip.

use super::*;

// ── Selection formulas ──────────────────────────────────────────────────────

#[test]
fn selection_is_deterministic() {
    let beacon = [7u8; 32];
    assert_eq!(is_selected(&beacon, 42, 3, 500_000), is_selected(&beacon, 42, 3, 500_000));
    assert_eq!(challenged_chunk(&beacon, 42, 3, 8192), challenged_chunk(&beacon, 42, 3, 8192));
    assert_eq!(activation_chunk(&beacon, 42, 3, 8192), activation_chunk(&beacon, 42, 3, 8192));
}

#[test]
fn selection_depends_on_beacon() {
    // Different epoch beacon => generally different selection outcome somewhere.
    let a = [1u8; 32];
    let b = [2u8; 32];
    let differ = (0u16..200).any(|idx| is_selected(&a, 1, idx, 500_000) != is_selected(&b, 1, idx, 500_000));
    assert!(differ, "beacon must influence selection");
}

#[test]
fn rate_zero_never_selects_rate_max_always_selects() {
    let beacon = [9u8; 32];
    for idx in 0u16..1000 {
        assert!(!is_selected(&beacon, 5, idx, 0));
        assert!(is_selected(&beacon, 5, idx, 1_000_000));
    }
}

#[test]
fn selection_distribution_is_close_to_rate() {
    // ~rate_ppm/1e6 of trials selected (spec D.12: distribution sanity).
    let beacon = [3u8; 32];
    let rate_ppm = 250_000u32; // 25%
    let trials = 10_000u16;
    let hits = (0..trials).filter(|&idx| is_selected(&beacon, 77, idx, rate_ppm)).count();
    let frac = hits as f64 / trials as f64;
    assert!((frac - 0.25).abs() < 0.03, "selection fraction {frac} not near 0.25");
}

#[test]
fn challenged_chunk_in_range() {
    let beacon = [4u8; 32];
    let size = 64 * 1024; // 64 chunks
    for idx in 0u16..500 {
        assert!(challenged_chunk(&beacon, 11, idx, size) < num_chunks(size));
        assert!(activation_chunk(&beacon, 11, idx, size) < num_chunks(size));
    }
}

#[test]
fn num_chunks_basic() {
    assert_eq!(num_chunks(1024), 1);
    assert_eq!(num_chunks(64 * 1024), 64);
}

// ── Voucher ─────────────────────────────────────────────────────────────────

#[test]
fn voucher_payload_deterministic_and_binds_fields() {
    let v = Voucher { channel_id: 7, cumulative: 1_000 };
    assert_eq!(voucher_signing_payload(&v), voucher_signing_payload(&v));
    let v2 = Voucher { channel_id: 7, cumulative: 1_001 };
    assert_ne!(voucher_signing_payload(&v), voucher_signing_payload(&v2));
    let v3 = Voucher { channel_id: 8, cumulative: 1_000 };
    assert_ne!(voucher_signing_payload(&v), voucher_signing_payload(&v3));
}

// ── Bao slice round-trip (native; requires the `std`/`bao-tree` build) ───────

/// Encode a single-chunk bao slice for `chunk` of `data`; returns (root, slice).
fn encode_chunk(data: &[u8], chunk: u64) -> ([u8; 32], alloc::vec::Vec<u8>) {
    use bao_tree::{
        io::outboard::PreOrderMemOutboard, io::sync::encode_ranges_validated, BlockSize, ChunkNum,
        ChunkRanges,
    };
    let ob = PreOrderMemOutboard::create(data, BlockSize::ZERO);
    let root = *ob.root().as_bytes();
    let ranges = ChunkRanges::from(ChunkNum(chunk)..ChunkNum(chunk + 1));
    let mut slice = alloc::vec::Vec::new();
    encode_ranges_validated(data, &ob, ranges.as_ref(), &mut slice).unwrap();
    (root, slice)
}

fn sample_blob(len: usize) -> alloc::vec::Vec<u8> {
    // Deterministic pseudo-random bytes (no OS randomness dependency).
    let mut out = alloc::vec::Vec::with_capacity(len);
    let mut x = 0x9e3779b97f4a7c15u64;
    for _ in 0..len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.push((x & 0xff) as u8);
    }
    out
}

#[test]
fn bao_slice_verifies_and_rejects_tampering() {
    let size = 64 * 1024u64; // 64 chunks
    let data = sample_blob(size as usize);
    let chunk = 40u64;
    let (root, slice) = encode_chunk(&data, chunk);

    // Honest proof verifies.
    assert!(bao_proof::verify_slice(&root, size, chunk, 1, &slice));

    // Flip one byte of the slice => reject.
    let mut bad_slice = slice.clone();
    let mid = bad_slice.len() / 2;
    bad_slice[mid] ^= 0x01;
    assert!(!bao_proof::verify_slice(&root, size, chunk, 1, &bad_slice));

    // Corrupt the root => reject.
    let mut bad_root = root;
    bad_root[0] ^= 0x01;
    assert!(!bao_proof::verify_slice(&bad_root, size, chunk, 1, &slice));

    // Wrong total_size => reject.
    assert!(!bao_proof::verify_slice(&root, size + 1024, chunk, 1, &slice));

    // A slice for chunk i must not verify as chunk j.
    assert!(!bao_proof::verify_slice(&root, size, chunk + 1, 1, &slice));
}

#[test]
fn bao_slice_every_chunk_index_round_trips() {
    let size = 16 * 1024u64;
    let data = sample_blob(size as usize);
    for chunk in 0..num_chunks(size) {
        let (root, slice) = encode_chunk(&data, chunk);
        assert!(slice.len() <= MAX_PROOF_BYTES as usize, "slice fits proof bound");
        assert!(bao_proof::verify_slice(&root, size, chunk, 1, &slice));
    }
}
