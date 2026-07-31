mod app;
mod battery;
mod common;
mod logging;
mod rtc;
mod source;
mod state;
mod system;
mod thermal;
mod ucsi;
mod updater;
mod widgets;

use std::{
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use clap::Parser;
use tracing_subscriber::{EnvFilter, prelude::*};

/// ODP Embedded Controller demo TUI.
#[derive(Parser)]
#[command(about, version)]
struct Cli {
    /// Data source to use.
    #[arg(long, value_enum, default_value_t = SourceKind::default())]
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

    /// Write logs to this file in addition to the in-app log panel.
    #[arg(long)]
    log_file: Option<PathBuf>,
}

/// Available data sources (only variants whose feature is compiled in are shown).
#[derive(clap::ValueEnum, Clone, Copy, Debug, Default)]
enum SourceKind {
    /// Deterministic in-process mock — no hardware required.
    Mock,
    /// Real hardware via serial transport.
    #[cfg_attr(not(target_os = "windows"), default)]
    Serial,
    /// Real hardware via the local OS ACPI interface (Windows).
    #[cfg(target_os = "windows")]
    #[default]
    Acpi,
    /// Real hardware via the local Windows class-driver interface.
    #[cfg(target_os = "windows")]
    Windows,
}

impl std::fmt::Display for SourceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mock => write!(f, "mock"),
            Self::Serial => write!(f, "serial"),
            #[cfg(target_os = "windows")]
            Self::Acpi => write!(f, "acpi"),
            #[cfg(target_os = "windows")]
            Self::Windows => write!(f, "windows"),
        }
    }
}

#[derive(clap::ValueEnum, Clone, Copy, Default)]
enum FlowControl {
    #[default]
    #[value(name = "none")]
    None,
    #[value(name = "hw")]
    Hardware,
}

/// Update periods — hardcoded, not user-configurable.
const BATTERY_PERIOD: Duration = Duration::from_secs(1);
const THERMAL_PERIOD: Duration = Duration::from_secs(1);
const RTC_PERIOD: Duration = Duration::from_secs(1);
const SYSTEM_PERIOD: Duration = Duration::from_millis(500);
const UCSI_PERIOD: Duration = Duration::from_secs(1);

fn init_tracing(cli: &Cli) -> color_eyre::Result<logging::LogBuffer> {
    let file_layer: Option<_> = cli
        .log_file
        .as_ref()
        .map(|path| -> color_eyre::Result<_> {
            let file = std::fs::File::create(path)?;
            Ok(tracing_subscriber::fmt::layer().with_writer(Mutex::new(file)))
        })
        .transpose()?;

    let log_buffer = logging::LogBuffer::new();
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(logging::TuiLayer::new(log_buffer.clone()))
        .with(file_layer)
        .init();

    Ok(log_buffer)
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    let cli = Cli::parse();
    let log_buffer = init_tracing(&cli)?;
    color_eyre::install()?;

    tracing::debug!("Starting EC test TUI with source: '{:?}'", cli.source);

    let source = source::build(&cli)?;
    let terminal = ratatui::init();

    let battery_state = Arc::new(RwLock::new(state::BatteryState::default()));
    let thermal_state = Arc::new(RwLock::new(state::ThermalState::default()));
    let rtc_state = Arc::new(RwLock::new(state::RtcState::default()));
    let system_state = Arc::new(RwLock::new(state::SystemState::default()));
    let ucsi_state = Arc::new(RwLock::new(state::UcsiState::default()));

    let (battery_tx, battery_rx) = std::sync::mpsc::channel::<state::BatteryCommand>();
    let (thermal_tx, thermal_rx) = std::sync::mpsc::channel::<state::ThermalCommand>();

    tokio::task::spawn({
        let upd = updater::BatteryUpdater::new(Arc::clone(&source), Arc::clone(&battery_state), battery_rx);
        async move { upd.run(BATTERY_PERIOD).await }
    });
    tokio::task::spawn({
        let upd = updater::ThermalUpdater::new(Arc::clone(&source), Arc::clone(&thermal_state), thermal_rx);
        async move { upd.run(THERMAL_PERIOD).await }
    });
    tokio::task::spawn({
        let upd = updater::RtcUpdater::new(Arc::clone(&source), Arc::clone(&rtc_state));
        async move { upd.run(RTC_PERIOD).await }
    });
    tokio::task::spawn({
        let upd = updater::SystemUpdater::new(Arc::clone(&system_state));
        async move { upd.run(SYSTEM_PERIOD).await }
    });
    tokio::task::spawn({
        let upd = updater::UcsiUpdater::new(Arc::clone(&source), Arc::clone(&ucsi_state));
        async move { upd.run(UCSI_PERIOD).await }
    });

    app::App::new(
        battery_state,
        thermal_state,
        rtc_state,
        system_state,
        ucsi_state,
        battery_tx,
        thermal_tx,
        log_buffer,
    )
    .run(terminal)
}
