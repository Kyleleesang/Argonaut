//! RocksDB backing store for the Prism DAG.
//!
//! Block inserts use `WriteBatch` for crash consistency — a block is never
//! half-indexed on disk. See `columns` for the B5 pruning decision that
//! trimmed this crate's column families down to the ones actually used.
pub mod columns;
pub mod db;

pub use db::{BlockStatus, DbError, PrismDb};
