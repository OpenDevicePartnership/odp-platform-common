//! Host-side UCSI mailbox envelope decoding.
//!
//! Only the 48-byte mailbox envelope — length, VERSION, CCI, and data length —
//! is validated here. The command-specific response payloads are decoded with
//! the upstream [`embedded_usb_pd`] UCSI v1.2 types, which are re-exported below
//! so callers consume the upstream shapes directly. Decoding lives here (not in
//! the Windows-only [`crate::acpi`] backend) so it can be unit-tested on any host.

use std::fmt;

use embedded_usb_pd::ucsi::v1_2::cci::LocalCci;
use embedded_usb_pd::ucsi::v1_2::lpm::{get_connector_capability, get_connector_status};
use embedded_usb_pd::ucsi::v1_2::ppm::get_capability;

/// Power direction of a connected connector.
pub use embedded_usb_pd::PowerRole as PowerDirection;
/// UCSI command opcode selector (re-exported for [`control`] and ACPI callers).
pub use embedded_usb_pd::ucsi::v1_2::CommandType;

/// PPM capabilities (GET_CAPABILITY response).
pub type UcsiCapability = get_capability::ResponseData;
/// Per-connector capabilities (GET_CONNECTOR_CAPABILITY response).
pub type UcsiConnectorCapability = get_connector_capability::ResponseData;
/// Connector status (GET_CONNECTOR_STATUS response).
pub type UcsiConnectorStatus = get_connector_status::ResponseData;

/// Length of the UCSI CONTROL field the OS writes to issue a command.
pub const CONTROL_LEN: usize = 8;

const MAILBOX_LEN: usize = 48;
#[cfg(any(target_os = "windows", test))]
const FFA_ENVELOPE_LEN: usize = 144;
#[cfg(any(target_os = "windows", test))]
const FFA_PAYLOAD_OFFSET: usize = 32;
const UCSI_VERSION_1_2: u16 = 0x0120;
const MESSAGE_IN_OFFSET: usize = 16;

/// Build an 8-byte CONTROL buffer: byte 0 = opcode, byte 2 = connector number.
///
/// Matches the UCSI command header (opcode, data-length=0) followed by the
/// LPM connector number in the command-specific field.
pub fn control(command: CommandType, connector: u8) -> [u8; CONTROL_LEN] {
    let mut buf = [0u8; CONTROL_LEN];
    buf[0] = command as u8;
    buf[2] = connector;
    buf
}

/// Normalize either a direct mailbox or the full FF-A envelope returned by
/// the Windows fixed-hardware operation region.
#[cfg(any(target_os = "windows", test))]
pub(crate) fn normalize_acpi_response(bytes: &[u8]) -> Result<&[u8], MailboxError> {
    match bytes.len() {
        MAILBOX_LEN => Ok(bytes),
        FFA_ENVELOPE_LEN => Ok(&bytes[FFA_PAYLOAD_OFFSET..FFA_PAYLOAD_OFFSET + MAILBOX_LEN]),
        length => Err(MailboxError::WrongLength(length)),
    }
}

/// UCSI interface version (BCD; `0x0120` == UCSI 1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UcsiVersion(pub u16);

impl fmt::Display for UcsiVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.0 >> 8, (self.0 >> 4) & 0xf)
    }
}

/// Error decoding a UCSI mailbox response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailboxError {
    /// The mailbox buffer was not exactly 48 bytes.
    WrongLength(usize),
    /// The VERSION field did not match a supported UCSI version.
    UnsupportedVersion(u16),
    /// CCI did not report command-complete.
    NotComplete,
    /// CCI reported a command error.
    CommandError,
    /// CCI reported the command was not supported.
    NotSupported,
    /// CCI data length did not match the expected response size.
    UnexpectedDataLen {
        /// Expected data length.
        expected: usize,
        /// Actual data length reported in CCI.
        actual: usize,
    },
    /// The command-specific response payload failed to decode.
    PayloadDecode,
}

impl fmt::Display for MailboxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongLength(n) => write!(f, "mailbox length {n} bytes, expected 48"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported UCSI version {v:#06x}"),
            Self::NotComplete => write!(f, "CCI did not report command complete"),
            Self::CommandError => write!(f, "CCI reported command error"),
            Self::NotSupported => write!(f, "CCI reported command not supported"),
            Self::UnexpectedDataLen { expected, actual } => {
                write!(f, "CCI data length {actual}, expected {expected}")
            }
            Self::PayloadDecode => write!(f, "malformed UCSI response payload"),
        }
    }
}

impl std::error::Error for MailboxError {}

