//! Object-safe wrapper around [`ec_test_lib::Source`].
//!
//! The library's source traits use an associated `Error` type which prevents
//! direct use of `dyn Source`.  This module defines [`DynSource`] — an
//! equivalent trait whose methods return `color_eyre::Result<T>` — and
//! provides a blanket implementation for every concrete `Source`.
//!
//! A single [`build`] factory resolves the CLI-selected backend into an
//! `Arc<dyn DynSource>` so the rest of the application is fully
//! source-agnostic with zero generics.

use std::sync::Arc;

use battery_service_interface::{BixFixedStrings, BstReturn};
use color_eyre::Result;
use color_eyre::eyre::eyre;
use ec_test_lib::Threshold;
use ec_test_lib::ucsi::UcsiSnapshot;
use time_alarm_service_interface::{
    AcpiTimerId, AcpiTimestamp, AlarmExpiredWakePolicy, AlarmTimerSeconds, TimeAlarmDeviceCapabilities, TimerStatus,
};

// ── Object-safe trait ────────────────────────────────────────────────────────

/// A type-erased EC data source.  All fallible methods return
/// [`color_eyre::Result`] so callers never need to know the concrete error
/// type.
pub(crate) trait DynSource: Send + Sync {
    // Battery
    fn get_bst(&self) -> Result<BstReturn>;
    fn get_bix(&self) -> Result<BixFixedStrings>;
    fn set_btp(&self, trippoint: u32) -> Result<()>;

    // Thermal
    fn get_temperature(&self) -> Result<f64>;
    fn get_rpm(&self) -> Result<f64>;
    fn get_min_rpm(&self) -> Result<f64>;
    fn get_max_rpm(&self) -> Result<f64>;
    fn get_threshold(&self, threshold: Threshold) -> Result<f64>;
    fn set_rpm(&self, rpm: f64) -> Result<()>;

    // RTC
    fn get_capabilities(&self) -> Result<TimeAlarmDeviceCapabilities>;
    fn get_real_time(&self) -> Result<AcpiTimestamp>;
    fn get_wake_status(&self, timer_id: AcpiTimerId) -> Result<TimerStatus>;
    fn get_expired_timer_wake_policy(&self, timer_id: AcpiTimerId) -> Result<AlarmExpiredWakePolicy>;
    fn get_timer_value(&self, timer_id: AcpiTimerId) -> Result<AlarmTimerSeconds>;

    // UCSI — default to unsupported so lightweight UI test doubles need not
    // implement it; real sources override it via the blanket impl below.
    fn get_ucsi_snapshot(&self, _connector: u8) -> Result<UcsiSnapshot> {
        Err(eyre!("UCSI not supported by this source"))
    }
}

// ── Blanket impl ─────────────────────────────────────────────────────────────

impl<T> DynSource for T
where
    T: ec_test_lib::Source + ec_test_lib::UcsiSource + Send + Sync,
    T::Error: Send + Sync + 'static,
{
    fn get_bst(&self) -> Result<BstReturn> {
        ec_test_lib::BatterySource::get_bst(self).map_err(Into::into)
    }
    fn get_bix(&self) -> Result<BixFixedStrings> {
        ec_test_lib::BatterySource::get_bix(self).map_err(Into::into)
    }
    fn set_btp(&self, trippoint: u32) -> Result<()> {
        ec_test_lib::BatterySource::set_btp(self, trippoint).map_err(Into::into)
    }

    fn get_temperature(&self) -> Result<f64> {
        ec_test_lib::ThermalSource::get_temperature(self).map_err(Into::into)
    }
    fn get_rpm(&self) -> Result<f64> {
        ec_test_lib::ThermalSource::get_rpm(self).map_err(Into::into)
    }
    fn get_min_rpm(&self) -> Result<f64> {
        ec_test_lib::ThermalSource::get_min_rpm(self).map_err(Into::into)
    }
    fn get_max_rpm(&self) -> Result<f64> {
        ec_test_lib::ThermalSource::get_max_rpm(self).map_err(Into::into)
    }
    fn get_threshold(&self, threshold: Threshold) -> Result<f64> {
        ec_test_lib::ThermalSource::get_threshold(self, threshold).map_err(Into::into)
    }
    fn set_rpm(&self, rpm: f64) -> Result<()> {
        ec_test_lib::ThermalSource::set_rpm(self, rpm).map_err(Into::into)
    }

    fn get_capabilities(&self) -> Result<TimeAlarmDeviceCapabilities> {
        ec_test_lib::RtcSource::get_capabilities(self).map_err(Into::into)
    }
    fn get_real_time(&self) -> Result<AcpiTimestamp> {
        ec_test_lib::RtcSource::get_real_time(self).map_err(Into::into)
    }
    fn get_wake_status(&self, timer_id: AcpiTimerId) -> Result<TimerStatus> {
        ec_test_lib::RtcSource::get_wake_status(self, timer_id).map_err(Into::into)
    }
    fn get_expired_timer_wake_policy(&self, timer_id: AcpiTimerId) -> Result<AlarmExpiredWakePolicy> {
        ec_test_lib::RtcSource::get_expired_timer_wake_policy(self, timer_id).map_err(Into::into)
    }
    fn get_timer_value(&self, timer_id: AcpiTimerId) -> Result<AlarmTimerSeconds> {
        ec_test_lib::RtcSource::get_timer_value(self, timer_id).map_err(Into::into)
    }

    fn get_ucsi_snapshot(&self, connector: u8) -> Result<UcsiSnapshot> {
        ec_test_lib::UcsiSource::get_snapshot(self, connector).map_err(Into::into)
    }
}

// ── Factory ──────────────────────────────────────────────────────────────────

use crate::{Cli, FlowControl, SourceKind};

/// Resolve the CLI-selected backend into a type-erased source.
pub(crate) fn build(cli: &Cli) -> Result<Arc<dyn DynSource>> {
    match cli.source {
        SourceKind::Mock => Ok(Arc::new(ec_test_lib::mock::Mock::default())),

        SourceKind::Serial => {
            let port = cli.port.as_deref().expect("--port is required for --source serial");
            let hw_flow = matches!(cli.flow_control, FlowControl::Hardware);
            let source =
                ec_test_lib::serial::Serial::new(port, cli.baud, hw_flow, cli.sensor_instance, cli.fan_instance)?;
            Ok(Arc::new(source))
        }

        #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
        SourceKind::Acpi => Ok(Arc::new(ec_test_lib::acpi::Acpi::new(cli.fan_instance))),

        #[cfg(target_os = "windows")]
        SourceKind::Windows => Ok(Arc::new(ec_test_lib::windows::Windows::new()?)),
    }
}
