//! Library half of the D3 provider daemon (`argonaut-provider`, `main.rs`) --
//! split out so `blobs`/`chain`/`responder` are independently testable and
//! reusable (e.g. by a future keeper binary for D5 repair, which is the same
//! shape: watch epochs, act on selected work).

pub mod blobs;
pub mod chain;
pub mod protocol;
pub mod responder;
