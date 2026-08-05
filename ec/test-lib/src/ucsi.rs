//! Host-side UCSI value types and platform-neutral mailbox decoding.
//!
//! The UCSI shared mailbox is 48 bytes; the PPM fills VERSION, CCI and the
//! MESSAGE IN payload. Decoding lives here — not in the Windows-only [`crate::acpi`]
//! backend — so it can be unit-tested on any host.

use std::fmt;

/// Length of the UCSI CONTROL field the OS writes to issue a command.
pub const CONTROL_LEN: usize = 8;

const MAILBOX_LEN: usize = 48;
const UCSI_VERSION_1_2: u16 = 0x0120;
const MESSAGE_IN_OFFSET: usize = 16;

/// UCSI command opcodes for the host read surface.
pub mod opcode {
    /// GET_CAPABILITY (PPM capabilities, 16-byte response).
    pub const GET_CAPABILITY: u8 = 0x06;
    /// GET_CONNECTOR_CAPABILITY (per-connector, 2-byte response).
    pub const GET_CONNECTOR_CAPABILITY: u8 = 0x07;
    /// GET_CONNECTOR_STATUS (per-connector, 11-byte response).
    pub const GET_CONNECTOR_STATUS: u8 = 0x12;
}

/// Build an 8-byte CONTROL buffer: byte 0 = opcode, byte 2 = connector number.
///
/// Matches the UCSI command header (opcode, data-length=0) followed by the
/// LPM connector number in the command-specific field.
pub fn control(opcode: u8, connector: u8) -> [u8; CONTROL_LEN] {
    let mut buf = [0u8; CONTROL_LEN];
    buf[0] = opcode;
    buf[2] = connector;
    buf
}

/// UCSI interface version (BCD; `0x0120` == UCSI 1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UcsiVersion(pub u16);

impl fmt::Display for UcsiVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.0 >> 8, (self.0 >> 4) & 0xf)
    }
}

/// PPM capabilities (GET_CAPABILITY response, subset used by the host UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UcsiCapability {
    /// Number of connectors managed by the PPM.
    pub num_connectors: u8,
    /// PPM supports the USB Power Delivery specification.
    pub usb_pd_supported: bool,
    /// BCD-coded USB PD spec version.
    pub bcd_pd_version: u16,
}

/// Per-connector capabilities (GET_CONNECTOR_CAPABILITY response).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UcsiConnectorCapability {
    /// Dual-role port.
    pub drp: bool,
    /// USB 2.0 capable.
    pub usb2: bool,
    /// USB 3.x capable.
    pub usb3: bool,
    /// Connector can act as a power provider (source).
    pub provider: bool,
    /// Connector can act as a power consumer (sink).
    pub consumer: bool,
}

/// Power direction of a connected connector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerDirection {
    /// Consuming power (sink).
    Sink,
    /// Providing power (source).
    Source,
}

impl fmt::Display for PowerDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sink => write!(f, "Sink"),
            Self::Source => write!(f, "Source"),
        }
    }
}

/// Connector status (GET_CONNECTOR_STATUS response, subset used by the host UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UcsiConnectorStatus {
    /// A partner is attached.
    pub connected: bool,
    /// Current power direction.
    pub power_direction: PowerDirection,
    /// Partner is a USB device.
    pub partner_usb: bool,
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
    let cci = u32::from_le_bytes(bytes[4..8].try_into().expect("4-byte CCI slice"));
    if cci & (1 << 31) == 0 {
        return Err(MailboxError::NotComplete);
    }
    if cci & (1 << 30) != 0 {
        return Err(MailboxError::CommandError);
    }
    if cci & (1 << 25) != 0 {
        return Err(MailboxError::NotSupported);
    }
    Ok((version, ((cci >> 8) & 0xff) as usize))
}

/// Validate the header and return the first `expected` MESSAGE IN bytes.
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

/// Read bit `index` from a little-endian byte slice (bit 0 = LSB of byte 0).
fn bit(bytes: &[u8], index: usize) -> bool {
    (bytes[index / 8] >> (index % 8)) & 1 == 1
}

/// Validate a mailbox and return the UCSI version.
pub fn decode_version(bytes: &[u8]) -> Result<UcsiVersion, MailboxError> {
    let (version, _) = validate(bytes)?;
    Ok(UcsiVersion(version))
}

