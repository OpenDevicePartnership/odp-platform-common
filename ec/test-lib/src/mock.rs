use crate::{BatterySource, ErrorType, RtcSource, ThermalSource, Threshold};
use battery_service_interface::{
    BatteryState, BatterySwapCapability, BatteryTechnology, BixFixedStrings, BstReturn, PowerUnit,
};
use embedded_mcu_hal::time::{Datetime, DatetimeFields, Month};
use std::sync::Mutex;
use std::time::Instant;
use time_alarm_service_interface::{
    AcpiDaylightSavingsTimeStatus, AcpiTimeZone, AcpiTimeZoneOffset, AcpiTimerId, AcpiTimestamp,
    AlarmExpiredWakePolicy, AlarmTimerSeconds, TimeAlarmDeviceCapabilities, TimerStatus,
};

/// Errors produced by mock data source operations.
#[derive(Debug)]
pub enum Error {
    /// Data validation failed (invalid enum discriminant, malformed field, etc.)
    InvalidData,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidData => write!(f, "Invalid data"),
        }
    }
}

impl std::error::Error for Error {}

impl crate::Error for Error {
    fn kind(&self) -> crate::ErrorKind {
        match self {
            Self::InvalidData => crate::ErrorKind::InvalidData,
        }
    }
}

// ── Ramp helper (shared by fan and temperature) ─────────────────────────────

/// Tracks a smooth linear ramp between two values over a given duration.
struct Ramp {
    current: f64,
    target: f64,
    start_value: f64,
    start_time: Instant,
    duration_secs: f64,
}

impl Ramp {
    fn new(initial: f64) -> Self {
        Self {
            current: initial,
            target: initial,
            start_value: initial,
            start_time: Instant::now(),
            duration_secs: 0.0,
        }
    }

    /// Begin ramping toward `target` over `duration` seconds.
    /// No-op if the target hasn't changed.
    fn set_target(&mut self, target: f64, duration: f64) {
        if (self.target - target).abs() > f64::EPSILON {
            self.start_value = self.current;
            self.target = target;
            self.start_time = Instant::now();
            self.duration_secs = duration;
        }
    }

    /// Return the current value, linearly interpolated along the active ramp.
    fn value(&mut self) -> f64 {
        if self.duration_secs <= 0.0 {
            self.current = self.target;
        } else {
            let t = (self.start_time.elapsed().as_secs_f64() / self.duration_secs).clamp(0.0, 1.0);
            self.current = self.start_value + (self.target - self.start_value) * t;
        }
        self.current
    }

    /// True when the ramp has reached its target.
    fn settled(&self) -> bool {
        self.duration_secs <= 0.0 || self.start_time.elapsed().as_secs_f64() >= self.duration_secs
    }
}

// ── Temperature model ────────────────────────────────────────────────────────

/// Simple xorshift64 PRNG — no external crate needed.
fn xorshift64(state: &mut u64) -> u64 {
    let mut s = *state;
    s ^= s << 13;
    s ^= s >> 7;
    s ^= s << 17;
    *state = s;
    s
}

/// Random load events that drive temperature spikes and cooldowns.
struct TempState {
    ramp: Ramp,
    rng: u64,
    idle_temp: f64,
}

/// Ambient / idle temperature (°C).
const IDLE_TEMP: f64 = 25.0;
/// Duration of every temperature ramp (seconds).
const TEMP_RAMP_SECS: f64 = 5.0;

impl TempState {
    fn new() -> Self {
        // Seed from Instant for some entropy.
        let seed = Instant::now().elapsed().as_nanos() as u64 | 1;
        Self {
            ramp: Ramp::new(IDLE_TEMP),
            rng: seed,
            idle_temp: IDLE_TEMP,
        }
    }

    /// Called each poll cycle.  When the current ramp has settled, randomly
    /// decide whether the system stays idle or gets a load spike.
    fn poll(&mut self) -> f64 {
        if self.ramp.settled() {
            let r = xorshift64(&mut self.rng);
            // 40% chance of a new load event, 60% chance of cooling back down.
            let load_event = (r % 100) < 40;

            if load_event {
                // Pick a random peak temperature between the thresholds.
                let kind = r % 10;
                let target = match kind {
                    0..=3 => 30.0 + (r % 800) as f64 / 100.0, // light load: 30–38 °C
                    4..=6 => 40.0 + (r % 400) as f64 / 100.0, // medium load: 40–44 °C
                    7..=8 => 44.0 + (r % 600) as f64 / 100.0, // heavy load: 44–50 °C
                    _ => 50.0 + (r % 300) as f64 / 100.0,     // critical: 50–53 °C
                };
                self.ramp.set_target(target, TEMP_RAMP_SECS);
            } else {
                // Cool back toward idle with slight jitter.
                let jitter = (r % 400) as f64 / 100.0; // 0–4 °C
                self.ramp.set_target(self.idle_temp + jitter, TEMP_RAMP_SECS);
            }
        }
        self.ramp.value()
    }
}

