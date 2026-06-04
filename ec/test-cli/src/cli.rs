//! CLI tool for EC feature testing.
//!
//! SPDX-License-Identifier: MIT
//!

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "ec-test-cli", about = "CLI tool for EC feature testing")]
pub struct Cli {
    /// Data source to use.
    #[arg(long, value_enum, default_value_t = SourceKind::default())]
    pub source: SourceKind,

    /// Serial port path (required when --source serial).
    #[arg(long, required_if_eq("source", "serial"))]
    pub port: Option<String>,

    /// Serial flow-control mode.
    #[arg(long, value_enum, default_value_t = FlowControl::None)]
    pub flow_control: FlowControl,

    /// Serial baud rate.
    #[arg(long, default_value_t = 115_200)]
    pub baud: u32,

    /// Sensor instance index.
    #[arg(long, default_value_t = 0)]
    pub sensor_instance: u8,

    /// Fan instance index.
    #[arg(long, default_value_t = 0)]
    pub fan_instance: u8,

    #[command(subcommand)]
    pub command: Command,
}

/// Available data sources.
#[derive(Clone, Copy, Default, ValueEnum)]
pub enum SourceKind {
    /// Deterministic in-process mock — no hardware required.
    Mock,
    /// Real hardware via serial transport.
    #[cfg_attr(not(target_os = "windows"), default)]
    Serial,
    #[cfg(target_os = "windows")]
    /// Real hardware via the local OS interface (Windows ACPI).
    #[default]
    Local,
}

impl std::fmt::Display for SourceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mock => write!(f, "mock"),
            Self::Serial => write!(f, "serial"),
            #[cfg(target_os = "windows")]
            Self::Local => write!(f, "local"),
        }
    }
}

#[derive(Clone, Copy, Default, ValueEnum)]
pub enum FlowControl {
    #[default]
    #[value(name = "none")]
    None,
    #[value(name = "hw")]
    Hw,
}

#[derive(Subcommand)]
pub enum Command {
    #[command(subcommand)]
    Thermal(ThermalCommand),
    #[command(subcommand)]
    Battery(BatteryCommand),
    #[command(subcommand)]
    Rtc(RtcCommand),
    #[command(subcommand)]
    Script(ScriptCommand),
}

#[derive(Subcommand)]
pub enum ScriptCommand {
    /// Run a text-DSL integration-test script against the selected source.
    ///
    /// `path` may be a single `.test` file or a directory; directories are
    /// walked recursively and every `*.test` file found is executed.
    Run {
        /// Path to a `.test` file or directory of `.test` files.
        path: std::path::PathBuf,
    },
}

#[derive(Subcommand)]
pub enum ThermalCommand {
    GetTemperature,
    GetRpm,
    GetMinRpm,
    GetMaxRpm,
    GetThreshold {
        #[arg(value_enum)]
        threshold: ThresholdArg,
    },
    SetRpm {
        rpm: f64,
    },
}

#[derive(Subcommand)]
pub enum BatteryCommand {
    GetBst,
    GetBix,
    SetBtp { trippoint: u32 },
}

#[derive(Subcommand)]
#[allow(clippy::enum_variant_names)]
pub enum RtcCommand {
    GetCapabilities,
    GetRealTime,
    GetWakeStatus {
        #[arg(value_enum)]
        timer: TimerIdArg,
    },
    GetExpiredTimerWakePolicy {
        #[arg(value_enum)]
        timer: TimerIdArg,
    },
    GetTimerValue {
        #[arg(value_enum)]
        timer: TimerIdArg,
    },
}

#[derive(Clone, ValueEnum)]
pub enum ThresholdArg {
    On,
    Ramping,
    Max,
}

impl From<ThresholdArg> for ec_test_lib::Threshold {
    fn from(arg: ThresholdArg) -> Self {
        match arg {
            ThresholdArg::On => Self::On,
            ThresholdArg::Ramping => Self::Ramping,
            ThresholdArg::Max => Self::Max,
        }
    }
}

#[derive(Clone, ValueEnum)]
pub enum TimerIdArg {
    Ac,
    Dc,
}

impl From<TimerIdArg> for time_alarm_service_interface::AcpiTimerId {
    fn from(arg: TimerIdArg) -> Self {
        match arg {
            TimerIdArg::Ac => Self::AcPower,
            TimerIdArg::Dc => Self::DcPower,
        }
    }
}