/// Decode a GET_CAPABILITY (16-byte) response.
pub fn decode_capability(bytes: &[u8]) -> Result<UcsiCapability, MailboxError> {
    let d = message_in(bytes, 16)?;
    Ok(UcsiCapability {
        num_connectors: d[4],
        usb_pd_supported: d[0] & (1 << 2) != 0,
        bcd_pd_version: u16::from_le_bytes([d[12], d[13]]),
    })
}

/// Decode a GET_CONNECTOR_CAPABILITY (2-byte) response.
pub fn decode_connector_capability(bytes: &[u8]) -> Result<UcsiConnectorCapability, MailboxError> {
    let d = message_in(bytes, 2)?;
    let raw = u16::from_le_bytes([d[0], d[1]]);
    let op = raw as u8;
    Ok(UcsiConnectorCapability {
        drp: op & (1 << 2) != 0,
        usb2: op & (1 << 5) != 0,
        usb3: op & (1 << 6) != 0,
        provider: raw & (1 << 8) != 0,
        consumer: raw & (1 << 9) != 0,
    })
}

/// Decode a GET_CONNECTOR_STATUS (11-byte) response.
pub fn decode_connector_status(bytes: &[u8]) -> Result<UcsiConnectorStatus, MailboxError> {
    let d = message_in(bytes, 11)?;
    Ok(UcsiConnectorStatus {
        connected: bit(d, 19),
        power_direction: if bit(d, 20) {
            PowerDirection::Source
        } else {
            PowerDirection::Sink
        },
        partner_usb: bit(d, 21),
    })
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
        let c = control(opcode::GET_CONNECTOR_STATUS, 1);
        assert_eq!(c[0], 0x12);
        assert_eq!(c[1], 0x00);
        assert_eq!(c[2], 0x01);
        assert_eq!(&c[3..], &[0u8; 5]);
    }

    // ── header validation boundaries ──────────────────────────────────────────

    #[test]
    fn rejects_wrong_length() {
        assert_eq!(decode_version(&[0u8; 47]).unwrap_err(), MailboxError::WrongLength(47));
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
    fn version_decodes_and_displays() {
        let mut buf = mailbox(cci_complete(16), &[]);
        buf[0..2].copy_from_slice(&0x0200u16.to_le_bytes());
        assert_eq!(decode_version(&buf).unwrap(), UcsiVersion(0x0200));
        assert_eq!(UcsiVersion(0x0120).to_string(), "1.2");
    }

    // ── field bit decode ──────────────────────────────────────────────────────

    #[test]
    fn capability_decodes_fixture() {
        // attributes bit2 (USB PD), num_connectors=1, bcdPD=0x0300.
        let mut msg = [0u8; 16];
        msg[0] = 0b0000_0100;
        msg[4] = 1;
        msg[12..14].copy_from_slice(&0x0300u16.to_le_bytes());
        let cap = decode_capability(&mailbox(cci_complete(16), &msg)).unwrap();
        assert_eq!(
            cap,
            UcsiCapability {
                num_connectors: 1,
                usb_pd_supported: true,
                bcd_pd_version: 0x0300,
            }
        );
    }

    #[test]
    fn connector_capability_decodes_fixture() {
        // operation_mode = drp|usb2|usb3, provider + consumer.
        let op = (1 << 2) | (1 << 5) | (1 << 6);
        let raw: u16 = op | (1 << 8) | (1 << 9);
        let cap = decode_connector_capability(&mailbox(cci_complete(2), &raw.to_le_bytes())).unwrap();
        assert_eq!(
            cap,
            UcsiConnectorCapability {
                drp: true,
                usb2: true,
                usb3: true,
                provider: true,
                consumer: true,
            }
        );
    }

    #[test]
    fn connector_status_decodes_connected_sink() {
        // connect_status bit19, power_direction bit20=0 (sink), partner usb bit21.
        let mut msg = [0u8; 11];
        msg[2] = (1 << 3) | (1 << 5);
        assert_eq!(
            decode_connector_status(&mailbox(cci_complete(11), &msg)).unwrap(),
            UcsiConnectorStatus {
                connected: true,
                power_direction: PowerDirection::Sink,
                partner_usb: true,
            }
        );
    }

    #[test]
    fn connector_status_decodes_source_direction() {
        let mut msg = [0u8; 11];
        msg[2] = (1 << 3) | (1 << 4); // connect + power_direction=source (bit20)
        let status = decode_connector_status(&mailbox(cci_complete(11), &msg)).unwrap();
        assert_eq!(status.power_direction, PowerDirection::Source);
    }
}
