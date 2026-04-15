mod app;
mod battery;
mod common;
mod logging;
mod rtc;
mod state;
mod thermal;
mod updater;
mod widgets;

use std::{path::PathBuf, sync::{Arc, Mutex, RwLock}, time::Duration};

use clap::Parser;
use tracing_subscriber::{EnvFilter, prelude::*};

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

    /// Write logs to this file in addition to the in-app log panel.
    #[arg(long)]
    log_file: Option<PathBuf>,
}

/// Available data sources (only variants whose feature is compiled in are shown).
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
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
    let cli = Cli::parse();

    // Build an optional file layer when --log-file is supplied.
    let file_layer: Option<_> = cli
        .log_file
        .as_ref()
        .map(|path| -> color_eyre::Result<_> {
            let file = std::fs::File::create(path)?;
            Ok(tracing_subscriber::fmt::layer().with_writer(Mutex::new(file)))
        })
        .transpose()?;

    let log_buffer = logging::LogBuffer::default();

    // Default to WARN when RUST_LOG is not set so the panel isn't flooded.
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(logging::TuiLayer::new(log_buffer.clone()))
        .with(file_layer)
        .init();

    color_eyre::install()?;

    tracing::debug!("Starting EC test TUI with source: '{:?}'", cli.source);

    let terminal = ratatui::init();

    match cli.source {
        SourceKind::Mock => {
            let period = Duration::from_secs(cli.sample_period.unwrap_or(1));
            run_with_source(ec_test_lib::mock::Mock::default(), period, log_buffer, terminal)
        }

        SourceKind::Serial => {
            let port = cli.port.expect("--port is required for --source serial");
            let hw_flow = matches!(cli.flow_control, FlowControl::Hardware);
            let source =
                ec_test_lib::serial::Serial::new(&port, cli.baud, hw_flow, cli.sensor_instance, cli.fan_instance)?;
            let period = Duration::from_secs(cli.sample_period.unwrap_or(60));
            run_with_source(source, period, log_buffer, terminal)
        }

        #[cfg(target_os = "windows")]
        SourceKind::Acpi => {
            let period = Duration::from_secs(cli.sample_period.unwrap_or(60));
            run_with_source(ec_test_lib::acpi::Acpi::new(cli.fan_instance), period, log_buffer, terminal)
        }
    }
}

fn run_with_source<S>(
    source: S,
    period: Duration,
    log_buffer: logging::LogBuffer,
    terminal: ratatui::DefaultTerminal,
) -> color_eyre::Result<()>
where
    S: ec_test_lib::Source + Send + Sync + 'static,
{
    let shared_state = Arc::new(RwLock::new(state::AppState::default()));
    let (battery_tx, battery_rx) = std::sync::mpsc::channel::<state::BatteryCommand>();
    let (thermal_tx, thermal_rx) = std::sync::mpsc::channel::<state::ThermalCommand>();
    let upd = updater::Updater::new(
        Arc::new(source),
        Arc::clone(&shared_state),
        battery_rx,
        thermal_rx,
        period,
    );
    std::thread::spawn(move || upd.run(period));
    app::App::new(shared_state, battery_tx, thermal_tx, log_buffer).run(terminal)
}
