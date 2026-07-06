//! B0: a minimal blake3 `PowAlgorithm` for `sc-consensus-pow` (architecture guide §3.9).
//!
//! Deliberately simple: fixed difficulty, no retargeting, no runtime-side
//! `DifficultyApi`. This milestone exists to rehearse the import-queue/mining-worker
//! plumbing before it's needed for real in B3, where this whole module gets replaced
//! by the Prism-ledger bridge (which imports blocks built from confirmed Prism leader
//! levels, not from client-side PoW mining against the plain chain). Nothing here is
//! meant to survive past that point.
use argonaut_runtime::opaque::Block;
use sc_consensus_pow::{Error as PowError, PowAlgorithm};
use sp_core::U256;
use sp_runtime::{generic::BlockId, traits::Block as BlockT};

/// Fixed mining target -- calibrated empirically against this dev machine's blake3
/// throughput to land around single-digit-second block times, nothing more
/// principled than that.
///
/// KNOWN LIMITATION, flagged for follow-up: this is a hardcoded constant with no
/// retargeting at all. It does not adapt to network hash rate. If more miners join
/// (or run on faster hardware), blocks will simply come out faster than intended --
/// there's no feedback loop pulling it back down, unlike Bitcoin-style rolling-window
/// retargeting (which is exactly what the architecture guide's *real* Prism miner
/// design calls for in §3.5, deliberately out of scope for this B0 rehearsal chain).
/// Before this consensus is used for anything beyond a single-node/local-devnet
/// rehearsal, this needs either a `DifficultyApi`-backed on-chain adjustment or at
/// least a client-side rolling-window retarget against recent block timestamps.
pub fn fixed_difficulty() -> U256 {
    U256::max_value() >> 24
}

#[derive(Clone)]
pub struct Blake3Pow;

impl PowAlgorithm<Block> for Blake3Pow {
    type Difficulty = U256;

    fn difficulty(&self, _parent: <Block as BlockT>::Hash) -> Result<Self::Difficulty, PowError<Block>> {
        Ok(fixed_difficulty())
    }

    fn verify(
        &self,
        _parent: &BlockId<Block>,
        pre_hash: &<Block as BlockT>::Hash,
        _pre_digest: Option<&[u8]>,
        seal: &sp_consensus_pow::Seal,
        difficulty: Self::Difficulty,
    ) -> Result<bool, PowError<Block>> {
        let nonce = match <[u8; 8]>::try_from(seal.as_slice()) {
            Ok(bytes) => u64::from_le_bytes(bytes),
            Err(_) => return Ok(false),
        };
        Ok(seal_value(pre_hash.as_ref(), nonce) <= difficulty)
    }
}

/// blake3(pre_hash ++ nonce), interpreted as a big-endian U256 -- the same shape
/// `argonaut_primitives_prism::block_hash`/`sortition` use for its own PoW check.
pub fn seal_value(pre_hash: &[u8], nonce: u64) -> U256 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(pre_hash);
    hasher.update(&nonce.to_le_bytes());
    U256::from_big_endian(hasher.finalize().as_bytes())
}
