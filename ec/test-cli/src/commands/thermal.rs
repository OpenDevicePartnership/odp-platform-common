//! CLI EC Test tool `thermal` subcommand
//!
//! SPDX-License-Identifier: MIT
//!

use crate::cli::ThermalCommand;
use ec_test_lib::ThermalSource;

pub fn run<S: ThermalSource>(source: S, cmd: ThermalCommand) -> Result<(), S::Error> {
    match cmd {
        ThermalCommand::GetTemperature => println!("{:?}", source.get_temperature()?),
        ThermalCommand::GetRpm => println!("{:?}", source.get_rpm()?),
        ThermalCommand::GetMinRpm => println!("{:?}", source.get_min_rpm()?),
        ThermalCommand::GetMaxRpm => println!("{:?}", source.get_max_rpm()?),
        ThermalCommand::GetThreshold { threshold } => {
            println!("{:?}", source.get_threshold(threshold.into())?)
        }
        ThermalCommand::SetRpm { rpm } => source.set_rpm(rpm)?,
    }
    Ok(())
}
