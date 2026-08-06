//! Windows data source.
use crate::mock::Mock;
use crate::ucsi::{UcsiCapability, UcsiConnectorCapability, UcsiConnectorStatus, UcsiVersion};
use crate::{BatterySource, ErrorType, RtcSource, ThermalSource, Threshold, UcsiSource};
use battery_service_interface::{BixFixedStrings, BstReturn};
use scopeguard::defer;
use time_alarm_service_interface::{
    AcpiTimerId, AcpiTimestamp, AlarmExpiredWakePolicy, AlarmTimerSeconds, TimeAlarmDeviceCapabilities, TimerStatus,
};
use windows::Win32::Devices::DeviceAndDriverInstallation::*;
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Power::{
    ACPI_TIME_AND_ALARM_CAPABILITIES, AcpiTimeResolutionMilliseconds, GUID_DEVICE_ACPI_TIME, IOCTL_ACPI_GET_REAL_TIME,
    IOCTL_ACPI_SET_REAL_TIME, IOCTL_GET_ACPI_TIME_AND_ALARM_CAPABILITIES, IOCTL_GET_WAKE_ALARM_POLICY,
    IOCTL_GET_WAKE_ALARM_VALUE, IOCTL_SET_WAKE_ALARM_POLICY, IOCTL_SET_WAKE_ALARM_VALUE,
};
use windows::core::{GUID, PCWSTR};