/// Validate the 48-byte mailbox length and CCI status, returning VERSION and
/// the CCI data-length field.
fn validate(bytes: &[u8]) -> Result<(u16, usize), MailboxError> {
    if bytes.len() != MAILBOX_LEN {
        return Err(MailboxError::WrongLength(bytes.len()));
    }
    let version = u16::from_le_bytes([bytes[0], bytes[1]]);
    let cci = LocalCci::from(u32::from_le_bytes(bytes[4..8].try_into().expect("4-byte CCI slice")));
    if !cci.cmd_complete() {
        return Err(MailboxError::NotComplete);
    }
    if cci.error() {
        return Err(MailboxError::CommandError);
    }
    if cci.not_supported() {
        return Err(MailboxError::NotSupported);
    }
    Ok((version, cci.data_len()))
}

/// Validate the header (enforcing UCSI 1.2 for command payloads) and return the
/// first `expected` MESSAGE IN bytes.
fn message_in(bytes: &[u8], expected: usize) -> Result<&[u8], MailboxError> {
    let (version, actual) = validate(bytes)?;
    if version != UCSI_VERSION_1_2 {
        return Err(MailboxError::UnsupportedVersion(version));
    }
    if actual != expected {
        return Err(MailboxError::UnexpectedDataLen { expected, actual });
    }
    Ok(&bytes[MESSAGE_IN_OFFSET..MESSAGE_IN_OFFSET + expected])
}

/// Validate a mailbox and return the reported UCSI version.
///
/// Version reporting is forward-compatible: any completed mailbox yields its
/// VERSION word, while the command-specific decoders below still gate on 1.2.
pub fn decode_version(bytes: &[u8]) -> Result<UcsiVersion, MailboxError> {
    let (version, _) = validate(bytes)?;
    Ok(UcsiVersion(version))
}

/// Decode a GET_CAPABILITY response.
pub fn decode_capability(bytes: &[u8]) -> Result<UcsiCapability, MailboxError> {
    let payload = message_in(bytes, get_capability::RESPONSE_DATA_LEN)?;
    let (data, _): (UcsiCapability, usize) =
        bincode::decode_from_slice(payload, bincode::config::standard().with_fixed_int_encoding())
            .map_err(|_| MailboxError::PayloadDecode)?;
    Ok(data)
}

/// Decode a GET_CONNECTOR_CAPABILITY response.
pub fn decode_connector_capability(bytes: &[u8]) -> Result<UcsiConnectorCapability, MailboxError> {
    let payload = message_in(bytes, get_connector_capability::RESPONSE_DATA_LEN)?;
    Ok(UcsiConnectorCapability::from(u16::from_le_bytes([
        payload[0], payload[1],
    ])))
}

