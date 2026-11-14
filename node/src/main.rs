//! IPM (Intelligence Protocol Mechanism) reference node.
// sc_cli::Error is large by design in the Substrate CLI API.
#![allow(clippy::result_large_err)]
#![warn(missing_docs)]

mod chain_spec;
mod cli;
mod command;
mod rpc;
mod service;

use polkadot_sdk::*;

fn main() -> sc_cli::Result<()> {
    command::run()
}
