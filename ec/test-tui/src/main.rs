mod app;
mod battery;
mod common;
mod rtc;
mod thermal;
mod ucsi;
mod widgets;

use std::time::Duration;

use clap::Parser;
use tracing_subscriber::EnvFilter;

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

    /// Sensor instance index.
    #[arg(long, default_value_t = 0)]
    sensor_instance: u8,

    /// Fan instance index.
    #[arg(long, default_value_t = 0)]
    fan_instance: u8,

    /// Battery graph sample period in seconds.
    /// Defaults to 1 for mock sources and 60 for real hardware.
    #[arg(long)]
    sample_period: Option<u64>,
}

/// Available data sources (only variants whose feature is compiled in are shown).
#[derive(clap::ValueEnum, Clone, Copy)]
enum SourceKind {
    /// Deterministic in-process mock — no hardware required.
    Mock,
    /// Real hardware via serial transport.
    Serial,
    #[cfg(target_os = "windows")]
    /// Real hardware via ACPI (aarch64-pc-windows-msvc only).
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

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    // Log to a file rather than stderr to avoid corrupting the ratatui terminal.
    let log_file = std::fs::File::create("ec-test-tui.log")?;
    tracing_subscriber::fmt()
        .with_writer(log_file)
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    color_eyre::install()?;

    let cli = Cli::parse();
    let terminal = ratatui::init();

    match cli.source {
        SourceKind::Mock => {
            let period = Duration::from_secs(cli.sample_period.unwrap_or(1));
            app::App::new(ec_test_lib::mock::Mock::default(), period).run(terminal)
        }

        SourceKind::Serial => {
            let port = cli.port.expect("--port is required for --source serial");
            let hw_flow = matches!(cli.flow_control, FlowControl::Hardware);
            let source =
                ec_test_lib::serial::Serial::new(&port, cli.baud, hw_flow, cli.sensor_instance, cli.fan_instance)?;
            let period = Duration::from_secs(cli.sample_period.unwrap_or(60));
            app::App::new(source, period).run(terminal)
        }

        #[cfg(target_os = "windows")]
        SourceKind::Acpi => {
            let period = Duration::from_secs(cli.sample_period.unwrap_or(60));
            app::App::new(ec_test_lib::acpi::Acpi::new(cli.fan_instance), period).run(terminal)
        }
    }
}