/// Decode a GET_CONNECTOR_STATUS response.
pub fn decode_connector_status(bytes: &[u8]) -> Result<UcsiConnectorStatus, MailboxError> {
    let payload = message_in(bytes, get_connector_status::RESPONSE_DATA_LEN)?;
    let raw: [u8; get_connector_status::RESPONSE_DATA_LEN] = payload
        .try_into()
        .expect("message_in returns exactly the requested length");
    UcsiConnectorStatus::try_from(raw).map_err(|_| MailboxError::PayloadDecode)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CCI for a completed command carrying `data_len` bytes.
    fn cci_complete(data_len: u8) -> u32 {
        (1 << 31) | ((data_len as u32) << 8)
    }

    /// Assemble a 48-byte mailbox from a CCI word and MESSAGE IN bytes.
    fn mailbox(cci: u32, message_in: &[u8]) -> [u8; MAILBOX_LEN] {
        let mut buf = [0u8; MAILBOX_LEN];
        buf[0..2].copy_from_slice(&UCSI_VERSION_1_2.to_le_bytes());
        buf[4..8].copy_from_slice(&cci.to_le_bytes());
        buf[MESSAGE_IN_OFFSET..MESSAGE_IN_OFFSET + message_in.len()].copy_from_slice(message_in);
        buf
    }

    #[test]
    fn control_places_opcode_and_connector() {
        let c = control(CommandType::GetConnectorStatus, 1);
        assert_eq!(c[0], 0x12);
        assert_eq!(c[1], 0x00);
        assert_eq!(c[2], 0x01);
        assert_eq!(&c[3..], &[0u8; 5]);
    }

    // ── envelope validation boundaries ────────────────────────────────────────

    #[test]
    fn rejects_wrong_length() {
        assert_eq!(decode_version(&[0u8; 47]).unwrap_err(), MailboxError::WrongLength(47));
    }

    #[test]
    fn normalizes_full_ffa_envelope() {
        const FFA_ENVELOPE_LEN: usize = 144;
        const FFA_PAYLOAD_OFFSET: usize = 32;
        let mailbox = mailbox(cci_complete(16), &[]);
        let mut envelope = [0u8; FFA_ENVELOPE_LEN];
        envelope[FFA_PAYLOAD_OFFSET..FFA_PAYLOAD_OFFSET + MAILBOX_LEN].copy_from_slice(&mailbox);

        assert_eq!(normalize_acpi_response(&envelope).unwrap(), mailbox);
    }

    #[test]
    fn command_decode_rejects_unsupported_version() {
        let mut buf = mailbox(cci_complete(16), &[]);
        buf[0..2].copy_from_slice(&0x0100u16.to_le_bytes());
        assert_eq!(
            decode_capability(&buf).unwrap_err(),
            MailboxError::UnsupportedVersion(0x0100)
        );
    }

    #[test]
    fn rejects_incomplete_cci() {
        assert_eq!(decode_version(&mailbox(0, &[])).unwrap_err(), MailboxError::NotComplete);
    }

    #[test]
    fn rejects_error_cci() {
        let buf = mailbox((1 << 31) | (1 << 30), &[]);
        assert_eq!(decode_version(&buf).unwrap_err(), MailboxError::CommandError);
    }

    #[test]
    fn rejects_not_supported_cci() {
        let buf = mailbox((1 << 31) | (1 << 25), &[]);
        assert_eq!(decode_version(&buf).unwrap_err(), MailboxError::NotSupported);
    }

    #[test]
    fn rejects_wrong_data_len() {
        let buf = mailbox(cci_complete(2), &[0u8; 16]);
        assert_eq!(
            decode_capability(&buf).unwrap_err(),
            MailboxError::UnexpectedDataLen {
                expected: 16,
                actual: 2
            }
        );
    }

    #[test]
    fn version_decodes_forward_compatibly_and_displays() {
        let mut buf = mailbox(cci_complete(16), &[]);
        buf[0..2].copy_from_slice(&0x0200u16.to_le_bytes());
        assert_eq!(decode_version(&buf).unwrap(), UcsiVersion(0x0200));
        assert_eq!(UcsiVersion(0x0120).to_string(), "1.2");
    }

    #[test]
    fn connector_status_payload_error_maps_to_decode() {
        // connect_status set but power-operation-mode = 0 is an invalid variant.
        let mut msg = [0u8; 11];
        msg[2] = 1 << 3; // connect_status (bit 19), power_op_mode left 0
        assert_eq!(
            decode_connector_status(&mailbox(cci_complete(11), &msg)).unwrap_err(),
            MailboxError::PayloadDecode
        );
    }

    // ── envelope → upstream payload decode ────────────────────────────────────

    #[test]
    fn capability_decodes_into_upstream_type() {
        // attributes bit2 (USB PD), num_connectors=1, bcdPD=0x0300.
        let mut msg = [0u8; 16];
        msg[0] = 0b0000_0100;
        msg[4] = 1;
        msg[12..14].copy_from_slice(&0x0300u16.to_le_bytes());
        let cap = decode_capability(&mailbox(cci_complete(16), &msg)).unwrap();
        assert_eq!(cap.num_connectors, 1);
        assert!(cap.attributes.usb_power_delivery());
        assert_eq!(cap.bcd_usb_pd_spec, 0x0300);
    }

    #[test]
    fn connector_capability_decodes_into_upstream_type() {
        // operation_mode = drp|usb2|usb3, provider + consumer.
        let op = (1 << 2) | (1 << 5) | (1 << 6);
        let raw: u16 = op | (1 << 8) | (1 << 9);
        let cap = decode_connector_capability(&mailbox(cci_complete(2), &raw.to_le_bytes())).unwrap();
        assert!(cap.operation_mode().drp());
        assert!(cap.operation_mode().usb2());
        assert!(cap.operation_mode().usb3());
        assert!(cap.provider());
        assert!(cap.consumer());
    }

    #[test]
    fn connector_status_decodes_connected_sink() {
        // bit19 connect, bit20=0 sink, partner usb (bit21); power_op_mode=1,
        // partner_type=1 so the upstream decoder accepts the connected payload.
        let mut msg = [0u8; 11];
        msg[2] = 0x01 | (1 << 3) | (1 << 5); // power_op_mode=1 + connect + partner usb
        msg[3] = 1 << 5; // partner_type = 1 (bits 29..31)
        let status = decode_connector_status(&mailbox(cci_complete(11), &msg)).unwrap();
        assert!(status.connect_status);
        let connected = status.status.expect("connected payload present");
        assert_eq!(connected.power_direction, PowerDirection::Sink);
        assert!(connected.partner_flags.usb());
    }

    #[test]
    fn connector_status_decodes_source_direction() {
        let mut msg = [0u8; 11];
        msg[2] = 0x01 | (1 << 3) | (1 << 4); // power_op_mode=1 + connect + source (bit20)
        msg[3] = 1 << 5; // partner_type = 1
        let status = decode_connector_status(&mailbox(cci_complete(11), &msg)).unwrap();
        assert_eq!(
            status.status.expect("connected payload present").power_direction,
            PowerDirection::Source
        );
    }
}
