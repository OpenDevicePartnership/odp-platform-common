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
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.source {
        SourceKind::Mock => dispatch(ec_test_lib::mock::Mock::default(), cli.command),

        SourceKind::Serial => {
            let port = cli.port.expect("--port is required for --source serial");
            let hw_flow = matches!(cli.flow_control, cli::FlowControl::Hw);
            let source = ec_test_lib::serial::Serial::new(&port, cli.baud, hw_flow)?;
            dispatch(source, cli.command)
        }

        SourceKind::Acpi => dispatch(ec_test_lib::acpi::Acpi::default(), cli.command),
    }
}
