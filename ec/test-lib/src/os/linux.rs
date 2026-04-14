use crate::{BatterySource, ErrorType, RtcSource, ThermalSource, Threshold};
use battery_service_messages::{
    BatteryState, BixFixedStrings, BstReturn, bat_swap_try_from_u32, bat_tech_try_from_u32, power_unit_try_from_u32,
};
use std::{fs, io, path::Path, path::PathBuf};
use time_alarm_service_messages::{
    AcpiTimerId, AcpiTimestamp, AlarmExpiredWakePolicy, AlarmTimerSeconds, TimeAlarmDeviceCapabilities, TimerStatus,
};

/// Errors produced by the Linux OS data source.
#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Parse,
    Unavailable(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Parse => write!(f, "Parse error"),
            Self::Unavailable(what) => write!(f, "Unavailable on Linux: {what}"),
        }
    }
}

impl std::error::Error for Error {}

impl crate::Error for Error {
    fn kind(&self) -> crate::ErrorKind {
        match self {
            Self::Io(_) => crate::ErrorKind::Io,
            Self::Parse => crate::ErrorKind::InvalidData,
            Self::Unavailable(_) => crate::ErrorKind::Other,
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

fn parse_error<E: std::error::Error + Send + Sync + 'static>(_e: E) -> Error {
    Error::Parse
}

// --- sysfs helpers ---

fn read_u64(path: &Path) -> Result<u64, Error> {
    fs::read_to_string(path)
        .map_err(Error::Io)?
        .trim()
        .parse::<u64>()
        .map_err(parse_error)
}

fn read_string(path: &Path) -> Result<String, Error> {
    Ok(fs::read_to_string(path).map_err(Error::Io)?.trim().to_string())
}

/// Truncate or zero-pad a string into a fixed-size byte array.
fn to_fixed_bytes<const N: usize>(s: &str) -> [u8; N] {
    let mut arr = [0u8; N];
    let bytes = s.as_bytes();
    let len = bytes.len().min(N);
    arr[..len].copy_from_slice(&bytes[..len]);
    arr
}

// --- device discovery ---

/// Find the sysfs path for the first battery power supply.
fn battery_path() -> Result<PathBuf, Error> {
    for entry in fs::read_dir("/sys/class/power_supply/").map_err(Error::Io)? {
        let path = entry.map_err(Error::Io)?.path();
        if fs::read_to_string(path.join("type"))
            .map(|t| t.trim() == "Battery")
            .unwrap_or(false)
        {
            return Ok(path);
        }
    }
    Err(Error::Io(io::Error::new(
        io::ErrorKind::NotFound,
        "no battery found in /sys/class/power_supply/",
    )))
}

/// Find the sysfs path for the first hwmon device that exposes fan inputs.
fn hwmon_path() -> Result<PathBuf, Error> {
    for entry in fs::read_dir("/sys/class/hwmon/").map_err(Error::Io)? {
        let path = entry.map_err(Error::Io)?.path();
        if path.join("fan1_input").exists() {
            return Ok(path);
        }
    }
    Err(Error::Io(io::Error::new(
        io::ErrorKind::NotFound,
        "no hwmon device with fan1_input found",
    )))
}

/// Find the sysfs path for the first ACPI thermal zone, falling back to any thermal zone.
fn thermal_zone_path() -> Result<PathBuf, Error> {
    let mut fallback = None;
    for entry in fs::read_dir("/sys/class/thermal/").map_err(Error::Io)? {
        let path = entry.map_err(Error::Io)?.path();
        if !path.join("temp").exists() {
            continue;
        }
        if fallback.is_none() {
            fallback = Some(path.clone());
        }
        if fs::read_to_string(path.join("type"))
            .map(|t| t.trim() == "acpitz")
            .unwrap_or(false)
        {
            return Ok(path);
        }
    }
    fallback.ok_or_else(|| Error::Io(io::Error::new(io::ErrorKind::NotFound, "no thermal zone found")))
}

/// Read the temperature of the first trip point with the given type (e.g. "active0", "passive",
/// "critical"), returning degrees Celsius.
fn thermal_trip_celsius(zone: &Path, trip_type: &str) -> Result<f64, Error> {
    for i in 0u32.. {
        let type_path = zone.join(format!("trip_point_{i}_type"));
        if !type_path.exists() {
            break;
        }
        if fs::read_to_string(&type_path)
            .map(|t| t.trim() == trip_type)
            .unwrap_or(false)
        {
            let millideg = read_u64(&zone.join(format!("trip_point_{i}_temp")))? as i64;
            return Ok(millideg as f64 / 1000.0);
        }
    }
    Err(Error::Unavailable("thermal trip point not found"))
}

// --- RTC ioctl ---

/// Mirrors the kernel's `struct rtc_time` (9 × i32, packed in declaration order).
#[repr(C)]
struct RtcTime {
    tm_sec: i32,
    tm_min: i32,
    tm_hour: i32,
    tm_mday: i32,
    tm_mon: i32,  // 0-based
    tm_year: i32, // years since 1900
    tm_wday: i32,
    tm_yday: i32,
    tm_isdst: i32,
}

/// Mirrors the kernel's `struct rtc_wkalrm` (2 bytes + 2 padding + rtc_time = 40 bytes).
#[repr(C)]
struct RtcWkalrm {
    enabled: u8,
    pending: u8,
    _pad: [u8; 2],
    time: RtcTime,
}

// _IOR('p', 0x09, struct rtc_time)    sizeof(rtc_time) = 36
const RTC_RD_TIME: libc::c_ulong = 0x8024_7009;
// _IOR('p', 0x10, struct rtc_wkalrm)  sizeof(rtc_wkalrm) = 40
const RTC_WKALM_RD: libc::c_ulong = 0x8028_7010;

struct OwnedFd(libc::c_int);

impl Drop for OwnedFd {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

fn open_rtc() -> Result<OwnedFd, Error> {
    let path = c"/dev/rtc0";
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY) };
    if fd < 0 {
        Err(Error::Io(io::Error::last_os_error()))
    } else {
        Ok(OwnedFd(fd))
    }
}

fn rtc_read_time() -> Result<RtcTime, Error> {
    let fd = open_rtc()?;
    let mut rt = RtcTime {
        tm_sec: 0,
        tm_min: 0,
        tm_hour: 0,
        tm_mday: 0,
        tm_mon: 0,
        tm_year: 0,
        tm_wday: 0,
        tm_yday: 0,
        tm_isdst: 0,
    };
    let ret = unsafe { libc::ioctl(fd.0, RTC_RD_TIME, &mut rt) };
    if ret < 0 {
        Err(Error::Io(io::Error::last_os_error()))
    } else {
        Ok(rt)
    }
}

fn rtc_read_wkalrm() -> Result<RtcWkalrm, Error> {
    let fd = open_rtc()?;
    let mut wkalrm = RtcWkalrm {
        enabled: 0,
        pending: 0,
        _pad: [0; 2],
        time: RtcTime {
            tm_sec: 0,
            tm_min: 0,
            tm_hour: 0,
            tm_mday: 0,
            tm_mon: 0,
            tm_year: 0,
            tm_wday: 0,
            tm_yday: 0,
            tm_isdst: 0,
        },
    };
    let ret = unsafe { libc::ioctl(fd.0, RTC_WKALM_RD, &mut wkalrm) };
    if ret < 0 {
        Err(Error::Io(io::Error::last_os_error()))
    } else {
        Ok(wkalrm)
    }
}

// --- public source type ---

#[derive(Default, Copy, Clone)]
pub struct OsSource {}

impl OsSource {
    pub fn new() -> Self {
        Default::default()
    }
}

impl ErrorType for OsSource {
    type Error = Error;
}

impl ThermalSource for OsSource {
    fn get_temperature(&self) -> Result<f64, Self::Error> {
        let zone = thermal_zone_path()?;
        let millideg = read_u64(&zone.join("temp"))? as i64;
        Ok(millideg as f64 / 1000.0)
    }

