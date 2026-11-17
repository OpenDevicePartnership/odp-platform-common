//! CLI EC Test tool `battery` subcommand.
//!
//! SPDX-License-Identifier: MIT
//!

use crate::cli::BatteryCommand;
use crate::debug::{DebugBixFixedStrings, DebugBstReturn, DebugPsrReturn};
use ec_test_lib::BatterySource;

pub fn run<S: BatterySource>(source: S, cmd: BatteryCommand) -> Result<(), S::Error> {
    match cmd {
        BatteryCommand::GetBst => println!("{:?}", DebugBstReturn(&source.get_bst()?)),
        BatteryCommand::GetBix => println!("{:?}", DebugBixFixedStrings(&source.get_bix()?)),
        BatteryCommand::GetPsr => println!("{:?}", DebugPsrReturn(&source.get_psr()?)),
        BatteryCommand::SetBtp { trippoint } => source.set_btp(trippoint)?,
    }
    Ok(())
}
