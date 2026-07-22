//! D.9 `blobs.rs`: the only module that touches blob storage directly.
//!
//! D3 vs D4 split (documented, not arbitrary): D3's acceptance (D.12) is the
//! challenge-response loop -- `submit_proof`, `activate_shard`, and the
//! `report_missed` -> slash -> `Degraded` path -- which needs only the shard
//! bytes + `bao-tree` (no networking). D4 adds retrieval: `read_range` below
//! serves the raw bytes a client requests over the gated retrieval protocol
//! (`protocol.rs`); `add_path`/`bao_slice` are unchanged from D3.
//!
//! `fetch` (pulling a blob from *another* peer to populate this store) is
//! still not implemented -- this daemon's shards always arrive via the
//! shard-push protocol (client -> provider, right after `accept_shard`), so
//! nothing here needs to pull from a peer. A future repair/keeper daemon
//! (D5) would need real peer-to-peer pull and should add it then.

use argonaut_primitives_storage::BlobRoot;
use async_trait::async_trait;
use bao_tree::io::outboard::PreOrderMemOutboard;
use bao_tree::io::sync::encode_ranges_validated;
use bao_tree::{BlockSize, ChunkNum, ChunkRanges};
use std::path::{Path, PathBuf};

#[async_trait]
pub trait BlobStore: Send + Sync {
    /// Ingest a file, returning its BLAKE3 root (== on-chain `ShardSpec.root`,
    /// C-D1.2) and size.
    async fn add_path(&self, path: &Path) -> anyhow::Result<(BlobRoot, u64)>;
    /// Ingest raw bytes directly -- the D4 shard-push protocol's path
    /// (`protocol.rs`): a client sends shard bytes over the wire, no local
    /// file involved. Returns the computed root.
    async fn add_bytes(&self, bytes: &[u8]) -> anyhow::Result<BlobRoot>;
    async fn has(&self, root: &BlobRoot) -> anyhow::Result<bool>;
    /// A single-chunk bao slice for `chunk` of the blob at `root` -- exactly
    /// the framing `bao_proof::verify_slice` (the on-chain host fn) expects,
    /// verbatim, nothing prepended (D.3.3).
    async fn bao_slice(&self, root: &BlobRoot, chunk: u64) -> anyhow::Result<Vec<u8>>;
    /// Raw bytes `[offset, offset+length)` of the blob at `root` -- what the
    /// D4 retrieval protocol serves per stride. Not bao-framed; the client
    /// verifies the *whole* reassembled shard against `root` once it has
    /// every byte (matches D4's "byte-identity" acceptance criterion; see
    /// `protocol.rs`'s module doc for why per-stride bao verification was
    /// cut from this pass).
    async fn read_range(&self, root: &BlobRoot, offset: u64, length: u32) -> anyhow::Result<Vec<u8>>;
    async fn delete(&self, root: &BlobRoot) -> anyhow::Result<()>;
}

/// Local-disk `BlobStore`: one file per blob, named by its hex root, under a
/// configured directory. Deliberately simple -- correctness (byte-exact bao
/// slices matching the on-chain root) matters far more than storage
/// efficiency for D3's acceptance bar.
pub struct DiskBlobStore {
    dir: PathBuf,
}

impl DiskBlobStore {
    pub fn new(dir: PathBuf) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    fn path_for(&self, root: &BlobRoot) -> PathBuf {
        self.dir.join(hex::encode(root))
    }

    /// Store raw shard bytes directly (the common D3 path: a provider
    /// receives shard bytes out-of-band at `accept_shard` time, not by
    /// fetching a file off local disk). Returns the computed root, so the
    /// caller can assert it matches the on-chain `ShardSpec.root` before
    /// trusting the write (C-D1.2's daemon-side check).
    pub async fn store_bytes(&self, bytes: &[u8]) -> anyhow::Result<BlobRoot> {
        let root = *blake3::hash(bytes).as_bytes();
        let path = self.path_for(&root);
        tokio::fs::write(&path, bytes).await?;
        Ok(root)
    }
}

#[async_trait]
impl BlobStore for DiskBlobStore {
    async fn add_path(&self, path: &Path) -> anyhow::Result<(BlobRoot, u64)> {
        let bytes = tokio::fs::read(path).await?;
        let size = bytes.len() as u64;
        let root = self.store_bytes(&bytes).await?;
        Ok((root, size))
    }

