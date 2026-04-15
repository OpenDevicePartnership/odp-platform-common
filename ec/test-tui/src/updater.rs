use std::sync::{Arc, RwLock, mpsc};
use std::time::{Duration, Instant};

use ec_test_lib::{Source, Threshold};
use time_alarm_service_messages::AcpiTimerId;
use tracing::{debug, info, trace, warn};

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
        info!(
            interval_secs = graph_sample_interval.as_secs_f64(),
            "updater created"
        );
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

    #[tracing::instrument(skip_all)]
    fn process_commands(&mut self) {
        while let Ok(cmd) = self.battery_rx.try_recv() {
            let BatteryCommand::SetBtp(v) = cmd;
            debug!(btp = v, "processing SetBtp command");
            let success = self.source.set_btp(v).is_ok();
            if success {
                info!(btp = v, "battery trip-point set successfully");
            } else {
                warn!(btp = v, "failed to set battery trip-point on hardware");
            }
            if let Ok(mut s) = self.state.write() {
                s.battery.btp = v;
                s.battery.btp_success = success;
            }
        }
        while let Ok(cmd) = self.thermal_rx.try_recv() {
            let ThermalCommand::SetRpm(rpm) = cmd;
            debug!(rpm, "processing SetRpm command");
            if self.source.set_rpm(rpm).is_err() {
                warn!(rpm, "failed to set fan RPM on hardware");
            }
        }
    }

    // ── Per-subsystem update helpers ──────────────────────────────────────────

    #[tracing::instrument(skip_all)]
    fn update_battery(&mut self) {
        let now = Instant::now();
        let update_graph = self
            .last_graph_update
            .is_none_or(|t| now.duration_since(t) >= self.graph_sample_interval);

        let mut s = self.state.write().expect("state RwLock poisoned");

        // BIX is static — only fetch until we get one good read.
        if !self.bix_cached {
            poll_bix(&mut s.battery, self.source.as_ref());
            if s.battery.bix_success {
                info!("BIX static battery info cached successfully");
            }
            self.bix_cached = s.battery.bix_success;
        }

        poll_bst(&mut s.battery, self.source.as_ref());

        if update_graph && s.battery.bst_success {
            let cap = s.battery.bst.battery_remaining_capacity;
            trace!(
                remaining_capacity = cap,
                voltage_mv = s.battery.bst.battery_present_voltage,
                "battery graph sample recorded"
            );
            s.battery.samples.insert(cap);
            s.battery.t_min += 1;
        }

        drop(s);

        if update_graph {
            self.last_graph_update = Some(now);
        }
    }

    #[tracing::instrument(skip_all)]
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
                trace!(skin_temp = t, "temperature read OK");
                s.thermal.sensor.skin_temp = t;
                s.thermal.sensor.samples.insert(t);
                s.thermal.sensor.temp_success = true;
            }
            Err(e) => {
                warn!(error = %e, "failed to read skin temperature");
                s.thermal.sensor.temp_success = false;
            }
        }
        // Thresholds are hardcoded for now (see thermal.rs).
        s.thermal.sensor.thresholds = crate::thermal::sensor_thresholds();
        s.thermal.sensor.thresholds_success = true;

        match rpm {
            Ok(r) => {
                trace!(rpm = r, "fan RPM read OK");
                s.thermal.fan.rpm = r;
                s.thermal.fan.samples.insert(r as u32);
                s.thermal.fan.rpm_success = true;
            }
            Err(e) => {
                warn!(error = %e, "failed to read fan RPM");
                s.thermal.fan.rpm_success = false;
            }
        }
        match (min_rpm, max_rpm) {
            (Ok(min), Ok(max)) => {
                s.thermal.fan.rpm_bounds = FanRpmBounds { min, max };
                s.thermal.fan.bounds_success = true;
            }
            (Err(e), _) | (_, Err(e)) => {
                warn!(error = %e, "failed to read fan RPM bounds");
                s.thermal.fan.bounds_success = false;
            }
        }
        match (thresh_on, thresh_ramp, thresh_max) {
            (Ok(on), Ok(ramping), Ok(max)) => {
                s.thermal.fan.state_levels = FanStateLevels { on, ramping, max };
                s.thermal.fan.levels_success = true;
            }
            (Err(e), ..) => {
                warn!(error = %e, "failed to read fan state levels");
                s.thermal.fan.levels_success = false;
            }
            (_, Err(e), _) | (_, _, Err(e)) => {
                warn!(error = %e, "failed to read fan state levels");
                s.thermal.fan.levels_success = false;
            }
        }

        s.thermal.t += 1;
    }

    #[tracing::instrument(skip_all)]
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
            if ok {
                info!("RTC capabilities cached successfully");
            } else if let Err(ref e) = c {
                warn!(error = %e, "failed to read RTC capabilities");
            }
            s.rtc.capabilities = Some(c.map_err(Into::into));
            if ok {
                self.rtc_caps_cached = true;
            }
        }

        match &timestamp {
            Ok(_) => trace!("RTC timestamp read OK"),
            Err(e) => warn!(error = %e, "failed to read RTC timestamp"),
        }
        s.rtc.timestamp = Some(timestamp.map_err(Into::into));

        if let Err(ref e) = ac_value {
            warn!(error = %e, "failed to read AC power timer value");
        }
        if let Err(ref e) = dc_value {
            warn!(error = %e, "failed to read DC power timer value");
        }

        s.rtc.timers[0].value = Some(ac_value.map_err(Into::into));
        s.rtc.timers[0].wake_policy = Some(ac_policy.map_err(Into::into));
        s.rtc.timers[0].timer_status = Some(ac_status.map_err(Into::into));
        s.rtc.timers[1].value = Some(dc_value.map_err(Into::into));
        s.rtc.timers[1].wake_policy = Some(dc_policy.map_err(Into::into));
        s.rtc.timers[1].timer_status = Some(dc_status.map_err(Into::into));
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Drain pending commands and refresh all subsystems once.
    #[tracing::instrument(skip_all)]
    pub fn update(&mut self) {
        debug!("update cycle start");
        self.process_commands();
        self.update_battery();
        self.update_thermal();
        self.update_rtc();
    }

    /// Perform an initial fetch, then loop forever sleeping `interval` between
    /// updates.  Intended to be called as a [`tokio::task::spawn`] task.
    pub async fn run(mut self, interval: Duration) {
        info!(interval_ms = interval.as_millis(), "updater task started");
        self.update();
        loop {
            tokio::time::sleep(interval).await;
            self.update();
        }
    }
}
