use crate::{BatterySource, ErrorType, RtcSource, ThermalSource, Threshold, common};
use battery_service_messages::{
    BatteryState, BixFixedStrings, BstReturn, bat_swap_try_from_u32, bat_tech_try_from_u32, power_unit_try_from_u32,
};
use scopeguard::defer;
use time_alarm_service_messages::{
    AcpiTimerId, AcpiTimestamp, AlarmExpiredWakePolicy, AlarmTimerSeconds, TimeAlarmDeviceCapabilities, TimerStatus,
};
use windows::Win32::Devices::DeviceAndDriverInstallation::*;
use windows::Win32::Devices::Properties::DEVPROPTYPE;
use windows::Win32::Devices::Properties::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::IO::*;
use windows::core::{GUID, PCWSTR};

// GUID defined in the KMDF INX file for ectest.sys
// {5362ad97-ddfe-429d-9305-31c0ad27880a}
const GUID_DEVCLASS_ECTEST: GUID = GUID::from_values(
    0x5362ad97,
    0xddfe,
    0x429d,
    [0x93, 0x05, 0x31, 0xc0, 0xad, 0x27, 0x88, 0x0a],
);
const IOCTL_ACPI_EVAL_METHOD_EX: u32 = 0x0032C018; // CTL_CODE(FILE_DEVICE_ACPI, 6, METHOD_BUFFERED, FILE_READ_ACCESS | FILE_WRITE_ACCESS)

fn get_device_path() -> Result<String, AcpiParseError> {
    let device_info_set = unsafe {
        SetupDiGetClassDevsW(
            Some(&GUID_DEVCLASS_ECTEST),
            PCWSTR::null(),
            HWND::default(),
            DIGCF_PRESENT,
        )
    }
    .map_err(|e| AcpiParseError::EvaluationFailed(e.code().0))?;

    // Ensure the device info list is always cleaned up when we exit this function.
    defer! {
        let _ = unsafe { SetupDiDestroyDeviceInfoList(device_info_set) };
    }

    let mut device_info_data = SP_DEVINFO_DATA {
        cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
        ..Default::default()
    };

    let mut device_index = 0;
    loop {
        if unsafe { SetupDiEnumDeviceInfo(device_info_set, device_index, &mut device_info_data) }.is_err() {
            break;
        }

        let mut property_buffer = [0u16; 128];
        let mut required_size = 0u32;
        let mut property_type = DEVPROPTYPE(0);

        let success = unsafe {
            SetupDiGetDevicePropertyW(
                device_info_set,
                &device_info_data,
                &DEVPKEY_Device_InstanceId,
                &mut property_type as *mut DEVPROPTYPE,
                Some(std::slice::from_raw_parts_mut(
                    property_buffer.as_mut_ptr() as *mut u8,
                    property_buffer.len() * 2,
                )),
                Some(&mut required_size),
                0,
            )
        };

        if success.is_ok() && required_size > 0 {
            let instance_id =
                String::from_utf16_lossy(&property_buffer[..(required_size as usize / 2).saturating_sub(1)]);
            if instance_id.contains("ETST0001") {
                let mut pdo_name_buffer = [0u16; 128];
                let mut pdo_required_size = 0u32;
                let mut pdo_property_type = DEVPROPTYPE(0);
                let success = unsafe {
                    SetupDiGetDevicePropertyW(
                        device_info_set,
                        &device_info_data,
                        &DEVPKEY_Device_PDOName,
                        &mut pdo_property_type as *mut DEVPROPTYPE,
                        Some(std::slice::from_raw_parts_mut(
                            pdo_name_buffer.as_mut_ptr() as *mut u8,
                            pdo_name_buffer.len() * 2,
                        )),
                        Some(&mut pdo_required_size),
                        0,
                    )
                };

                if success.is_ok() && pdo_required_size > 0 {
                    let pdo_name = String::from_utf16_lossy(
                        &pdo_name_buffer[..(pdo_required_size as usize / 2).saturating_sub(1)],
                    );
                    let path = format!("\\\\.\\GLOBALROOT{}", pdo_name);
                    return Ok(path);
                }
            }
        }
        device_index += 1;
    }

    Err(AcpiParseError::EvaluationFailed(-1)) // Device not found
}

