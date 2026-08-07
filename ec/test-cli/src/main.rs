//! Entry point for the ec-test-cli crate
//!
//! SPDX-License-Identifier: MIT
//!

mod cli;
mod commands;
mod debug;

use clap::Parser;
use cli::{Cli, Command, SourceKind};
use ec_test_lib::Source;

fn dispatch<S: Source>(source: S, command: Command) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        Command::Thermal(cmd) => commands::thermal::run(source, cmd).map_err(Into::into),
        Command::Battery(cmd) => commands::battery::run(source, cmd).map_err(Into::into),
        Command::Rtc(cmd) => commands::rtc::run(source, cmd).map_err(Into::into),
        Command::Script(cmd) => commands::script::run(source, cmd),
        // `eval` requires the concrete ACPI source and is handled in `main`.
        #[cfg(target_os = "windows")]
        Command::Eval(_) => unreachable!("eval is dispatched before reaching a generic source"),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // `eval` needs the concrete ACPI source, which the generic `dispatch` can't provide.
    #[cfg(target_os = "windows")]
    if let Command::Eval(cmd) = &cli.command {
        return match cli.source {
            SourceKind::Acpi => commands::eval::run(&ec_test_lib::acpi::Acpi::new(cli.fan_instance), cmd),
            _ => Err("`eval` is only supported with `--source acpi`".into()),
        };
    }

    match cli.source {
        SourceKind::Mock => dispatch(ec_test_lib::mock::Mock::default(), cli.command),

        SourceKind::Serial => {
            let port = cli.port.expect("--port is required for --source serial");
            let hw_flow = matches!(cli.flow_control, cli::FlowControl::Hw);
            let source =
                ec_test_lib::serial::Serial::new(&port, cli.baud, hw_flow, cli.sensor_instance, cli.fan_instance)?;
            dispatch(source, cli.command)
        }

        #[cfg(target_os = "windows")]
        SourceKind::Acpi => dispatch(ec_test_lib::acpi::Acpi::new(cli.fan_instance), cli.command),

        #[cfg(target_os = "windows")]
        SourceKind::Windows => dispatch(ec_test_lib::windows::Windows::new()?, cli.command),
    }
}
