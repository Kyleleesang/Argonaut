//! Substrate Node Template CLI library.
#![warn(missing_docs)]

mod chain_spec;
#[macro_use]
mod service;
#[cfg(feature = "dev-signatures")]
mod benchmarking;
mod bridge;
mod cli;
mod command;
mod falcon_sign;
mod keygen;
mod rpc;

fn main() -> sc_cli::Result<()> {
	command::run()
}