// ── Fan model (rewritten to use Ramp) ────────────────────────────────────────

type FanState = Ramp;

/// Battery charge/discharge simulation state.
struct BstState {
    /// 2 = charging, 1 = discharging
    state: u32,
    capacity: u32,
}

pub struct Mock {
    rtc: Mutex<MockRtc>,
    set_rpm: Mutex<Option<f64>>,
    current_temp_c: Mutex<f64>,
    temp_state: Mutex<TempState>,
    fan_state: Mutex<FanState>,
    bst_state: Mutex<BstState>,
    thresholds: Mutex<MockThresholds>,
}

#[derive(Copy, Clone)]
struct MockThresholds {
    on: f64,
    ramping: f64,
    max: f64,
}

impl Default for MockThresholds {
    fn default() -> Self {
        Self {
            on: 28.0,
            ramping: 40.0,
            max: 44.0,
        }
    }
}

impl Clone for Mock {
    fn clone(&self) -> Self {
        Self::new()
    }
}

impl Mock {
    pub fn new() -> Self {
        Self {
            rtc: Mutex::new(MockRtc::default()),
            set_rpm: Mutex::new(None),
            current_temp_c: Mutex::new(IDLE_TEMP),
            temp_state: Mutex::new(TempState::new()),
            fan_state: Mutex::new(Ramp::new(0.0)),
            bst_state: Mutex::new(BstState { state: 2, capacity: 0 }),
            thresholds: Mutex::new(MockThresholds::default()),
        }
    }
}

impl Default for Mock {
    fn default() -> Self {
        Self::new()
    }
}

impl ErrorType for Mock {
    type Error = Error;
}

impl ThermalSource for Mock {
    fn get_temperature(&self) -> Result<f64, Self::Error> {
        let mut state = self.temp_state.lock().unwrap();
        let temp_c = state.poll();

        // Publish for get_rpm() to read.
        *self.current_temp_c.lock().unwrap() = temp_c;
        Ok(temp_c)
    }

    fn get_rpm(&self) -> Result<f64, Self::Error> {
        // If the user explicitly set an RPM, honour it.
        if let Some(rpm) = *self.set_rpm.lock().unwrap() {
            return Ok(rpm);
        }

        let temp_c = *self.current_temp_c.lock().unwrap();
        let max_rpm = self.get_max_rpm()?;

        // Target RPM and ramp duration based on temperature thresholds.
        let (target, ramp_secs) = if temp_c >= 44.0 {
            (max_rpm, 5.0)
        } else if temp_c >= 40.0 {
            (3500.0, 5.0)
        } else if temp_c >= 28.0 {
            (1500.0, 2.0)
        } else {
            (0.0, 2.0)
        };

        let mut fan = self.fan_state.lock().unwrap();
        fan.set_target(target, ramp_secs);
        Ok(fan.value())
    }

    fn get_min_rpm(&self) -> Result<f64, Self::Error> {
        Ok(0.0)
    }

    fn get_max_rpm(&self) -> Result<f64, Self::Error> {
        Ok(6000.0)
    }

    fn get_threshold(&self, threshold: Threshold) -> Result<f64, Self::Error> {
        let t = *self.thresholds.lock().unwrap();
        Ok(match threshold {
            Threshold::On => t.on,
            Threshold::Ramping => t.ramping,
            Threshold::Max => t.max,
        })
    }

    fn set_threshold(&self, threshold: Threshold, value: f64) -> Result<(), Self::Error> {
        let mut t = self.thresholds.lock().unwrap();
        match threshold {
            Threshold::On => t.on = value,
            Threshold::Ramping => t.ramping = value,
            Threshold::Max => t.max = value,
        }
        Ok(())
    }

    fn set_rpm(&self, rpm: f64) -> Result<(), Self::Error> {
        *self.set_rpm.lock().unwrap() = Some(rpm);
        Ok(())
    }
}

impl BatterySource for Mock {
    fn get_bst(&self) -> Result<BstReturn, Self::Error> {
        const MAX_CAPACITY: u32 = 10000;
        const RATE: u32 = 1000;

        let mut guard = self.bst_state.lock().unwrap();
        let BstState { state, capacity } = &mut *guard;

        if *state == 2 {
            *capacity += RATE;
            if *capacity > MAX_CAPACITY {
                *capacity = MAX_CAPACITY;
                *state = 1;
            }
        } else {
            *capacity = capacity.saturating_sub(RATE);
            if *capacity < RATE {
                *state = 2;
            }
        }

        Ok(BstReturn {
            battery_state: BatteryState::from_bits(*state).ok_or(Error::InvalidData)?,
            battery_present_rate: 3839,
            battery_remaining_capacity: *capacity,
            battery_present_voltage: 12569,
        })
    }