/// ACPI argument types - these correspond to the ACPI_METHOD_ARGUMENT_* defines in apiioct.h from the Windows SDK
#[derive(num_enum::IntoPrimitive, num_enum::TryFromPrimitive, Debug, Copy, Clone)]
#[repr(u16)]
enum AcpiArgumentType {
    Integer = 0x0,
    String = 0x1,
    Buffer = 0x2,
    Package = 0x3,
    PackageEx = 0x4,
}

/// Errors that can occur when parsing ACPI buffers.
#[derive(Debug)]
pub enum AcpiParseError {
    /// The buffer was too short to contain the expected data
    InsufficientLength,
    /// The buffer contained invalid or unrecognized data
    InvalidFormat,
    /// The ACPI evaluation FFI call returned a non-zero error code
    EvaluationFailed(i32),
}

impl std::error::Error for AcpiParseError {}
impl std::fmt::Display for AcpiParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Errors produced by ACPI data source operations.
#[derive(Debug)]
pub enum Error {
    /// ACPI buffer parsing failed
    Parse(AcpiParseError),
    /// Response had an unexpected format (wrong argument count, wrong type, etc.)
    UnexpectedResponse,
    /// ACPI method returned an argument of unexpected type
    UnexpectedArgumentType(u16),
    /// ACPI operation returned nonzero status
    OperationFailed,
    /// Data validation failed (invalid enum discriminant, malformed field, etc.)
    InvalidData,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(e) => write!(f, "ACPI parse error: {e}"),
            Self::UnexpectedResponse => write!(f, "Unexpected response"),
            Self::UnexpectedArgumentType(t) => write!(f, "Unexpected argument type: {t}"),
            Self::OperationFailed => write!(f, "Operation failed"),
            Self::InvalidData => write!(f, "Invalid data"),
        }
    }
}

impl std::error::Error for Error {}

impl crate::Error for Error {
    fn kind(&self) -> crate::ErrorKind {
        match self {
            Self::Parse(_) => crate::ErrorKind::Other,
            Self::UnexpectedResponse => crate::ErrorKind::UnexpectedResponse,
            Self::UnexpectedArgumentType(_) => crate::ErrorKind::UnexpectedResponse,
            Self::OperationFailed => crate::ErrorKind::Other,
            Self::InvalidData => crate::ErrorKind::InvalidData,
        }
    }
}

impl From<AcpiParseError> for Error {
    fn from(e: AcpiParseError) -> Self {
        Self::Parse(e)
    }
}

// A user-friendly ACPI input method containing a name and optional arguments
struct AcpiMethodInput<'a, 'b> {
    name: &'a str,
    args: Option<&'b [AcpiMethodArgument]>,
}

/// A user-friendly ACPI method argument
#[derive(Debug, Copy, Clone)]
pub(crate) enum AcpiMethodArgument {
    /// Arbitrary u32 integer (DWORD)
    Int(u32),
    /// GUID in mixed-endian format
    Guid(uuid::Bytes),
}

#[repr(C)]
#[derive(Debug, Default)]
pub(crate) struct AcpiMethodArgumentV1 {
    pub type_: u16,
    pub data_length: u16,
    pub data_32: u32,
    pub data: Vec<u8>,
}

// Convert a user-friendly ACPI method argument to format expected by driver
impl TryFrom<AcpiMethodArgument> for AcpiMethodArgumentV1 {
    type Error = AcpiParseError;
    fn try_from(arg: AcpiMethodArgument) -> Result<Self, AcpiParseError> {
        Ok(match arg {
            AcpiMethodArgument::Guid(g) => Self {
                type_: 2,
                data_length: 16,
                data_32: 0,
                data: g.to_vec(),
            },
            AcpiMethodArgument::Int(i) => Self {
                type_: 0,
                data_length: 4,
                data_32: i,
                data: i.to_le_bytes().to_vec(),
            },
        })
    }
}