fn get_device_path(interface_guid: &GUID) -> Result<String, Error> {
    let device_info_set = unsafe {
        SetupDiGetClassDevsW(
            Some(interface_guid),
            PCWSTR::null(),
            HWND::default(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    }
    .map_err(|e| Error::Io(e.code().0))?;

    defer! {
        let _ = unsafe { SetupDiDestroyDeviceInfoList(device_info_set) };
    }

    let mut interface_data = SP_DEVICE_INTERFACE_DATA {
        cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
        ..Default::default()
    };

    unsafe { SetupDiEnumDeviceInterfaces(device_info_set, None, interface_guid, 0, &mut interface_data) }
        .map_err(|_| Error::DeviceNotFound)?;

    // First call reports the required buffer size (fails with ERROR_INSUFFICIENT_BUFFER).
    let mut required = 0u32;
    let _ = unsafe {
        SetupDiGetDeviceInterfaceDetailW(device_info_set, &interface_data, None, 0, Some(&mut required), None)
    };
    if required == 0 {
        return Err(Error::DeviceNotFound);
    }

    // Over-align via u64 so the SP_DEVICE_INTERFACE_DETAIL_DATA_W header and its u16 DevicePath are aligned.
    let mut buffer = vec![0u64; (required as usize).div_ceil(std::mem::size_of::<u64>())];
    let detail = buffer.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
    unsafe {
        (*detail).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
    }
    unsafe {
        SetupDiGetDeviceInterfaceDetailW(
            device_info_set,
            &interface_data,
            Some(detail),
            required,
            Some(&mut required),
            None,
        )
    }
    .map_err(|e| Error::Io(e.code().0))?;

    let path_offset = std::mem::offset_of!(SP_DEVICE_INTERFACE_DETAIL_DATA_W, DevicePath);
    let path_base = buffer.as_ptr() as *const u8;
    let path = unsafe { PCWSTR::from_raw(path_base.add(path_offset) as *const u16).to_string() }
        .map_err(|_| Error::InvalidData)?;
    Ok(path)
}

/// Errors produced by Windows data source operations.
#[derive(Debug)]
pub enum Error {
    /// The device interface could not be found.
    DeviceNotFound,
    /// A Windows API call failed with the returned HRESULT.
    Io(i32),
    /// The device returned a malformed or unexpected buffer.
    InvalidData,
    /// The Windows class-driver source does not expose this operation.
    Unsupported(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeviceNotFound => write!(f, "Device not found"),
            Self::Io(code) => write!(f, "HRESULT {code:#x}"),
            Self::InvalidData => write!(f, "Invalid data"),
            Self::Unsupported(what) => write!(f, "unsupported by Windows source: {what}"),
        }
    }
}

impl std::error::Error for Error {}

impl crate::Error for Error {
    fn kind(&self) -> crate::ErrorKind {
        match self {
            Self::DeviceNotFound => crate::ErrorKind::Io,
            Self::Io(_) => crate::ErrorKind::Io,
            Self::InvalidData => crate::ErrorKind::InvalidData,
            Self::Unsupported(_) => crate::ErrorKind::Other,
        }
    }
}

impl From<crate::mock::Error> for Error {
    fn from(_: crate::mock::Error) -> Self {
        Self::InvalidData
    }
}

/// A resolved handle to one Windows class-driver device interface.
struct WindowsDevice {
    device_path: String,
}

impl WindowsDevice {
    /// Create a device from given `interface_guid`.
    fn new(interface_guid: &GUID) -> Result<Self, Error> {
        Ok(Self {
            device_path: get_device_path(interface_guid)?,
        })
    }

    /// Issue a single buffered IOCTL, requiring the driver to fill `output` completely.
    fn ioctl(&self, code: u32, input: &[u8], output: &mut [u8]) -> Result<(), Error> {
        let wide: Vec<u16> = self.device_path.encode_utf16().chain(std::iter::once(0)).collect();
        let handle = unsafe {
            CreateFileW(
                PCWSTR::from_raw(wide.as_ptr()),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
        }
        .map_err(|e| Error::Io(e.code().0))?;

        defer! {
            let _ = unsafe { CloseHandle(handle) };
        }

        let mut bytes_returned = 0u32;
        unsafe {
            DeviceIoControl(
                handle,
                code,
                if input.is_empty() {
                    None
                } else {
                    Some(input.as_ptr() as *const core::ffi::c_void)
                },
                input.len() as u32,
                if output.is_empty() {
                    None
                } else {
                    Some(output.as_mut_ptr() as *mut core::ffi::c_void)
                },
                output.len() as u32,
                Some(&mut bytes_returned),
                None,
            )
        }
        .map_err(|e| Error::Io(e.code().0))?;

        // A short read — fewer bytes than the output buffer — means a malformed response.
        if bytes_returned as usize != output.len() {
            Err(Error::InvalidData)
        } else {
            Ok(())
        }
    }
}

/// Windows data source.
pub struct Windows {
    time_alarm: WindowsDevice,
    // Revisit: Implement battery and thermal as Windows devices once the drivers exist.
    // For now, delegate to a mock.
    mock: Mock,
}

impl ErrorType for Windows {
    type Error = Error;
}

impl Windows {
    pub fn new() -> Result<Self, Error> {
        Ok(Self {
            time_alarm: WindowsDevice::new(&GUID_DEVICE_ACPI_TIME)?,
            mock: Mock::default(),
        })
    }
}

impl RtcSource for Windows {
    fn get_real_time(&self) -> Result<AcpiTimestamp, Error> {
        // ACPI_REAL_TIME shares the 16-byte ACPI layout used by AcpiTimestamp.
        let mut out = [0u8; 16];
        self.time_alarm.ioctl(IOCTL_ACPI_GET_REAL_TIME, &[], &mut out)?;
        AcpiTimestamp::try_from_bytes(&out).map_err(|_| Error::InvalidData)
    }

    fn set_real_time(&self, timestamp: AcpiTimestamp) -> Result<(), Error> {
        let input = timestamp.as_bytes();
        self.time_alarm.ioctl(IOCTL_ACPI_SET_REAL_TIME, &input, &mut [])?;
        Ok(())
    }

    fn get_timer_value(&self, timer_id: AcpiTimerId) -> Result<AlarmTimerSeconds, Error> {
        // WAKE_ALARM_INFORMATION { TimerIdentifier: u32, Timeout: u32 }
        let mut input = [0u8; 8];
        input[0..4].copy_from_slice(&u32::from(timer_id).to_le_bytes());
        let mut out = [0u8; 8];
        self.time_alarm.ioctl(IOCTL_GET_WAKE_ALARM_VALUE, &input, &mut out)?;
        let seconds = u32::from_le_bytes(out[4..8].try_into().map_err(|_| Error::InvalidData)?);
        Ok(AlarmTimerSeconds(seconds))
    }

    fn set_timer_value(&self, timer_id: AcpiTimerId, value: AlarmTimerSeconds) -> Result<(), Error> {
        let mut input = [0u8; 8];
        input[0..4].copy_from_slice(&u32::from(timer_id).to_le_bytes());
        input[4..8].copy_from_slice(&value.0.to_le_bytes());
        self.time_alarm.ioctl(IOCTL_SET_WAKE_ALARM_VALUE, &input, &mut [])?;
        Ok(())
    }

    fn get_expired_timer_wake_policy(&self, timer_id: AcpiTimerId) -> Result<AlarmExpiredWakePolicy, Error> {
        // WAKE_ALARM_INFORMATION carries the policy seconds in `Timeout`.
        let mut input = [0u8; 8];
        input[0..4].copy_from_slice(&u32::from(timer_id).to_le_bytes());
        let mut out = [0u8; 8];
        self.time_alarm.ioctl(IOCTL_GET_WAKE_ALARM_POLICY, &input, &mut out)?;
        let policy = u32::from_le_bytes(out[4..8].try_into().map_err(|_| Error::InvalidData)?);
        Ok(AlarmExpiredWakePolicy(policy))
    }

    fn set_expired_timer_wake_policy(
        &self,
        timer_id: AcpiTimerId,
        policy: AlarmExpiredWakePolicy,
    ) -> Result<(), Error> {
        let mut input = [0u8; 8];
        input[0..4].copy_from_slice(&u32::from(timer_id).to_le_bytes());
        input[4..8].copy_from_slice(&policy.0.to_le_bytes());
        self.time_alarm.ioctl(IOCTL_SET_WAKE_ALARM_POLICY, &input, &mut [])?;
        Ok(())
    }

    fn get_capabilities(&self) -> Result<TimeAlarmDeviceCapabilities, Error> {
        let mut raw = ACPI_TIME_AND_ALARM_CAPABILITIES::default();
        // SAFETY: `raw` is `#[repr(C)]` + `Copy`; this views exactly its own bytes for the IOCTL.
        let out = unsafe {
            std::slice::from_raw_parts_mut(
                std::ptr::from_mut(&mut raw).cast::<u8>(),
                std::mem::size_of::<ACPI_TIME_AND_ALARM_CAPABILITIES>(),
            )
        };
        self.time_alarm
            .ioctl(IOCTL_GET_ACPI_TIME_AND_ALARM_CAPABILITIES, &[], out)?;
        let mut caps = TimeAlarmDeviceCapabilities(0);
        caps.set_ac_wake_implemented(raw.AcWakeSupported.into());
        caps.set_dc_wake_implemented(raw.DcWakeSupported.into());
        caps.set_realtime_implemented(raw.RealTimeFeaturesSupported.into());
        caps.set_realtime_accuracy_in_milliseconds(raw.RealTimeResolution == AcpiTimeResolutionMilliseconds);
        caps.set_get_wake_status_supported(raw.S4S5WakeStatusSupported.into());
        caps.set_ac_s4_wake_supported(raw.S4AcWakeSupported.into());
        caps.set_ac_s5_wake_supported(raw.S5AcWakeSupported.into());
        caps.set_dc_s4_wake_supported(raw.S4DcWakeSupported.into());
        caps.set_dc_s5_wake_supported(raw.S5DcWakeSupported.into());
        Ok(caps)
    }

    // Revisit: acpitime.c exposes no IOCTL for _GWS/_CWS; so return fake data for now?
    // The HIDTime.sys driver might need to define/expose these manually?
    fn get_wake_status(&self, _timer_id: AcpiTimerId) -> Result<TimerStatus, Error> {
        Ok(TimerStatus(0))
    }

    fn clear_wake_status(&self, _timer_id: AcpiTimerId) -> Result<(), Error> {
        Ok(())
    }
}

// Revisit: Implement these as Windows devices like TAD once the drivers exist for them.
// For now, just return mock data.
impl ThermalSource for Windows {
    fn get_temperature(&self) -> Result<f64, Error> {
        ThermalSource::get_temperature(&self.mock).map_err(Into::into)
    }

    fn get_rpm(&self) -> Result<f64, Error> {
        ThermalSource::get_rpm(&self.mock).map_err(Into::into)
    }

    fn get_min_rpm(&self) -> Result<f64, Error> {
        ThermalSource::get_min_rpm(&self.mock).map_err(Into::into)
    }

    fn get_max_rpm(&self) -> Result<f64, Error> {
        ThermalSource::get_max_rpm(&self.mock).map_err(Into::into)
    }

    fn get_threshold(&self, threshold: Threshold) -> Result<f64, Error> {
        ThermalSource::get_threshold(&self.mock, threshold).map_err(Into::into)
    }

    fn set_threshold(&self, threshold: Threshold, value: f64) -> Result<(), Error> {
        ThermalSource::set_threshold(&self.mock, threshold, value).map_err(Into::into)
    }

    fn set_rpm(&self, rpm: f64) -> Result<(), Error> {
        ThermalSource::set_rpm(&self.mock, rpm).map_err(Into::into)
    }
}

impl BatterySource for Windows {
    fn get_bst(&self) -> Result<BstReturn, Error> {
        BatterySource::get_bst(&self.mock).map_err(Into::into)
    }

    fn get_bix(&self) -> Result<BixFixedStrings, Error> {
        BatterySource::get_bix(&self.mock).map_err(Into::into)
    }

    fn set_btp(&self, trippoint: u32) -> Result<(), Error> {
        BatterySource::set_btp(&self.mock, trippoint).map_err(Into::into)
    }
}

impl UcsiSource for Windows {
    fn get_version(&self) -> Result<UcsiVersion, Self::Error> {
        Err(Error::Unsupported("UCSI get_version"))
    }

    fn get_capability(&self) -> Result<UcsiCapability, Self::Error> {
        Err(Error::Unsupported("UCSI get_capability"))
    }

    fn get_connector_capability(&self, _connector: u8) -> Result<UcsiConnectorCapability, Self::Error> {
        Err(Error::Unsupported("UCSI get_connector_capability"))
    }

    fn get_connector_status(&self, _connector: u8) -> Result<UcsiConnectorStatus, Self::Error> {
        Err(Error::Unsupported("UCSI get_connector_status"))
    }
}
