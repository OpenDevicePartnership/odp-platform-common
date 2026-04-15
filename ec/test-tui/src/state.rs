use battery_service_messages::{BixFixedStrings, BstReturn};
use time_alarm_service_messages::{
    AcpiTimestamp, AlarmExpiredWakePolicy, AlarmTimerSeconds, TimeAlarmDeviceCapabilities,
    TimerStatus,
};

use crate::common::SampleBuf;

pub const BATTERY_MAX_SAMPLES: usize = 60;
pub const THERMAL_MAX_SAMPLES: usize = 60;

/// `None` = not yet fetched, `Some(Ok(v))` = success, `Some(Err(e))` = fetch failed.
pub type Fetched<T> = Option<color_eyre::Result<T>>;

// ── Battery ──────────────────────────────────────────────────────────────────

/// Live battery state (BST + BIX + graph samples).
///
/// Written exclusively by the background [`crate::updater::Updater`];
/// read by the battery UI module for rendering.
pub struct BatteryState {
    pub bst: BstReturn,
    pub bst_success: bool,
    pub bix: BixFixedStrings,
    pub bix_success: bool,
    pub samples: SampleBuf<u32, BATTERY_MAX_SAMPLES>,
    pub t_min: usize,
    /// Last trip-point value set by the user via the BTP input box.
    pub btp: u32,
    /// Whether the most recent `set_btp` call succeeded.
    pub btp_success: bool,
}

impl Default for BatteryState {
    fn default() -> Self {
        Self {
            bst: Default::default(),
            bst_success: false,
            bix: Default::default(),
            bix_success: false,
            samples: Default::default(),
            t_min: 0,
            btp: 0,
            // Start as `true` so the BTP indicator is green before any user interaction.
            btp_success: true,
        }
    }
}

/// Write-back command from the battery UI to the background updater.
pub enum BatteryCommand {
    /// Set the battery trip-point (BTP) to the given value.
    SetBtp(u32),
}

// ── Thermal ──────────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct SensorThresholds {
    #[allow(dead_code)] // reserved for future low-threshold enforcement
    pub warn_low: f64,
    pub warn_high: f64,
    pub prochot: f64,
    pub critical: f64,
}

#[derive(Default)]
pub struct FanRpmBounds {
    pub min: f64,
    pub max: f64,
}

#[derive(Default)]
pub struct FanStateLevels {
    pub on: f64,
    pub ramping: f64,
    pub max: f64,
}

#[derive(Default)]
pub struct SensorData {
    pub skin_temp: f64,
    pub temp_success: bool,
    pub thresholds: SensorThresholds,
    pub thresholds_success: bool,
    pub samples: SampleBuf<f64, THERMAL_MAX_SAMPLES>,
}

#[derive(Default)]
pub struct FanData {
    pub rpm: f64,
    pub rpm_success: bool,
    pub rpm_bounds: FanRpmBounds,
    pub bounds_success: bool,
    pub state_levels: FanStateLevels,
    pub levels_success: bool,
    pub samples: SampleBuf<u32, THERMAL_MAX_SAMPLES>,
}

#[derive(Default)]
pub struct ThermalState {
    pub sensor: SensorData,
    pub fan: FanData,
    /// Monotonic tick counter; used for graph x-axis labels.
    pub t: usize,
}

/// Write-back command from the thermal UI to the background updater.
pub enum ThermalCommand {
    /// Set the fan RPM limit.
    SetRpm(f64),
}

// ── RTC ──────────────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct TimerData {
    pub value: Fetched<AlarmTimerSeconds>,
    pub wake_policy: Fetched<AlarmExpiredWakePolicy>,
    pub timer_status: Fetched<TimerStatus>,
}

#[derive(Default)]
pub struct RtcState {
    pub capabilities: Fetched<TimeAlarmDeviceCapabilities>,
    pub timestamp: Fetched<AcpiTimestamp>,
    /// `[0]` = AC Power timer, `[1]` = DC Power timer.
    pub timers: [TimerData; 2],
}

// ── AppState ─────────────────────────────────────────────────────────────────

/// All shared data state.  Written by the background updater; read by the UI.
#[derive(Default)]
pub struct AppState {
    pub battery: BatteryState,
    pub thermal: ThermalState,
    pub rtc: RtcState,
}
