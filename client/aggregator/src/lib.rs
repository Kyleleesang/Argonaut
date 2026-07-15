//! STARK aggregator: batches XMSS signatures and proves them via leanMultisig.
//!
//! Non-interactive: anyone with the signatures can aggregate, not just the wallet.
//! Batches close at min(N_max=1024, T_max=750ms) per §4.3.
//!
//! # AggregationBackend trait
//! leanMultisig sits behind this trait so it can be swapped for a different
//! STARK prover (winterfell, Plonky3, etc.) without touching call sites.
//! Wire the actual leanMultisig backend behind a feature flag once the upstream
//! crate is pinned at a stable revision (milestone C1).
//!
//! # Fee model (documented per §4.3)
//! Bundle fee = base_fee + per_inner_tx_fee.
//! The aggregator collects inner-tx tips — this is what pays for prover hardware.
use argonaut_primitives_agg::{AggBundle, AggSchemeId, XmssRootRef};

pub const N_MAX: usize = 1024;
pub const T_MAX_MS: u64 = 750;

/// A batch ready for proving.
pub struct Batch {
    pub txs: Vec<Vec<u8>>,
    pub signer_keys: Vec<XmssRootRef>,
    pub epochs: Vec<u32>,
    pub xmss_sigs: Vec<Vec<u8>>,
}

/// Prover backend error.
#[derive(Debug, thiserror::Error)]
pub enum ProverError {
    #[error("Prover backend failed: {0}")]
    Backend(String),
}

/// Pluggable prover backend.
/// Implement with leanMultisig behind a feature flag; swap as needed.
pub trait AggregationBackend: Send + Sync {
    fn prove(&self, scheme: &AggSchemeId, batch: &Batch) -> Result<Vec<u8>, ProverError>;
}

/// Stub backend — returns empty proof (always fails on-chain verify until wired).
pub struct StubBackend;

impl AggregationBackend for StubBackend {
    fn prove(&self, _scheme: &AggSchemeId, _batch: &Batch) -> Result<Vec<u8>, ProverError> {
        Ok(vec![]) // replace with leanMultisig at C1
    }
}

/// The aggregator collects submissions, closes batches, and produces AggBundles.
pub struct Aggregator {
    pub scheme: AggSchemeId,
    backend: Box<dyn AggregationBackend>,
    pending: Vec<(Vec<u8>, Vec<u8>, XmssRootRef, u32)>, // (tx, xmss_sig, pk_root, epoch)
}

impl Aggregator {
    pub fn new(scheme: AggSchemeId, backend: Box<dyn AggregationBackend>) -> Self {
        Aggregator { scheme, backend, pending: Vec::new() }
    }

    /// Submit an XMSS-signed tx to the pending batch.
    pub fn submit(&mut self, tx: Vec<u8>, xmss_sig: Vec<u8>, pk_root: XmssRootRef, epoch: u32) {
        self.pending.push((tx, xmss_sig, pk_root, epoch));
    }

    /// Close the pending batch and produce an AggBundle, or None if empty.
    pub fn close_batch(&mut self) -> Option<Result<AggBundle, ProverError>> {
        if self.pending.is_empty() {
            return None;
        }
        let items = std::mem::take(&mut self.pending);
        let batch = Batch {
            txs: items.iter().map(|(tx, ..)| tx.clone()).collect(),
            xmss_sigs: items.iter().map(|(_, sig, ..)| sig.clone()).collect(),
            signer_keys: items.iter().map(|(.., pk, _)| *pk).collect(),
            epochs: items.iter().map(|(.., e)| *e).collect(),
        };
        let proof = match self.backend.prove(&self.scheme, &batch) {
            Ok(p) => p,
            Err(e) => return Some(Err(e)),
        };
        Some(Ok(AggBundle {
            scheme: self.scheme.clone(),
            txs: batch.txs,
            signer_keys: batch.signer_keys,
            epochs: batch.epochs,
            proof,
        }))
    }
}