#[repr(C)]
#[derive(Debug)]
pub(crate) struct AcpiEvalInputBufferComplexV1Ex {
    pub signature: u32,
    pub methodname: [u8; 256],
    pub size: u32,
    pub argumentcount: u32,
    pub arguments: Vec<AcpiMethodArgumentV1>,
}

#[repr(C)]
#[derive(Debug, Default)]
pub(crate) struct AcpiEvalOutputBufferV1 {
    pub signature: u32,
    pub length: u32,
    pub count: u32,
    pub arguments: Vec<AcpiMethodArgumentV1>,
}

pub(crate) const ACPI_EVAL_INPUT_BUFFER_COMPLEX_SIGNATURE_EX: u32 = u32::from_le_bytes(*b"AeiF");

// Convert a user-friendly ACPI input method to format expected by driver
impl TryFrom<AcpiMethodInput<'_, '_>> for AcpiEvalInputBufferComplexV1Ex {
    type Error = AcpiParseError;
    fn try_from(method: AcpiMethodInput) -> Result<Self, AcpiParseError> {
        let mut buffer = [0u8; 256];
        let bytes = method.name.as_bytes();
        let len = bytes.len().min(256);
        buffer[..len].copy_from_slice(&bytes[..len]);

        let arguments = if let Some(args) = method.args {
            args.iter()
                .map(|&arg| AcpiMethodArgumentV1::try_from(arg))
                .collect::<Result<Vec<_>, _>>()?
        } else {
            Vec::default()
        };
        let size = arguments.iter().map(|arg| arg.data_length as u32).sum();

        Ok(AcpiEvalInputBufferComplexV1Ex {
            signature: ACPI_EVAL_INPUT_BUFFER_COMPLEX_SIGNATURE_EX,
            methodname: buffer,
            size,
            argumentcount: arguments.len() as u32,
            arguments,
        })
    }
}

// Convert ACPI input struct to a raw, packed byte buffer
impl From<AcpiEvalInputBufferComplexV1Ex> for Vec<u8> {
    fn from(input: AcpiEvalInputBufferComplexV1Ex) -> Self {
        let mut buf = Vec::new();
        buf.extend(&input.signature.to_le_bytes());
        buf.extend(&input.methodname);
        buf.extend(&input.size.to_le_bytes());
        buf.extend(&input.argumentcount.to_le_bytes());

        for arg in input.arguments.iter() {
            buf.extend(&arg.type_.to_le_bytes());
            buf.extend(&arg.data_length.to_le_bytes());
            buf.extend(&arg.data);
        }

        buf
    }
}

// Convert vec[u8] into AcpiEvalOutputBufferV1
impl TryFrom<Vec<u8>> for AcpiEvalOutputBufferV1 {
    type Error = AcpiParseError;
    fn try_from(value: Vec<u8>) -> Result<Self, AcpiParseError> {
        let signature = u32::from_le_bytes(value[0..4].try_into().map_err(|_| AcpiParseError::InvalidFormat)?);
        let length = u32::from_le_bytes(value[4..8].try_into().map_err(|_| AcpiParseError::InvalidFormat)?);
        let count = u32::from_le_bytes(value[8..12].try_into().map_err(|_| AcpiParseError::InvalidFormat)?);

        let mut offset = 12;
        let mut arguments = Vec::new();

        for _ in 0..count {
            if offset + 8 > value.len() {
                return Err(AcpiParseError::InsufficientLength);
            }

            let type_ = u16::from_le_bytes(value[offset..offset + 2].try_into().unwrap());
            let data_length = u16::from_le_bytes(value[offset + 2..offset + 4].try_into().unwrap()) as usize;
            let data_32 = if type_ == 0 {
                u32::from_le_bytes(value[offset + 4..offset + 8].try_into().unwrap())
            } else {
                0
            };
            offset += 4;

            if offset + data_length > value.len() {
                return Err(AcpiParseError::InsufficientLength);
            }

            let data = value[offset..offset + data_length].to_vec();
            offset += data_length;

            arguments.push(AcpiMethodArgumentV1 {
                type_,
                data_length: data_length as u16,
                data_32,
                data,
            });
        }

        // Now return generated content
        Ok(AcpiEvalOutputBufferV1 {
            signature,
            length,
            count,
            arguments,
        })
    }
}

