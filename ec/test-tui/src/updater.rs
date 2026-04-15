use std::sync::{Arc, RwLock, mpsc};
use std::time::{Duration, Instant};

use ec_test_lib::{Source, Threshold};
use time_alarm_service_messages::AcpiTimerId;

use crate::battery::{poll_bix, poll_bst};
use crate::state::{AppState, BatteryCommand, FanRpmBounds, FanStateLevels, ThermalCommand};

/// Background updater — owns the data source and periodically refreshes
/// the shared [`AppState`] so the UI thread only has to render.
///
/// Runs on a dedicated OS thread via [`std::thread::spawn`].
pub struct Updater<S: Source> {
    source: Arc<S>,
    state: Arc<RwLock<AppState>>,
    battery_rx: mpsc::Receiver<BatteryCommand>,
    thermal_rx: mpsc::Receiver<ThermalCommand>,
    graph_sample_interval: Duration,
    last_graph_update: Option<Instant>,
    /// True once BIX (static battery info) has been fetched successfully.
    bix_cached: bool,
    /// True once RTC capabilities (static) have been fetched successfully.
    rtc_caps_cached: bool,
}

impl<S: Source + Send + 'static> Updater<S> {
    pub fn new(
        source: Arc<S>,
        state: Arc<RwLock<AppState>>,
        battery_rx: mpsc::Receiver<BatteryCommand>,
        thermal_rx: mpsc::Receiver<ThermalCommand>,
        graph_sample_interval: Duration,
    ) -> Self {
        Self {
            source,
            state,
            battery_rx,
            thermal_rx,
            graph_sample_interval,
            last_graph_update: None,
            bix_cached: false,
            rtc_caps_cached: false,
        }
    }

    // ── Command processing ────────────────────────────────────────────────────

    fn process_commands(&mut self) {
        while let Ok(cmd) = self.battery_rx.try_recv() {
            let BatteryCommand::SetBtp(v) = cmd;
            let success = self.source.set_btp(v).is_ok();
            if let Ok(mut s) = self.state.write() {
                s.battery.btp = v;
                s.battery.btp_success = success;
            }
        }
        while let Ok(cmd) = self.thermal_rx.try_recv() {
            let ThermalCommand::SetRpm(rpm) = cmd;
            let _ = self.source.set_rpm(rpm);
        }
    }

    // ── Per-subsystem update helpers ──────────────────────────────────────────

    fn update_battery(&mut self) {
        let now = Instant::now();
        let update_graph = self
            .last_graph_update
            .is_none_or(|t| now.duration_since(t) >= self.graph_sample_interval);

        let mut s = self.state.write().expect("state RwLock poisoned");

        // BIX is static — only fetch until we get one good read.
        if !self.bix_cached {
            poll_bix(&mut s.battery, self.source.as_ref());
            self.bix_cached = s.battery.bix_success;
        }

        poll_bst(&mut s.battery, self.source.as_ref());

        if update_graph && s.battery.bst_success {
            let cap = s.battery.bst.battery_remaining_capacity;
            s.battery.samples.insert(cap);
            s.battery.t_min += 1;
        }

        drop(s);

        if update_graph {
            self.last_graph_update = Some(now);
        }
    }

    fn update_thermal(&mut self) {
        // Fetch all thermal readings before acquiring the write lock so
        // we hold the lock only for the short write phase.
        let temp = self.source.get_temperature();
        let rpm = self.source.get_rpm();
        let min_rpm = self.source.get_min_rpm();
        let max_rpm = self.source.get_max_rpm();
        let thresh_on = self.source.get_threshold(Threshold::On);
        let thresh_ramp = self.source.get_threshold(Threshold::Ramping);
        let thresh_max = self.source.get_threshold(Threshold::Max);

        let mut s = self.state.write().expect("state RwLock poisoned");

        match temp {
            Ok(t) => {
                s.thermal.sensor.skin_temp = t;
                s.thermal.sensor.samples.insert(t);
                s.thermal.sensor.temp_success = true;
            }
            Err(_) => s.thermal.sensor.temp_success = false,
        }
        // Thresholds are hardcoded for now (see thermal.rs).
        s.thermal.sensor.thresholds = crate::thermal::sensor_thresholds();
        s.thermal.sensor.thresholds_success = true;

        match rpm {
            Ok(r) => {
                s.thermal.fan.rpm = r;
                s.thermal.fan.samples.insert(r as u32);
                s.thermal.fan.rpm_success = true;
            }
            Err(_) => s.thermal.fan.rpm_success = false,
        }
        match (min_rpm, max_rpm) {
            (Ok(min), Ok(max)) => {
                s.thermal.fan.rpm_bounds = FanRpmBounds { min, max };
                s.thermal.fan.bounds_success = true;
            }
            _ => s.thermal.fan.bounds_success = false,
        }
        match (thresh_on, thresh_ramp, thresh_max) {
            (Ok(on), Ok(ramping), Ok(max)) => {
                s.thermal.fan.state_levels = FanStateLevels { on, ramping, max };
                s.thermal.fan.levels_success = true;
            }
            _ => s.thermal.fan.levels_success = false,
        }

        s.thermal.t += 1;
    }

    fn update_rtc(&mut self) {
        // Capabilities are static — only fetch until we get a good read.
        let caps = if self.rtc_caps_cached {
            None
        } else {
            Some(self.source.get_capabilities())
        };
        let timestamp = self.source.get_real_time();
        let ac_value = self.source.get_timer_value(AcpiTimerId::AcPower);
        let ac_policy = self.source.get_expired_timer_wake_policy(AcpiTimerId::AcPower);
        let ac_status = self.source.get_wake_status(AcpiTimerId::AcPower);
        let dc_value = self.source.get_timer_value(AcpiTimerId::DcPower);
        let dc_policy = self.source.get_expired_timer_wake_policy(AcpiTimerId::DcPower);
        let dc_status = self.source.get_wake_status(AcpiTimerId::DcPower);

        let mut s = self.state.write().expect("state RwLock poisoned");

        if let Some(c) = caps {
            let ok = c.is_ok();
            s.rtc.capabilities = Some(c.map_err(Into::into));
            if ok {
                self.rtc_caps_cached = true;
            }
        }
        s.rtc.timestamp = Some(timestamp.map_err(Into::into));
        s.rtc.timers[0].value = Some(ac_value.map_err(Into::into));
        s.rtc.timers[0].wake_policy = Some(ac_policy.map_err(Into::into));
        s.rtc.timers[0].timer_status = Some(ac_status.map_err(Into::into));
        s.rtc.timers[1].value = Some(dc_value.map_err(Into::into));
        s.rtc.timers[1].wake_policy = Some(dc_policy.map_err(Into::into));
        s.rtc.timers[1].timer_status = Some(dc_status.map_err(Into::into));
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Drain pending commands and refresh all subsystems once.
    pub fn update(&mut self) {
        self.process_commands();
        self.update_battery();
        self.update_thermal();
        self.update_rtc();
    }

    /// Perform an initial fetch, then loop forever sleeping `interval` between
    /// updates.  Intended to be called from a dedicated [`std::thread::spawn`].
    pub fn run(mut self, interval: Duration) {
        self.update();
        loop {
            std::thread::sleep(interval);
            self.update();
        }
    }
}
