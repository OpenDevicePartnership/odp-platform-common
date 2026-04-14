use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "ec-test-cli", about = "CLI tool for EC feature testing")]
pub struct Cli {
    /// Data source to use.
    #[arg(long, value_enum)]
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

    #[command(subcommand)]
    pub command: Command,
}

/// Available data sources.
#[derive(Clone, Copy, ValueEnum)]
pub enum SourceKind {
    /// Deterministic in-process mock — no hardware required.
    Mock,
    /// Real hardware via serial transport.
    Serial,
    /// Real hardware via ACPI (Windows only).
    Acpi,
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

impl From<TimerIdArg> for time_alarm_service_messages::AcpiTimerId {
    fn from(arg: TimerIdArg) -> Self {
        match arg {
            TimerIdArg::Ac => Self::AcPower,
            TimerIdArg::Dc => Self::DcPower,
        }
    }
}
