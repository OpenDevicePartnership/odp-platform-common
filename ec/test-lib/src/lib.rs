// Multiple source features may be enabled simultaneously; the binary selects one at runtime.

use battery_service_messages::{BixFixedStrings, BstReturn};
use time_alarm_service_messages::{
    AcpiTimerId, AcpiTimestamp, AlarmExpiredWakePolicy, AlarmTimerSeconds, TimeAlarmDeviceCapabilities, TimerStatus,
};

pub(crate) mod common;

pub mod os;

pub mod mock;
pub mod serial;

/// EC data source error.
///
/// Custom error types should implement this trait to allow generic code
/// to extract a common [`ErrorKind`].
pub trait Error: std::error::Error + Send + Sync + 'static {
    /// Convert error to a generic EC error kind.
    fn kind(&self) -> ErrorKind;
}

impl Error for std::convert::Infallible {
    #[inline]
    fn kind(&self) -> ErrorKind {
        match *self {}
    }
}

/// EC data source error kind.
///
/// Represents a common set of errors. Implementations are free to define
/// more specific error types, but must map them to these kinds.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// Received an unexpected or malformed response from the device.
    UnexpectedResponse,
    /// Data validation failed (invalid enum discriminant, malformed field, etc.)
    InvalidData,
    /// A transport-level I/O error occurred.
    Io,
    /// A protocol framing error occurred.
    Protocol,
    /// Message serialization or deserialization failed.
    Serialization,
    /// A different error occurred.
    Other,
}

impl Error for ErrorKind {
    #[inline]
    fn kind(&self) -> ErrorKind {
        *self
    }
}

impl std::fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnexpectedResponse => write!(f, "Unexpected response"),
            Self::InvalidData => write!(f, "Invalid data"),
            Self::Io => write!(f, "I/O error"),
            Self::Protocol => write!(f, "Protocol error"),
            Self::Serialization => write!(f, "Serialization error"),
            Self::Other => write!(
                f,
                "A different error occurred. The original error may contain more information"
            ),
        }
    }
}

impl std::error::Error for ErrorKind {}

/// EC data source error type trait.
///
/// Defines the error type shared across all source trait methods.
pub trait ErrorType {
    /// Error type
    type Error: Error;
}

/// Trait for thermal data sources (temperature, fan RPM, thresholds).
pub trait ThermalSource: ErrorType {
    /// Get current temperature
    fn get_temperature(&self) -> Result<f64, Self::Error>;

    /// Get current fan RPM
    fn get_rpm(&self) -> Result<f64, Self::Error>;

    /// Get min fan RPM
    fn get_min_rpm(&self) -> Result<f64, Self::Error>;

    /// Get max fan RPM
    fn get_max_rpm(&self) -> Result<f64, Self::Error>;

    /// Get fan threshold
    fn get_threshold(&self, threshold: Threshold) -> Result<f64, Self::Error>;

    /// Set fan RPM limit
    fn set_rpm(&self, rpm: f64) -> Result<(), Self::Error>;
}

/// Trait for battery data sources (BST, BIX, BTP).
pub trait BatterySource: ErrorType {
    /// Get battery BST data
    fn get_bst(&self) -> Result<BstReturn, Self::Error>;

    /// Get battery BIX data
    fn get_bix(&self) -> Result<BixFixedStrings, Self::Error>;

    /// Set battery trippoint
    fn set_btp(&self, trippoint: u32) -> Result<(), Self::Error>;
}

/// Trait for RTC (Real-Time Clock) data sources.
pub trait RtcSource: ErrorType {
    /// Get RTC capabilities bitfield - see _GCP
    fn get_capabilities(&self) -> Result<TimeAlarmDeviceCapabilities, Self::Error>;

    /// Get RTC time as unix timestamp - see _GRT
    fn get_real_time(&self) -> Result<AcpiTimestamp, Self::Error>;

    /// Query the wake status of the timer - see _GWS
    fn get_wake_status(&self, timer_id: AcpiTimerId) -> Result<TimerStatus, Self::Error>;

    /// Get the expired timer wake policy - see _TIP
    fn get_expired_timer_wake_policy(&self, timer_id: AcpiTimerId) -> Result<AlarmExpiredWakePolicy, Self::Error>;

    /// Get the timer value - see _TIV
    fn get_timer_value(&self, timer_id: AcpiTimerId) -> Result<AlarmTimerSeconds, Self::Error>;
}

/// Marker trait implemented by all EC data sources.
pub trait Source: ThermalSource + BatterySource + RtcSource {}
impl<T: ThermalSource + BatterySource + RtcSource> Source for T {}

// Blanket impls so that Arc<S> can be used anywhere a source trait is required.
// This lets modules share one source instance via Arc instead of each owning a clone.
use std::sync::Arc;

impl<T: ErrorType> ErrorType for Arc<T> {
    type Error = T::Error;
}

impl<T: ThermalSource> ThermalSource for Arc<T> {
    fn get_temperature(&self) -> Result<f64, Self::Error> {
        self.as_ref().get_temperature()
    }
    fn get_rpm(&self) -> Result<f64, Self::Error> {
        self.as_ref().get_rpm()
    }
    fn get_min_rpm(&self) -> Result<f64, Self::Error> {
        self.as_ref().get_min_rpm()
    }
    fn get_max_rpm(&self) -> Result<f64, Self::Error> {
        self.as_ref().get_max_rpm()
    }
    fn get_threshold(&self, threshold: Threshold) -> Result<f64, Self::Error> {
        self.as_ref().get_threshold(threshold)
    }
    fn set_rpm(&self, rpm: f64) -> Result<(), Self::Error> {
        self.as_ref().set_rpm(rpm)
    }
}

impl<T: BatterySource> BatterySource for Arc<T> {
    fn get_bst(&self) -> Result<BstReturn, Self::Error> {
        self.as_ref().get_bst()
    }
    fn get_bix(&self) -> Result<BixFixedStrings, Self::Error> {
        self.as_ref().get_bix()
    }
    fn set_btp(&self, trippoint: u32) -> Result<(), Self::Error> {
        self.as_ref().set_btp(trippoint)
    }
}

impl<T: RtcSource> RtcSource for Arc<T> {
    fn get_capabilities(&self) -> Result<TimeAlarmDeviceCapabilities, Self::Error> {
        self.as_ref().get_capabilities()
    }
    fn get_real_time(&self) -> Result<AcpiTimestamp, Self::Error> {
        self.as_ref().get_real_time()
    }
    fn get_wake_status(&self, timer_id: AcpiTimerId) -> Result<TimerStatus, Self::Error> {
        self.as_ref().get_wake_status(timer_id)
    }
    fn get_expired_timer_wake_policy(&self, timer_id: AcpiTimerId) -> Result<AlarmExpiredWakePolicy, Self::Error> {
        self.as_ref().get_expired_timer_wake_policy(timer_id)
    }
    fn get_timer_value(&self, timer_id: AcpiTimerId) -> Result<AlarmTimerSeconds, Self::Error> {
        self.as_ref().get_timer_value(timer_id)
    }
}

/// Fan threshold type
pub enum Threshold {
    /// On threshold temperature
    On,
    /// Ramping threshold temperature
    Ramping,
    /// Max threshold temperature
    Max,
}