    fn get_rpm(&self) -> Result<f64, Self::Error> {
        Ok(read_u64(&hwmon_path()?.join("fan1_input"))? as f64)
    }

    fn get_min_rpm(&self) -> Result<f64, Self::Error> {
        Ok(read_u64(&hwmon_path()?.join("fan1_min"))? as f64)
    }

    fn get_max_rpm(&self) -> Result<f64, Self::Error> {
        Ok(read_u64(&hwmon_path()?.join("fan1_max"))? as f64)
    }

    fn get_threshold(&self, threshold: Threshold) -> Result<f64, Self::Error> {
        let zone = thermal_zone_path()?;
        match threshold {
            Threshold::On => thermal_trip_celsius(&zone, "active0"),
            Threshold::Ramping => thermal_trip_celsius(&zone, "passive"),
            Threshold::Max => thermal_trip_celsius(&zone, "critical"),
        }
    }

    fn set_rpm(&self, rpm: f64) -> Result<(), Self::Error> {
        let target = hwmon_path()?.join("fan1_target");
        if target.exists() {
            fs::write(&target, format!("{}\n", rpm as u64)).map_err(Error::Io)
        } else {
            Err(Error::Unavailable("fan1_target not writable on this system"))
        }
    }
}

impl BatterySource for OsSource {
    fn get_bst(&self) -> Result<BstReturn, Self::Error> {
        let bat = battery_path()?;

        let status = read_string(&bat.join("status"))?;
        let battery_state = match status.as_str() {
            "Charging" => BatteryState::CHARGING,
            "Discharging" => BatteryState::DISCHARGING,
            _ => BatteryState::empty(),
        };

        // Present rate: µW → mW, or µA → mA
        let battery_present_rate = if bat.join("power_now").exists() {
            (read_u64(&bat.join("power_now"))? / 1000) as u32
        } else {
            (read_u64(&bat.join("current_now"))? / 1000) as u32
        };

        // Remaining capacity: µWh → mWh, or µAh → mAh
        let battery_remaining_capacity = if bat.join("energy_now").exists() {
            (read_u64(&bat.join("energy_now"))? / 1000) as u32
        } else {
            (read_u64(&bat.join("charge_now"))? / 1000) as u32
        };

        // Present voltage: µV → mV
        let battery_present_voltage = (read_u64(&bat.join("voltage_now"))? / 1000) as u32;

        Ok(BstReturn {
            battery_state,
            battery_present_rate,
            battery_remaining_capacity,
            battery_present_voltage,
        })
    }

