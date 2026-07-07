use crate::columns::*;
use argonaut_primitives_prism::{block_hash, PrismBlock, H256};
use parity_scale_codec::{Decode, Encode};
use rocksdb::{ColumnFamilyDescriptor, Options, WriteBatch, DB};
use sp_core::U256;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("RocksDB error: {0}")]
    Rocks(#[from] rocksdb::Error),
    #[error("SCALE decode error")]
    Codec,
}

/// Block lifecycle state.
#[derive(Encode, Decode, Clone, Debug, PartialEq, Eq)]
pub enum BlockStatus {
    Pending = 0,
    Valid = 1,
    Orphan = 2,
}

pub struct PrismDb {
    db: DB,
}

impl PrismDb {
    pub fn open(path: &Path) -> Result<Self, DbError> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);

        let cf_descs: Vec<ColumnFamilyDescriptor> = ALL_COLUMNS
            .iter()
            .map(|name| ColumnFamilyDescriptor::new(*name, Options::default()))
            .collect();

        let db = DB::open_cf_descriptors(&opts, path, cf_descs)?;
        Ok(PrismDb { db })
    }

    /// Insert a block atomically: raw bytes + status in a single WriteBatch.
    /// A block is never half-indexed on disk — crash consistency guaranteed.
    pub fn insert_block(&self, block: &PrismBlock) -> Result<(), DbError> {
        let hash = block_hash(&block.header);
        let encoded = block.encode();
        let mut batch = WriteBatch::default();

        let cf_blocks = self.db.cf_handle(BLOCKS).expect("BLOCKS CF missing");
        batch.put_cf(cf_blocks, hash.as_bytes(), &encoded);

        let cf_status = self.db.cf_handle(BLOCK_STATUS).expect("BLOCK_STATUS CF missing");
        batch.put_cf(cf_status, hash.as_bytes(), BlockStatus::Valid.encode());

        self.db.write(batch)?;
        Ok(())
    }

    /// Every block ever accepted (see `BLOCKS` CF — only accepted blocks are
    /// ever written there; true orphans, missing a dependency, never are), in
    /// arbitrary (RocksDB key/hash) order. Used by `PrismChain::with_db` to
    /// reconstruct in-memory DAG state after a restart — order doesn't matter
    /// there since `insert_block`'s orphan-cascade makes replay convergent
    /// regardless of the sequence blocks are fed in, as long as the full
    /// accepted set is eventually covered.
    pub fn iter_blocks(&self) -> impl Iterator<Item = PrismBlock> + '_ {
        let cf = self.db.cf_handle(BLOCKS).expect("BLOCKS CF missing");
        self.db
            .iterator_cf(cf, rocksdb::IteratorMode::Start)
            .filter_map(|res| res.ok())
            .filter_map(|(_, value)| PrismBlock::decode(&mut &value[..]).ok())
    }

    pub fn get_block(&self, hash: &H256) -> Result<Option<PrismBlock>, DbError> {
        let cf = self.db.cf_handle(BLOCKS).expect("BLOCKS CF missing");
        match self.db.get_cf(cf, hash.as_bytes())? {
            None => Ok(None),
            Some(bytes) => {
                PrismBlock::decode(&mut &bytes[..]).map(Some).map_err(|_| DbError::Codec)
            }
        }
    }

    pub fn get_block_status(&self, hash: &H256) -> Result<Option<BlockStatus>, DbError> {
        let cf = self.db.cf_handle(BLOCK_STATUS).expect("BLOCK_STATUS CF missing");
        match self.db.get_cf(cf, hash.as_bytes())? {
            None => Ok(None),
            Some(bytes) => {
                BlockStatus::decode(&mut &bytes[..]).map(Some).map_err(|_| DbError::Codec)
            }
        }
    }

    pub fn set_block_status(&self, hash: &H256, status: BlockStatus) -> Result<(), DbError> {
        let cf = self.db.cf_handle(BLOCK_STATUS).expect("BLOCK_STATUS CF missing");
        self.db.put_cf(cf, hash.as_bytes(), status.encode())?;
        Ok(())
    }

    pub fn put_u64(&self, cf_name: &str, key: &[u8], val: u64) -> Result<(), DbError> {
        let cf = self.db.cf_handle(cf_name).unwrap_or_else(|| panic!("{cf_name} CF missing"));
        self.db.put_cf(cf, key, val.to_le_bytes())?;
        Ok(())
    }

    pub fn get_u64(&self, cf_name: &str, key: &[u8]) -> Result<Option<u64>, DbError> {
        let cf = self.db.cf_handle(cf_name).unwrap_or_else(|| panic!("{cf_name} CF missing"));
        match self.db.get_cf(cf, key)? {
            None => Ok(None),
            Some(bytes) if bytes.len() == 8 => {
                Ok(Some(u64::from_le_bytes(bytes[..8].try_into().unwrap())))
            }
            _ => Err(DbError::Codec),
        }
    }

    /// `U256` is exactly 32 bytes, same as `H256` -- stored big-endian,
    /// matching how `sortition`/`block_hash` already interpret hash bytes as
    /// big-endian `U256` elsewhere in this codebase.
    pub fn put_u256(&self, cf_name: &str, key: &[u8], val: &U256) -> Result<(), DbError> {
        let cf = self.db.cf_handle(cf_name).unwrap_or_else(|| panic!("{cf_name} CF missing"));
        self.db.put_cf(cf, key, val.to_big_endian())?;
        Ok(())
    }

    pub fn get_u256(&self, cf_name: &str, key: &[u8]) -> Result<Option<U256>, DbError> {
        let cf = self.db.cf_handle(cf_name).unwrap_or_else(|| panic!("{cf_name} CF missing"));
        match self.db.get_cf(cf, key)? {
            None => Ok(None),
            Some(bytes) if bytes.len() == 32 => Ok(Some(U256::from_big_endian(&bytes))),
            _ => Err(DbError::Codec),
        }
    }

    pub fn put_h256(&self, cf_name: &str, key: &[u8], val: &H256) -> Result<(), DbError> {
        let cf = self.db.cf_handle(cf_name).unwrap_or_else(|| panic!("{cf_name} CF missing"));
        self.db.put_cf(cf, key, val.as_bytes())?;
        Ok(())
    }

    pub fn get_h256(&self, cf_name: &str, key: &[u8]) -> Result<Option<H256>, DbError> {
        let cf = self.db.cf_handle(cf_name).unwrap_or_else(|| panic!("{cf_name} CF missing"));
        match self.db.get_cf(cf, key)? {
            None => Ok(None),
            Some(bytes) if bytes.len() == 32 => Ok(Some(H256::from_slice(&bytes))),
            _ => Err(DbError::Codec),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use argonaut_primitives_prism::{BlockContent, MerkleProof, SuperHeader};
    use sp_core::U256;

    fn tmp_db() -> (tempfile::TempDir, PrismDb) {
        let dir = tempfile::tempdir().expect("create tempdir");
        let db = PrismDb::open(dir.path()).expect("open PrismDb");
        (dir, db)
    }

    fn dummy_tx_block(nonce: u64) -> PrismBlock {
        PrismBlock {
            header: SuperHeader {
                parents_root: H256::zero(),
                content_root: H256::zero(),
                timestamp_ms: 0,
                nonce,
                difficulty: U256::max_value(),
            },
            parent: H256::zero(),
            content: BlockContent::Transaction { txs: vec![vec![nonce as u8]] },
            parent_proof: MerkleProof::default(),
            content_proof: MerkleProof::default(),
        }
    }

    #[test]
    fn insert_and_get_block_round_trips() {
        let (_dir, db) = tmp_db();
        let block = dummy_tx_block(1);
        let hash = block_hash(&block.header);

        assert!(db.get_block(&hash).unwrap().is_none());
        db.insert_block(&block).unwrap();

        let fetched = db.get_block(&hash).unwrap().expect("block should be present");
        assert_eq!(fetched, block);
    }

    #[test]
    fn insert_block_sets_status_valid() {
        let (_dir, db) = tmp_db();
        let block = dummy_tx_block(2);
        let hash = block_hash(&block.header);

        assert_eq!(db.get_block_status(&hash).unwrap(), None);
        db.insert_block(&block).unwrap();
        assert_eq!(db.get_block_status(&hash).unwrap(), Some(BlockStatus::Valid));
    }

    #[test]
    fn set_block_status_overrides_it() {
        let (_dir, db) = tmp_db();
        let block = dummy_tx_block(3);
        let hash = block_hash(&block.header);
        db.insert_block(&block).unwrap();

        db.set_block_status(&hash, BlockStatus::Orphan).unwrap();
        assert_eq!(db.get_block_status(&hash).unwrap(), Some(BlockStatus::Orphan));
    }

    #[test]
    fn u64_round_trip() {
        let (_dir, db) = tmp_db();
        db.put_u64(PROPOSER_TREE_LEVEL, b"key", 42).unwrap();
        assert_eq!(db.get_u64(PROPOSER_TREE_LEVEL, b"key").unwrap(), Some(42));
        assert_eq!(db.get_u64(PROPOSER_TREE_LEVEL, b"missing").unwrap(), None);
    }

    #[test]
    fn h256_round_trip() {
        let (_dir, db) = tmp_db();
        let val = H256::from([7u8; 32]);
        db.put_h256(PROPOSER_NODE_VOTE, b"key", &val).unwrap();
        assert_eq!(db.get_h256(PROPOSER_NODE_VOTE, b"key").unwrap(), Some(val));
        assert_eq!(db.get_h256(PROPOSER_NODE_VOTE, b"missing").unwrap(), None);
    }

    #[test]
    fn reopening_the_same_path_preserves_data() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let block = dummy_tx_block(4);
        let hash = block_hash(&block.header);
        {
            let db = PrismDb::open(dir.path()).expect("open PrismDb");
            db.insert_block(&block).unwrap();
        }
        // Re-open against the same on-disk path -- data must survive.
        let db = PrismDb::open(dir.path()).expect("re-open PrismDb");
        assert_eq!(db.get_block(&hash).unwrap(), Some(block));
    }
}
