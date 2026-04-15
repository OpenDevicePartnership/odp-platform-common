use std::sync::{Arc, RwLock, mpsc};
use std::time::{Duration, Instant};

use ec_test_lib::{Source, Threshold};
use time_alarm_service_messages::AcpiTimerId;
use tracing::{debug, info, trace, warn};

use crate::battery::{poll_bix, poll_bst};
use crate::state::{AppState, BatteryCommand, FanRpmBounds, FanStateLevels, ThermalCommand};

// ── Battery ───────────────────────────────────────────────────────────────────

/// Polls BST/BIX and records a graph sample on every tick.
/// Intended to run every 30 s.
pub struct BatteryUpdater<S: Source> {
    source: Arc<S>,
    state: Arc<RwLock<AppState>>,
    battery_rx: mpsc::Receiver<BatteryCommand>,
    bix_cached: bool,
}

impl<S: Source + Send + Sync + 'static> BatteryUpdater<S> {
    pub fn new(source: Arc<S>, state: Arc<RwLock<AppState>>, battery_rx: mpsc::Receiver<BatteryCommand>) -> Self {
        Self {
            source,
            state,
            battery_rx,
            bix_cached: false,
        }
    }

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
    }

    #[tracing::instrument(skip_all)]
    fn update(&mut self) {
        self.process_commands();

        let mut s = self.state.write().expect("state RwLock poisoned");

        if !self.bix_cached {
            poll_bix(&mut s.battery, self.source.as_ref());
            if s.battery.bix_success {
                info!("BIX static battery info cached successfully");
            }
            self.bix_cached = s.battery.bix_success;
        }

        poll_bst(&mut s.battery, self.source.as_ref());

        if s.battery.bst_success {
            let cap = s.battery.bst.battery_remaining_capacity;
            trace!(
                remaining_capacity = cap,
                voltage_mv = s.battery.bst.battery_present_voltage,
                "battery graph sample recorded"
            );
            s.battery.samples.insert(cap);
            s.battery.t_min += 1;
        }
    }

    /// Run forever, updating once immediately then sleeping `interval` between
    /// ticks.  Spawn as a [`tokio::task::spawn`] task.
    pub async fn run(mut self, interval: Duration) {
        info!(interval_ms = interval.as_millis(), "battery updater started");
        self.update();
        loop {
            tokio::time::sleep(interval).await;
            self.update();
        }
    }
}

// ── Thermal ───────────────────────────────────────────────────────────────────

/// Polls temperature and fan metrics on every tick.
/// Intended to run every 5 s.
pub struct ThermalUpdater<S: Source> {
    source: Arc<S>,
    state: Arc<RwLock<AppState>>,
    thermal_rx: mpsc::Receiver<ThermalCommand>,
}

impl<S: Source + Send + Sync + 'static> ThermalUpdater<S> {
    pub fn new(source: Arc<S>, state: Arc<RwLock<AppState>>, thermal_rx: mpsc::Receiver<ThermalCommand>) -> Self {
        Self {
            source,
            state,
            thermal_rx,
        }
    }

    fn process_commands(&mut self) {
        while let Ok(cmd) = self.thermal_rx.try_recv() {
            let ThermalCommand::SetRpm(rpm) = cmd;
            debug!(rpm, "processing SetRpm command");
            if self.source.set_rpm(rpm).is_err() {
                warn!(rpm, "failed to set fan RPM on hardware");
            }
        }
    }

    #[tracing::instrument(skip_all)]
    fn update(&mut self) {
        self.process_commands();

        // Fetch before acquiring the write lock to minimise lock hold time.
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

    pub async fn run(mut self, interval: Duration) {
        info!(interval_ms = interval.as_millis(), "thermal updater started");
        self.update();
        loop {
            tokio::time::sleep(interval).await;
            self.update();
        }
    }
}

// ── RTC ───────────────────────────────────────────────────────────────────────

/// Polls RTC time, capabilities, and timer state on every tick.
/// Intended to run every 1 s.
pub struct RtcUpdater<S: Source> {
    source: Arc<S>,
    state: Arc<RwLock<AppState>>,
    rtc_caps_cached: bool,
}