    fn get_bix(&self) -> Result<BixFixedStrings, Self::Error> {
        let bat = battery_path()?;

        // power_unit: 0 = mW, 1 = mA — infer from which capacity attributes are present
        let energy_based = bat.join("energy_full").exists();
        let power_unit = power_unit_try_from_u32(if energy_based { 0 } else { 1 }).map_err(|_| Error::Parse)?;

        let design_capacity = if energy_based {
            (read_u64(&bat.join("energy_full_design"))? / 1000) as u32
        } else {
            (read_u64(&bat.join("charge_full_design"))? / 1000) as u32
        };

        let last_full_charge_capacity = if energy_based {
            (read_u64(&bat.join("energy_full"))? / 1000) as u32
        } else {
            (read_u64(&bat.join("charge_full"))? / 1000) as u32
        };

        let design_voltage = (read_u64(&bat.join("voltage_min_design"))? / 1000) as u32;

        let cycle_count = read_u64(&bat.join("cycle_count")).unwrap_or(0) as u32;

        // Li-ion and Li-poly are rechargeable (Secondary); everything else is Primary.
        let technology_str = read_string(&bat.join("technology")).unwrap_or_default();
        let battery_technology = bat_tech_try_from_u32(match technology_str.as_str() {
            "Li-ion" | "Li-poly" | "LiP" => 1,
            _ => 0,
        })
        .map_err(|_| Error::Parse)?;

        let model_number = to_fixed_bytes(&read_string(&bat.join("model_name")).unwrap_or_default());
        let serial_number = to_fixed_bytes(&read_string(&bat.join("serial_number")).unwrap_or_default());
        let battery_type = to_fixed_bytes(&technology_str);
        let oem_info = to_fixed_bytes(&read_string(&bat.join("manufacturer")).unwrap_or_default());

        Ok(BixFixedStrings {
            revision: 0,
            power_unit,
            design_capacity,
            last_full_charge_capacity,
            battery_technology,
            design_voltage,
            // design_cap_of_warning/low are in sysfs only as percentages; not available as
            // absolute mWh/mAh values without also knowing full charge capacity.
            design_cap_of_warning: 0,
            design_cap_of_low: 0,
            cycle_count,
            // The fields below have no Linux sysfs equivalent.
            measurement_accuracy: 0,
            max_sampling_time: 0,
            min_sampling_time: 0,
            max_averaging_interval: 0,
            min_averaging_interval: 0,
            battery_capacity_granularity_1: 0,
            battery_capacity_granularity_2: 0,
            model_number,
            serial_number,
            battery_type,
            oem_info,
            battery_swapping_capability: bat_swap_try_from_u32(0).map_err(|_| Error::Parse)?,
        })
    }