    async fn add_bytes(&self, bytes: &[u8]) -> anyhow::Result<BlobRoot> {
        self.store_bytes(bytes).await
    }

    async fn has(&self, root: &BlobRoot) -> anyhow::Result<bool> {
        Ok(tokio::fs::try_exists(self.path_for(root)).await?)
    }

    async fn bao_slice(&self, root: &BlobRoot, chunk: u64) -> anyhow::Result<Vec<u8>> {
        let data = tokio::fs::read(self.path_for(root)).await?;
        // Proven pattern (primitives/storage/src/tests.rs::encode_chunk):
        // BlockSize::ZERO matches the host fn's own `BaoTree::new` call
        // (D.3.3's native impl notes -- chunk-level verification is
        // independent of any outboard block-size choice).
        let ob = PreOrderMemOutboard::create(&data, BlockSize::ZERO);
        let actual_root = *ob.root.as_bytes();
        anyhow::ensure!(
            &actual_root == root,
            "stored blob's computed root {} does not match requested root {} -- \
             on-disk corruption or a store_bytes/root mismatch bug",
            hex::encode(actual_root),
            hex::encode(root)
        );
        let ranges = ChunkRanges::from(ChunkNum(chunk)..ChunkNum(chunk + 1));
        let mut slice = Vec::new();
        encode_ranges_validated(&data, &ob, ranges.as_ref(), &mut slice)?;
        Ok(slice)
    }

    async fn read_range(&self, root: &BlobRoot, offset: u64, length: u32) -> anyhow::Result<Vec<u8>> {
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        let mut file = tokio::fs::File::open(self.path_for(root)).await?;
        file.seek(std::io::SeekFrom::Start(offset)).await?;
        // Read up to `length` bytes; a short read at EOF is not an error --
        // the caller (the retrieval protocol handler) sends back exactly
        // what was read, which is how the client learns it has reached the
        // end of the blob.
        let mut buf = vec![0u8; length as usize];
        let n = file.read(&mut buf).await?;
        buf.truncate(n);
        Ok(buf)
    }

    async fn delete(&self, root: &BlobRoot) -> anyhow::Result<()> {
        let path = self.path_for(root);
        if tokio::fs::try_exists(&path).await? {
            tokio::fs::remove_file(&path).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_blob(len: usize) -> Vec<u8> {
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

    #[tokio::test]
    async fn stored_blob_yields_a_slice_the_real_host_fn_accepts() {
        let dir = tempfile::tempdir().unwrap();
        let store = DiskBlobStore::new(dir.path().to_path_buf()).unwrap();
        let data = sample_blob(64 * 1024); // 64 KiB, 64 chunks at CHUNK_SIZE=1024
        let root = store.store_bytes(&data).await.unwrap();

        assert!(store.has(&root).await.unwrap());
        let slice = store.bao_slice(&root, 5).await.unwrap();

        // The real on-chain verifier -- not a re-implementation.
        assert!(argonaut_primitives_storage::bao_proof::verify_slice(
            &root,
            data.len() as u64,
            5,
            1,
            &slice,
        ));
        // A slice for chunk 5 must not verify as chunk 6 (D1's own acceptance bar).
        assert!(!argonaut_primitives_storage::bao_proof::verify_slice(
            &root,
            data.len() as u64,
            6,
            1,
            &slice,
        ));
    }

    #[tokio::test]
    async fn read_range_returns_exact_bytes_and_truncates_at_eof() {
        let dir = tempfile::tempdir().unwrap();
        let store = DiskBlobStore::new(dir.path().to_path_buf()).unwrap();
        let data = sample_blob(10_000);
        let root = store.store_bytes(&data).await.unwrap();

        let mid = store.read_range(&root, 4_000, 1_000).await.unwrap();
        assert_eq!(mid, data[4_000..5_000]);

        // Requesting past EOF returns whatever's left, not an error.
        let tail = store.read_range(&root, 9_500, 1_000).await.unwrap();
        assert_eq!(tail, data[9_500..10_000]);
        assert_eq!(tail.len(), 500);
    }

    #[tokio::test]
    async fn delete_then_has_is_false() {
        let dir = tempfile::tempdir().unwrap();
        let store = DiskBlobStore::new(dir.path().to_path_buf()).unwrap();
        let root = store.store_bytes(b"hello world, this is a test blob").await.unwrap();
        assert!(store.has(&root).await.unwrap());
        store.delete(&root).await.unwrap();
        assert!(!store.has(&root).await.unwrap());
    }
}