    fn get_bix(&self) -> Result<BixFixedStrings, Self::Error> {
        Ok(BixFixedStrings {
            revision: 1,
            power_unit: PowerUnit::MilliWatts,
            design_capacity: 10000,
            last_full_charge_capacity: 9890,
            battery_technology: BatteryTechnology::Primary,
            design_voltage: 13000,
            design_cap_of_warning: 5000,
            design_cap_of_low: 3000,
            cycle_count: 1337,
            measurement_accuracy: 80000,
            max_sampling_time: 42,
            min_sampling_time: 7,
            max_averaging_interval: 5,
            min_averaging_interval: 1,
            battery_capacity_granularity_1: 10,
            battery_capacity_granularity_2: 10,
            model_number: [b'4', b'2', b'.', b'0', 0, 0, 0, 0],
            serial_number: [b'1', b'2', b'3', b'-', b'4', b'5', 0, 0],
            battery_type: [b'L', b'i', b'-', b'o', b'n', 0, 0, 0],
            oem_info: [b'B', b'a', b't', b'B', b'r', b'o', b's', 0],
            battery_swapping_capability: BatterySwapCapability::ColdSwappable,
        })
    }

    fn set_btp(&self, _trippoint: u32) -> Result<(), Self::Error> {
        // Do nothing for mock
        Ok(())
    }
}

#[derive(Copy, Clone)]
struct MockRtc {
    time: AcpiTimestamp,
    timers: [MockRtcTimer; 2],
}

#[derive(Copy, Clone)]
struct MockRtcTimer {
    value: AlarmTimerSeconds,
    wake_policy: AlarmExpiredWakePolicy,
    timer_status: TimerStatus,
}

impl Default for MockRtcTimer {
    fn default() -> Self {
        Self {
            value: AlarmTimerSeconds(0),
            wake_policy: AlarmExpiredWakePolicy::INSTANTLY,
            timer_status: TimerStatus(0),
        }
    }
}

impl MockRtc {
    fn new() -> Self {
        Self {
            time: AcpiTimestamp {
                datetime: Datetime::new(DatetimeFields {
                    year: 2026,
                    month: Month::January,
                    day: 1,
                    ..Default::default()
                })
                .expect("statically known valid datetime"),
                time_zone: AcpiTimeZone::MinutesFromUtc(
                    AcpiTimeZoneOffset::new(-8 * 60).expect("statically known valid timezone"),
                ),
                dst_status: AcpiDaylightSavingsTimeStatus::NotObserved,
            },
            timers: [MockRtcTimer::default(); 2],
        }
    }

    fn get_timer(&self, timer_id: AcpiTimerId) -> &MockRtcTimer {
        &self.timers[timer_id as usize]
    }
}

impl Default for MockRtc {
    fn default() -> Self {
        Self::new()
    }
}

impl RtcSource for Mock {
    fn get_capabilities(&self) -> Result<TimeAlarmDeviceCapabilities, Self::Error> {
        Ok(TimeAlarmDeviceCapabilities(0xF7))
    }

    fn get_real_time(&self) -> Result<AcpiTimestamp, Self::Error> {
        Ok(self.rtc.lock().unwrap().time)
    }

    fn get_wake_status(&self, timer_id: AcpiTimerId) -> Result<TimerStatus, Self::Error> {
        Ok(self.rtc.lock().unwrap().get_timer(timer_id).timer_status)
    }

    fn get_expired_timer_wake_policy(&self, timer_id: AcpiTimerId) -> Result<AlarmExpiredWakePolicy, Self::Error> {
        Ok(self.rtc.lock().unwrap().get_timer(timer_id).wake_policy)
    }

    fn get_timer_value(&self, timer_id: AcpiTimerId) -> Result<AlarmTimerSeconds, Self::Error> {
        Ok(self.rtc.lock().unwrap().get_timer(timer_id).value)
    }

    fn set_real_time(&self, timestamp: AcpiTimestamp) -> Result<(), Self::Error> {
        self.rtc.lock().unwrap().time = timestamp;
        Ok(())
    }

    fn set_timer_value(&self, timer_id: AcpiTimerId, value: AlarmTimerSeconds) -> Result<(), Self::Error> {
        self.rtc.lock().unwrap().timers[timer_id as usize].value = value;
        Ok(())
    }

    fn set_expired_timer_wake_policy(
        &self,
        timer_id: AcpiTimerId,
        policy: AlarmExpiredWakePolicy,
    ) -> Result<(), Self::Error> {
        self.rtc.lock().unwrap().timers[timer_id as usize].wake_policy = policy;
        Ok(())
    }

    fn clear_wake_status(&self, timer_id: AcpiTimerId) -> Result<(), Self::Error> {
        self.rtc.lock().unwrap().timers[timer_id as usize].timer_status = TimerStatus(0);
        Ok(())
    }
}