    fn set_btp(&self, trippoint: u32) -> Result<(), Self::Error> {
        let bat = battery_path()?;
        // sysfs alarm is in µWh/µAh; convert from mWh/mAh by multiplying by 1000.
        let alarm_value = trippoint as u64 * 1000;
        fs::write(bat.join("alarm"), format!("{alarm_value}\n")).map_err(Error::Io)
    }
}

impl RtcSource for OsSource {
    fn get_capabilities(&self) -> Result<TimeAlarmDeviceCapabilities, Self::Error> {
        // No Linux sysfs equivalent for the ACPI _GCP capabilities bitfield.
        Ok(TimeAlarmDeviceCapabilities(0))
    }

    fn get_real_time(&self) -> Result<AcpiTimestamp, Self::Error> {
        let rt = rtc_read_time()?;

        // Build a 16-byte RawAcpiTimestamp buffer (matches the packed struct in acpi_timestamp.rs):
        //   [0..2]  year (u16 LE)
        //   [2]     month (1-based)
        //   [3]     day
        //   [4]     hour
        //   [5]     minute
        //   [6]     second
        //   [7]     valid_or_padding — 1 = time is valid (_GRT semantics)
        //   [8..10] milliseconds (u16 LE)
        //   [10..12] time_zone (i16 LE) — 2047 = unspecified
        //   [12]    daylight (u8) — 0 = NotObserved
        //   [13..16] _padding
        let mut buf = [0u8; 16];
        buf[0..2].copy_from_slice(&((rt.tm_year + 1900) as u16).to_le_bytes());
        buf[2] = (rt.tm_mon + 1) as u8; // tm_mon is 0-based; UEFI is 1-based
        buf[3] = rt.tm_mday as u8;
        buf[4] = rt.tm_hour as u8;
        buf[5] = rt.tm_min as u8;
        buf[6] = rt.tm_sec as u8;
        buf[7] = 1; // valid_or_padding = 1 (time is valid)
        // buf[8..10] milliseconds = 0 (already zeroed)
        buf[10..12].copy_from_slice(&2047i16.to_le_bytes()); // EFI_UNSPECIFIED_TIMEZONE
        // buf[12] daylight = 0 (NotObserved, already zeroed)
        // buf[13..16] _padding (already zeroed)

        AcpiTimestamp::try_from_bytes(&buf).map_err(|_| Error::Parse)
    }

    fn get_wake_status(&self, _timer_id: AcpiTimerId) -> Result<TimerStatus, Self::Error> {
        let wkalrm = rtc_read_wkalrm()?;
        // Linux has a single RTC alarm shared by both AC and DC timer IDs.
        // Report bit 0 (AC) and bit 1 (DC) together to reflect that status.
        let bits = if wkalrm.enabled != 0 { 0x03u32 } else { 0x00u32 };
        Ok(TimerStatus(bits))
    }

    fn get_expired_timer_wake_policy(&self, _timer_id: AcpiTimerId) -> Result<AlarmExpiredWakePolicy, Self::Error> {
        Err(Error::Unavailable(
            "_TIP (expired timer wake policy) has no Linux equivalent",
        ))
    }

    fn get_timer_value(&self, _timer_id: AcpiTimerId) -> Result<AlarmTimerSeconds, Self::Error> {
        Err(Error::Unavailable(
            "_TIV (timer interrupt value) has no Linux equivalent",
        ))
    }
}