#[derive(Default, Copy, Clone)]
pub struct Acpi {}

impl Acpi {
    pub fn new() -> Self {
        Default::default()
    }

    fn evaluate(name: &str, args: Option<&[AcpiMethodArgument]>) -> Result<AcpiEvalOutputBufferV1, AcpiParseError> {
        // Maximum number of arguments allowed is 7 as per spec
        if let Some(args) = args
            && args.len() > 7
        {
            return Err(AcpiParseError::InsufficientLength);
        }

        let method = AcpiMethodInput { name, args };
        let input = AcpiEvalInputBufferComplexV1Ex::try_from(method)?;

        // Input buffer
        let in_buf: Vec<u8> = input.into();

        // Output buffer
        let out_buf_len = 1024;
        let mut out_buf = vec![0u8; out_buf_len];

        // Get device path
        let device_path = get_device_path()?;

        // Open device
        let device_path_wide: Vec<u16> = device_path.encode_utf16().chain(std::iter::once(0)).collect();
        let h_device = unsafe {
            windows::Win32::Storage::FileSystem::CreateFileW(
                PCWSTR::from_raw(device_path_wide.as_ptr()),
                (GENERIC_READ | GENERIC_WRITE).0,
                windows::Win32::Storage::FileSystem::FILE_SHARE_READ
                    | windows::Win32::Storage::FileSystem::FILE_SHARE_WRITE,
                None,
                windows::Win32::Storage::FileSystem::OPEN_EXISTING,
                windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
        }
        .map_err(|e| AcpiParseError::EvaluationFailed(e.code().0))?;

        if h_device.is_invalid() {
            return Err(AcpiParseError::EvaluationFailed(-2));
        }

        defer! {
            let _ = unsafe { CloseHandle(h_device) };
        }

        // Call DeviceIoControl
        let mut bytes_returned = 0u32;
        let success = unsafe {
            DeviceIoControl(
                h_device,
                IOCTL_ACPI_EVAL_METHOD_EX,
                Some(in_buf.as_ptr() as *const std::ffi::c_void),
                in_buf.len() as u32,
                Some(out_buf.as_mut_ptr() as *mut std::ffi::c_void),
                out_buf_len as u32,
                Some(&mut bytes_returned),
                None,
            )
        };

        match success {
            Ok(_) => {
                // Adjust out_buf_len to actual bytes returned
                out_buf.truncate(bytes_returned as usize);
                AcpiEvalOutputBufferV1::try_from(out_buf)
            }
            Err(e) => Err(AcpiParseError::EvaluationFailed(e.code().0)),
        }
    }

    /// Evaluates the provided method with the provided arguments and returns its single u32 result.
    /// Errors if the result is not a single u32.
    fn evaluate_u32(name: &str, args: Option<&[AcpiMethodArgument]>) -> Result<u32, Error> {
        let output = Acpi::evaluate(name, args)?;

        if output.count != 1 {
            Err(Error::UnexpectedResponse)
        } else if output.arguments[0].type_ != AcpiArgumentType::Integer as u16 {
            Err(Error::UnexpectedArgumentType(output.arguments[0].type_))
        } else {
            Ok(output.arguments[0].data_32)
        }
    }
}

fn acpi_get_var(guid: uuid::Uuid) -> Result<f64, Error> {
    let args = [AcpiMethodArgument::Int(1), AcpiMethodArgument::Guid(guid.to_bytes_le())];
    let output = Acpi::evaluate("\\_SB.ECT0.TGVR", Some(&args))?;

    if output.count != 2 {
        Err(Error::UnexpectedResponse)
    } else if output.arguments[0].data_32 != 0 {
        Err(Error::OperationFailed)
    } else {
        Ok(f64::from(output.arguments[1].data_32))
    }
}

fn acpi_set_var(guid: uuid::Uuid, value: f64) -> Result<(), Error> {
    let value = value as u32;

    let args = [
        AcpiMethodArgument::Int(1),
        AcpiMethodArgument::Guid(guid.to_bytes_le()),
        AcpiMethodArgument::Int(value),
    ];
    let output = Acpi::evaluate("\\_SB.ECT0.TSVR", Some(&args))?;

    if output.count != 1 {
        Err(Error::UnexpectedResponse)
    } else if output.arguments[0].data_32 != 0 {
        Err(Error::OperationFailed)
    } else {
        Ok(())
    }
}

impl ErrorType for Acpi {
    type Error = Error;
}

impl ThermalSource for Acpi {
    fn get_temperature(&self) -> Result<f64, Self::Error> {
        let output = Acpi::evaluate("\\_SB.ECT0.RTMP", None)?;
        if output.count != 1 {
            Err(Error::UnexpectedResponse)
        } else {
            Ok(common::dk_to_c(output.arguments[0].data_32))
        }
    }

