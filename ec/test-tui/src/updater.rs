use std::sync::{Arc, RwLock, mpsc};
use std::time::{Duration, Instant};

use ec_test_lib::Threshold;
use time_alarm_service_interface::AcpiTimerId;
use tracing::{debug, info, trace, warn};

use crate::battery::{poll_bix, poll_bst};
use crate::source::DynSource;
use crate::state::{
    BatteryCommand, BatteryState, FanRpmBounds, FanStateLevels, RtcState, SystemState, ThermalCommand, ThermalState,
};

// ── Battery ───────────────────────────────────────────────────────────────────

/// Polls BST/BIX and records a graph sample on every tick.
pub struct BatteryUpdater {
    source: Arc<dyn DynSource>,
    state: Arc<RwLock<BatteryState>>,
    battery_rx: mpsc::Receiver<BatteryCommand>,
    bix_cached: bool,
}

impl BatteryUpdater {
    pub fn new(
        source: Arc<dyn DynSource>,
        state: Arc<RwLock<BatteryState>>,
        battery_rx: mpsc::Receiver<BatteryCommand>,
    ) -> Self {
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
                s.btp = v;
                s.btp_success = success;
            }
        }
    }

    #[tracing::instrument(skip_all)]
    fn update(&mut self) {
        self.process_commands();

        let mut s = self.state.write().expect("state RwLock poisoned");

        if !self.bix_cached {
            poll_bix(&mut s, self.source.as_ref());
            if s.bix_success {
                info!("BIX static battery info cached successfully");
            }
            self.bix_cached = s.bix_success;
        }

        poll_bst(&mut s, self.source.as_ref());

        if s.bst_success {
            let cap = s.bst.battery_remaining_capacity;
            trace!(
                remaining_capacity = cap,
                voltage_mv = s.bst.battery_present_voltage,
                "battery graph sample recorded"
            );
            s.samples.insert(cap);
            s.t_min += 1;
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
pub struct ThermalUpdater {
    source: Arc<dyn DynSource>,
    state: Arc<RwLock<ThermalState>>,
    thermal_rx: mpsc::Receiver<ThermalCommand>,
}

impl ThermalUpdater {
    pub fn new(
        source: Arc<dyn DynSource>,
        state: Arc<RwLock<ThermalState>>,
        thermal_rx: mpsc::Receiver<ThermalCommand>,
    ) -> Self {
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
            let success = self.source.set_rpm(rpm).is_ok();
            if success {
                info!(rpm, "fan RPM limit set successfully");
            } else {
                warn!(rpm, "failed to set fan RPM on hardware");
            }
            if let Ok(mut s) = self.state.write() {
                s.rpm_limit = rpm;
                s.rpm_set_success = success;
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
                s.sensor.skin_temp = t;
                s.sensor.samples.insert(t);
                s.sensor.temp_success = true;
            }
            Err(e) => {
                warn!(error = %e, "failed to read skin temperature");
                s.sensor.temp_success = false;
            }
        }
        s.sensor.thresholds = crate::thermal::sensor_thresholds();
        s.sensor.thresholds_success = true;

        match rpm {
            Ok(r) => {
                trace!(rpm = r, "fan RPM read OK");
                s.fan.rpm = r;
                s.fan.samples.insert(r as u32);
                s.fan.rpm_success = true;
            }
            Err(e) => {
                warn!(error = %e, "failed to read fan RPM");
                s.fan.rpm_success = false;
            }
        }
        match (min_rpm, max_rpm) {
            (Ok(min), Ok(max)) => {
                s.fan.rpm_bounds = FanRpmBounds { min, max };
                s.fan.bounds_success = true;
            }
            (Err(e), _) | (_, Err(e)) => {
                warn!(error = %e, "failed to read fan RPM bounds");
                s.fan.bounds_success = false;
            }
        }
        match (thresh_on, thresh_ramp, thresh_max) {
            (Ok(on), Ok(ramping), Ok(max)) => {
                s.fan.state_levels = FanStateLevels { on, ramping, max };
                s.fan.levels_success = true;
            }
            (Err(e), ..) => {
                warn!(error = %e, "failed to read fan state levels");
                s.fan.levels_success = false;
            }
            (_, Err(e), _) | (_, _, Err(e)) => {
                warn!(error = %e, "failed to read fan state levels");
                s.fan.levels_success = false;
            }
        }

        s.t += 1;
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
pub struct RtcUpdater {
    source: Arc<dyn DynSource>,
    state: Arc<RwLock<RtcState>>,
    rtc_caps_cached: bool,
}

impl RtcUpdater {
    pub fn new(source: Arc<dyn DynSource>, state: Arc<RwLock<RtcState>>) -> Self {
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
            s.capabilities = Some(c);
            if ok {
                self.rtc_caps_cached = true;
            }
        }

        match &timestamp {
            Ok(_) => trace!("RTC timestamp read OK"),
            Err(e) => warn!(error = %e, "failed to read RTC timestamp"),
        }
        s.timestamp = Some(timestamp);

        if let Err(ref e) = ac_value {
            warn!(error = %e, "failed to read AC power timer value");
        }
        if let Err(ref e) = dc_value {
            warn!(error = %e, "failed to read DC power timer value");
        }

        s.timers[0].value = Some(ac_value);
        s.timers[0].wake_policy = Some(ac_policy);
        s.timers[0].timer_status = Some(ac_status);
        s.timers[1].value = Some(dc_value);
        s.timers[1].wake_policy = Some(dc_policy);
        s.timers[1].timer_status = Some(dc_status);
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
    state: Arc<RwLock<SystemState>>,
    sys_info: sysinfo::System,
    sys_nets: sysinfo::Networks,
    last_update: Option<Instant>,
}

impl SystemUpdater {
    pub fn new(state: Arc<RwLock<SystemState>>) -> Self {
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
        s.cpu.usage = usage;
        s.cpu.per_core = self.sys_info.cpus().iter().map(|c| c.cpu_usage()).collect();
        s.cpu.samples.insert(usage);
        s.cpu.success = true;
        trace!(usage, "CPU usage sampled");

        // ── Memory ────────────────────────────────────────────────────────────
        let total = self.sys_info.total_memory();
        let used = self.sys_info.used_memory();
        s.memory.total_bytes = total;
        s.memory.used_bytes = used;
        s.memory.swap_total_bytes = self.sys_info.total_swap();
        s.memory.swap_used_bytes = self.sys_info.used_swap();
        let mem_pct = if total > 0 {
            used as f64 / total as f64 * 100.0
        } else {
            0.0
        };
        s.memory.samples.insert(mem_pct);
        s.memory.success = true;
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
        s.network.rx_bps = rx_bps;
        s.network.tx_bps = tx_bps;
        s.network.total_rx = total_rx;
        s.network.total_tx = total_tx;
        s.network.rx_samples.insert(rx_bps);
        s.network.tx_samples.insert(tx_bps);
        s.network.success = true;
        trace!(rx_bps, tx_bps, "network sampled");

        s.t += 1;
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
