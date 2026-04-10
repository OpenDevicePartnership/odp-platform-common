const _: () = {
    let count = cfg!(feature = "mock") as u8 + cfg!(feature = "acpi") as u8 + cfg!(feature = "serial") as u8;
    assert!(
        count == 1,
        "Exactly one of the following features must be enabled: `mock`, `acpi`, or `serial`."
    );
};

mod cli;
mod commands;
mod debug;

use clap::Parser;
use cli::{Cli, Command};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    #[cfg(feature = "mock")]
    let source = ec_test_lib::mock::Mock::default();

    #[cfg(feature = "acpi")]
    let source = ec_test_lib::acpi::Acpi::default();

    #[cfg(feature = "serial")]
    let source = {
        let flow_control = cli.flow_control == cli::FlowControl::Hw;
        ec_test_lib::serial::Serial::new(&cli.port, cli.baud, flow_control)?
    };

    match cli.command {
        Command::Thermal(cmd) => commands::thermal::run(source, cmd)?,
        Command::Battery(cmd) => commands::battery::run(source, cmd)?,
        Command::Rtc(cmd) => commands::rtc::run(source, cmd)?,
    }

    Ok(())
}