    fn get_rpm(&self) -> Result<f64, Self::Error> {
        acpi_get_var(common::guid::FAN_CURRENT_RPM)
    }

    fn get_min_rpm(&self) -> Result<f64, Self::Error> {
        acpi_get_var(common::guid::FAN_MIN_RPM)
    }

    fn get_max_rpm(&self) -> Result<f64, Self::Error> {
        acpi_get_var(common::guid::FAN_MAX_RPM)
    }

    fn get_threshold(&self, threshold: Threshold) -> Result<f64, Self::Error> {
        match threshold {
            Threshold::On => Ok(common::dk_to_c(acpi_get_var(common::guid::FAN_ON_TEMP)? as u32)),
            Threshold::Ramping => Ok(common::dk_to_c(acpi_get_var(common::guid::FAN_RAMP_TEMP)? as u32)),
            Threshold::Max => Ok(common::dk_to_c(acpi_get_var(common::guid::FAN_MAX_TEMP)? as u32)),
        }
    }

    fn set_rpm(&self, rpm: f64) -> Result<(), Self::Error> {
        acpi_set_var(common::guid::FAN_CURRENT_RPM, rpm)
    }
}

impl BatterySource for Acpi {
    fn get_bst(&self) -> Result<BstReturn, Self::Error> {
        let data = Acpi::evaluate("\\_SB.ECT0.TBST", None)?;

        // We are expecting 4 32-bit values
        if data.count != 4 {
            Err(Error::UnexpectedResponse)
        } else {
            Ok(BstReturn {
                battery_state: BatteryState::from_bits(data.arguments[0].data_32).ok_or(Error::InvalidData)?,
                battery_present_rate: data.arguments[1].data_32,
                battery_remaining_capacity: data.arguments[2].data_32,
                battery_present_voltage: data.arguments[3].data_32,
            })
        }
    }

