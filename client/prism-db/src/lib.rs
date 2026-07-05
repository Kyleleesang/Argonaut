//! RocksDB backing store for the Prism DAG.
//!
//! Column families mirror prism-rust verbatim so naming is stable across
//! future cross-repo comparisons. Block inserts use `WriteBatch` for crash
//! consistency — a block is never half-indexed on disk.
pub mod columns;
pub mod db;

pub use db::{BlockStatus, DbError, PrismDb};
