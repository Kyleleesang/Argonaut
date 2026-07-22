//! Argonaut storage client SDK (Implementation D §D.3.4, §D.10).
//!
//! Pure Reed-Solomon erasure coding + manifest construction — **no iroh here**
//! (that lands with the daemon at D3). Encode is the client-side half of the
//! "one hash tree everywhere" rule: every shard's [`BlobRoot`] is its BLAKE3
//! root, identical to what an iroh-blobs node would advertise, so the daemon,
//! the pallets, and this SDK all agree byte-for-byte.
//!
//! RS layout (D.3.4): `shard_size = ceil(file_size / k)` rounded up to the next
//! multiple of 1024; the file is zero-padded to `k * shard_size`; shards `0..k`
//! are the data shards in order, `k..n` the parity shards. Decode truncates the
//! reassembled bytes to `file_size` and verifies `blake3 == file_hash`.

use argonaut_primitives_storage::{BlobRoot, Manifest, CHUNK_SIZE, MANIFEST_VERSION};
use parity_scale_codec::Encode;

pub mod deal;
pub mod fetch;
pub mod net;

/// BLAKE3 root of a byte slice == bao-tree root == iroh-blobs content id.
pub fn blob_root(bytes: &[u8]) -> BlobRoot {
    *blake3::hash(bytes).as_bytes()
}

/// Round `x` up to the next multiple of `m` (`m > 0`). Uses the stable inherent
/// `u64::div_ceil`.
fn round_up(x: u64, m: u64) -> u64 {
    x.div_ceil(m).saturating_mul(m)
}

/// Result of [`encode`]: the manifest, its SCALE bytes + root (== the on-chain
/// `DealParams.manifest`), and the `n` shard blobs.
pub struct Encoded {
    pub manifest: Manifest,
    pub manifest_bytes: Vec<u8>,
    pub manifest_root: BlobRoot,
    pub shards: Vec<Vec<u8>>,
}

/// Erasure-encode `file` into a `k`-of-`n` shard set + manifest (D.3.4).
pub fn encode(file: &[u8], k: u8, n: u8) -> anyhow::Result<Encoded> {
    anyhow::ensure!(
        k >= 1 && (k as u16) < n as u16 && n <= 64,
        "require 1 <= k < n <= 64 (got k={k}, n={n})"
    );
    let file_size = file.len() as u64;
    let file_hash = blob_root(file);
    let k64 = k as u64;

    // shard_size = ceil(file_size / k) rounded up to a multiple of 1024 (>= 1024).
    let shard_size = round_up(file_size.div_ceil(k64).max(1), CHUNK_SIZE as u64);
    let ss = shard_size as usize;

    // Zero-pad to exactly k * shard_size, then split into the k data shards.
    let mut padded = file.to_vec();
    padded.resize(ss * k as usize, 0u8);
    let mut shards: Vec<Vec<u8>> = (0..k as usize)
        .map(|i| padded[i * ss..(i + 1) * ss].to_vec())
        .collect();

    // Parity shards k..n.
    let recovery = reed_solomon_simd::encode(k as usize, (n - k) as usize, &shards)
        .map_err(|e| anyhow::anyhow!("reed-solomon encode failed: {e}"))?;
    shards.extend(recovery);
    debug_assert_eq!(shards.len(), n as usize);

    let roots: Vec<BlobRoot> = shards.iter().map(|s| blob_root(s)).collect();
    let manifest = Manifest {
        version: MANIFEST_VERSION,
        file_size,
        file_hash,
        k,
        n,
        shard_size,
        shards: roots,
    };
    let manifest_bytes = manifest.encode();
    let manifest_root = blob_root(&manifest_bytes);
    Ok(Encoded { manifest, manifest_bytes, manifest_root, shards })
}

