mod app;
mod battery;
mod common;
mod rtc;
mod thermal;
mod ucsi;
mod widgets;

use std::time::Duration;

use clap::Parser;

/// ODP Embedded Controller demo TUI.
#[derive(Parser)]
#[command(about, version)]
struct Cli {
    /// Data source to use.
    #[arg(long, value_enum)]
    source: SourceKind,

    /// Serial port path (required when --source serial).
    #[arg(long, required_if_eq("source", "serial"))]
    port: Option<String>,

    /// Serial flow-control mode.
    #[arg(long, value_enum, default_value_t = FlowControl::None)]
    flow_control: FlowControl,

    /// Serial baud rate.
    #[arg(long, default_value_t = 115_200)]
    baud: u32,
}

/// Available data sources (only variants whose feature is compiled in are shown).
#[derive(clap::ValueEnum, Clone, Copy)]
enum SourceKind {
    /// Deterministic in-process mock — no hardware required.
    Mock,
    /// Real hardware via serial transport.
    Serial,
    /// Real hardware via ACPI (aarch64-pc-windows-msvc only).
    #[cfg(feature = "acpi")]
    Acpi,
}

#[derive(clap::ValueEnum, Clone, Copy, Default)]
enum FlowControl {
    #[default]
    #[value(name = "none")]
    None,
    #[value(name = "hw")]
    Hardware,
}

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    let cli = Cli::parse();
    let terminal = ratatui::init();

    match cli.source {
        SourceKind::Mock => app::App::new(ec_test_lib::mock::Mock::default(), Duration::from_secs(1)).run(terminal),

        SourceKind::Serial => {
            let port = cli.port.expect("--port is required for --source serial");
            let hw_flow = matches!(cli.flow_control, FlowControl::Hardware);
            let source = ec_test_lib::serial::Serial::new(&port, cli.baud, hw_flow)?;
            app::App::new(source, Duration::from_secs(60)).run(terminal)
        }

        #[cfg(feature = "acpi")]
        SourceKind::Acpi => app::App::new(ec_test_lib::acpi::Acpi::default(), Duration::from_secs(60)).run(terminal),
    }
}