    fn get_bix(&self) -> Result<BixFixedStrings, Self::Error> {
        let data = Acpi::evaluate("\\_SB.ECT0.TBIX", None)?;
        // We are expecting 21 arguments
        if data.count != 21 {
            Err(Error::UnexpectedResponse)
        } else {
            Ok(BixFixedStrings {
                revision: data.arguments[0].data_32,
                power_unit: power_unit_try_from_u32(data.arguments[1].data_32).map_err(|_| Error::InvalidData)?,
                design_capacity: data.arguments[2].data_32,
                last_full_charge_capacity: data.arguments[3].data_32,
                battery_technology: bat_tech_try_from_u32(data.arguments[4].data_32).map_err(|_| Error::InvalidData)?,
                design_voltage: data.arguments[5].data_32,
                design_cap_of_warning: data.arguments[6].data_32,
                design_cap_of_low: data.arguments[7].data_32,
                cycle_count: data.arguments[8].data_32,
                measurement_accuracy: data.arguments[9].data_32,
                max_sampling_time: data.arguments[10].data_32,
                min_sampling_time: data.arguments[11].data_32,
                max_averaging_interval: data.arguments[12].data_32,
                min_averaging_interval: data.arguments[13].data_32,
                battery_capacity_granularity_1: data.arguments[14].data_32,
                battery_capacity_granularity_2: data.arguments[15].data_32,
                model_number: data.arguments[16]
                    .data
                    .clone()
                    .try_into()
                    .map_err(|_| Error::InvalidData)?,
                serial_number: data.arguments[17]
                    .data
                    .clone()
                    .try_into()
                    .map_err(|_| Error::InvalidData)?,
                battery_type: data.arguments[18]
                    .data
                    .clone()
                    .try_into()
                    .map_err(|_| Error::InvalidData)?,
                oem_info: data.arguments[19]
                    .data
                    .clone()
                    .try_into()
                    .map_err(|_| Error::InvalidData)?,
                battery_swapping_capability: bat_swap_try_from_u32(data.arguments[20].data_32)
                    .map_err(|_| Error::InvalidData)?,
            })
        }
    }

    fn set_btp(&self, trippoint: u32) -> Result<(), Self::Error> {
        // No return value is expected according to ACPI spec
        let _ = Acpi::evaluate("\\_SB.ECT0.TBTP", Some(&[AcpiMethodArgument::Int(trippoint)]))?;
        Ok(())
    }
}

impl RtcSource for Acpi {
    fn get_capabilities(&self) -> Result<TimeAlarmDeviceCapabilities, Self::Error> {
        Ok(TimeAlarmDeviceCapabilities(Acpi::evaluate_u32(
            "\\_SB.ECT0._GCP",
            None,
        )?))
    }

    fn get_real_time(&self) -> Result<AcpiTimestamp, Self::Error> {
        let result = Acpi::evaluate("\\_SB.ECT0._GRT", None)?;
        if result.count != 1 {
            return Err(Error::UnexpectedResponse);
        }

        let result = &result.arguments[0];
        if result.type_ != AcpiArgumentType::Buffer as u16 {
            return Err(Error::UnexpectedResponse);
        }

        AcpiTimestamp::try_from_bytes(result.data.as_slice()).map_err(|_| Error::InvalidData)
    }

    fn get_wake_status(&self, timer_id: AcpiTimerId) -> Result<TimerStatus, Self::Error> {
        Ok(TimerStatus(Acpi::evaluate_u32(
            "\\_SB.ECT0._GWS",
            Some(&[AcpiMethodArgument::Int(timer_id.into())]),
        )?))
    }

    fn get_expired_timer_wake_policy(&self, timer_id: AcpiTimerId) -> Result<AlarmExpiredWakePolicy, Self::Error> {
        Ok(AlarmExpiredWakePolicy(Acpi::evaluate_u32(
            "\\_SB.ECT0._TIP",
            Some(&[AcpiMethodArgument::Int(timer_id.into())]),
        )?))
    }

    fn get_timer_value(&self, timer_id: AcpiTimerId) -> Result<AlarmTimerSeconds, Self::Error> {
        Ok(AlarmTimerSeconds(Acpi::evaluate_u32(
            "\\_SB.ECT0._TIV",
            Some(&[AcpiMethodArgument::Int(timer_id.into())]),
        )?))
    }
}