impl<S: Source + Send + Sync + 'static> RtcUpdater<S> {
    pub fn new(source: Arc<S>, state: Arc<RwLock<AppState>>) -> Self {
        Self {
            source,
            state,
            rtc_caps_cached: false,
        }
    }

    #[tracing::instrument(skip_all)]
    fn update(&mut self) {
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

    pub async fn run(mut self, interval: Duration) {
        info!(interval_ms = interval.as_millis(), "RTC updater started");
        self.update();
        loop {
            tokio::time::sleep(interval).await;
            self.update();
        }
    }
}

// ── System ────────────────────────────────────────────────────────────────────

/// Polls OS-level CPU, memory, and network metrics on every tick.
/// Intended to run every 500 ms.
pub struct SystemUpdater {
    state: Arc<RwLock<AppState>>,
    sys_info: sysinfo::System,
    sys_nets: sysinfo::Networks,
    last_update: Option<Instant>,
}

impl SystemUpdater {
    pub fn new(state: Arc<RwLock<AppState>>) -> Self {
        info!("initialising sysinfo");
        Self {
            state,
            sys_info: sysinfo::System::new_all(),
            sys_nets: sysinfo::Networks::new_with_refreshed_list(),
            last_update: None,
        }
    }

    #[tracing::instrument(skip_all)]
    fn update(&mut self) {
        let now = Instant::now();
        let elapsed_secs = self
            .last_update
            .map(|t| now.duration_since(t).as_secs_f64())
            .unwrap_or(1.0)
            .max(0.001);
        self.last_update = Some(now);

        self.sys_info.refresh_cpu_all();
        self.sys_info.refresh_memory();
        self.sys_nets.refresh(true);

        let mut s = self.state.write().expect("state RwLock poisoned");

        // ── CPU ───────────────────────────────────────────────────────────────
        let usage = self.sys_info.global_cpu_usage() as f64;
        s.system.cpu.usage = usage;
        s.system.cpu.per_core = self.sys_info.cpus().iter().map(|c| c.cpu_usage()).collect();
        s.system.cpu.samples.insert(usage);
        s.system.cpu.success = true;
        trace!(usage, "CPU usage sampled");

        // ── Memory ────────────────────────────────────────────────────────────
        let total = self.sys_info.total_memory();
        let used = self.sys_info.used_memory();
        s.system.memory.total_bytes = total;
        s.system.memory.used_bytes = used;
        s.system.memory.swap_total_bytes = self.sys_info.total_swap();
        s.system.memory.swap_used_bytes = self.sys_info.used_swap();
        let mem_pct = if total > 0 {
            used as f64 / total as f64 * 100.0
        } else {
            0.0
        };
        s.system.memory.samples.insert(mem_pct);
        s.system.memory.success = true;
        trace!(used, total, "memory sampled");

        // ── Network ───────────────────────────────────────────────────────────
        let (total_rx, total_tx, delta_rx, delta_tx) =
            self.sys_nets.iter().fold((0u64, 0u64, 0u64, 0u64), |acc, (_, d)| {
                (
                    acc.0 + d.total_received(),
                    acc.1 + d.total_transmitted(),
                    acc.2 + d.received(),
                    acc.3 + d.transmitted(),
                )
            });
        let rx_bps = delta_rx as f64 / elapsed_secs;
        let tx_bps = delta_tx as f64 / elapsed_secs;
        s.system.network.rx_bps = rx_bps;
        s.system.network.tx_bps = tx_bps;
        s.system.network.total_rx = total_rx;
        s.system.network.total_tx = total_tx;
        s.system.network.rx_samples.insert(rx_bps);
        s.system.network.tx_samples.insert(tx_bps);
        s.system.network.success = true;
        trace!(rx_bps, tx_bps, "network sampled");

        s.system.t += 1;
    }

    pub async fn run(mut self, interval: Duration) {
        info!(interval_ms = interval.as_millis(), "system updater started");
        self.update();
        loop {
            tokio::time::sleep(interval).await;
            self.update();
        }
    }
}