/// Reconstruct the original file from `>= k` of the `n` shards (D.3.4 decode).
///
/// `available` has exactly `n` entries; `Some(bytes)` for a present shard (each
/// `shard_size` bytes), `None` for a missing one. Verifies the reassembled file
/// against `manifest.file_hash`.
pub fn decode(manifest: &Manifest, available: &[Option<Vec<u8>>]) -> anyhow::Result<Vec<u8>> {
    let k = manifest.k as usize;
    let n = manifest.n as usize;
    anyhow::ensure!(available.len() == n, "available must have n={n} entries");
    let ss = manifest.shard_size as usize;

    let mut orig: Vec<(usize, &[u8])> = Vec::new();
    let mut rec: Vec<(usize, &[u8])> = Vec::new();
    for (i, slot) in available.iter().enumerate() {
        if let Some(bytes) = slot {
            anyhow::ensure!(bytes.len() == ss, "shard {i} has wrong size");
            if i < k {
                orig.push((i, bytes.as_slice()));
            } else {
                rec.push((i - k, bytes.as_slice()));
            }
        }
    }
    anyhow::ensure!(orig.len() + rec.len() >= k, "need >= k={k} shards to reconstruct");

    // Fill known data shards; RS-restore the rest.
    let mut data: Vec<Option<Vec<u8>>> = core::iter::repeat_with(|| None).take(k).collect();
    for (i, b) in &orig {
        data[*i] = Some(b.to_vec());
    }
    if orig.len() < k {
        let restored = reed_solomon_simd::decode(k, n - k, orig.iter().copied(), rec.iter().copied())
            .map_err(|e| anyhow::anyhow!("reed-solomon decode failed: {e}"))?;
        for (idx, bytes) in restored {
            data[idx] = Some(bytes);
        }
    }

    let mut out = Vec::with_capacity(k * ss);
    for (i, slot) in data.into_iter().enumerate() {
        let shard = slot.ok_or_else(|| anyhow::anyhow!("data shard {i} unrecoverable"))?;
        out.extend_from_slice(&shard);
    }
    out.truncate(manifest.file_size as usize);
    anyhow::ensure!(blob_root(&out) == manifest.file_hash, "file hash mismatch after decode");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-random bytes (xorshift; no OS randomness dependency).
    fn sample(len: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(len);
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
    fn rs_encode_decode_drop_any_6() {
        // Spec D.12: encode 10-of-16, drop any 6, reconstruct byte-identical.
        let file = sample(100_000);
        let enc = encode(&file, 10, 16).unwrap();
        assert_eq!(enc.shards.len(), 16);
        assert_eq!(enc.manifest.shard_size % CHUNK_SIZE as u64, 0);

        // Drop 6 shards (mix of data + parity), keep exactly k = 10.
        let mut avail: Vec<Option<Vec<u8>>> = enc.shards.iter().map(|s| Some(s.clone())).collect();
        for i in [0usize, 5, 7, 11, 13, 15] {
            avail[i] = None;
        }
        let out = decode(&enc.manifest, &avail).unwrap();
        assert_eq!(out, file, "reconstruction must be byte-identical");
        assert_eq!(blob_root(&out), enc.manifest.file_hash);
    }

    #[test]
    fn rs_decode_with_all_data_shards_present() {
        // No RS work needed when all k data shards survive.
        let file = sample(20_000);
        let enc = encode(&file, 4, 6).unwrap();
        let mut avail: Vec<Option<Vec<u8>>> = (0..6).map(|_| None).collect();
        for i in 0..4 {
            avail[i] = Some(enc.shards[i].clone());
        }
        assert_eq!(decode(&enc.manifest, &avail).unwrap(), file);
    }

    #[test]
    fn manifest_roots_match_shard_blobs() {
        let file = sample(5_000);
        let enc = encode(&file, 4, 6).unwrap();
        for (i, s) in enc.shards.iter().enumerate() {
            assert_eq!(blob_root(s), enc.manifest.shards[i], "shard {i} root mismatch");
        }
        assert_eq!(enc.manifest.shards.len(), 6);
    }

    #[test]
    fn too_few_shards_fails() {
        let file = sample(10_000);
        let enc = encode(&file, 10, 16).unwrap();
        // keep only 9 (< k)
        let mut avail: Vec<Option<Vec<u8>>> = (0..16).map(|_| None).collect();
        for i in 0..9 {
            avail[i] = Some(enc.shards[i].clone());
        }
        assert!(decode(&enc.manifest, &avail).is_err());
    }

    #[test]
    fn rejects_bad_params() {
        assert!(encode(&[0u8; 10], 0, 4).is_err());
        assert!(encode(&[0u8; 10], 4, 4).is_err());
        assert!(encode(&[0u8; 10], 5, 4).is_err());
    }
}
