use crate::columns::*;
use argonaut_primitives_prism::{block_hash, PrismBlock, H256};
use parity_scale_codec::{Decode, Encode};
use rocksdb::{ColumnFamilyDescriptor, Options, WriteBatch, DB};
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
