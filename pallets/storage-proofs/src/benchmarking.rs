//! Benchmarks for `pallet-storage-proofs` (placeholder weights, spec D.1 rule 8).
//!
//! `submit_proof` / `report_missed` require a fully activated deal in
//! `storage-deals` plus a real bao slice, which the unit tests cover. A proper
//! benchmark harness for those is deferred; this module is an empty stub so the
//! `runtime-benchmarks` feature still compiles. The pallet is intentionally NOT
//! listed in the runtime's `define_benchmarks!` yet.
use super::*;
use frame_benchmarking::v2::*;

#[benchmarks]
mod benchmarks {
    use super::*;

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
