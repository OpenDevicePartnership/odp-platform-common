//! Host-side UCSI value types and platform-neutral mailbox decoding.
//!
//! The UCSI shared mailbox is 48 bytes; the PPM fills VERSION, CCI and the
//! MESSAGE IN payload. Decoding lives here — not in the Windows-only [`crate::acpi`]
//! backend — so it can be unit-tested on any host.

use std::fmt;

/// Total UCSI mailbox size in bytes.
pub const MAILBOX_LEN: usize = 48;
/// UCSI 1.2 version word reported in the mailbox VERSION field.
pub const UCSI_VERSION_1_2: u16 = 0x0120;
/// Length of the UCSI CONTROL field the OS writes to issue a command.
pub const CONTROL_LEN: usize = 8;

const VERSION_OFFSET: usize = 0;
const CCI_OFFSET: usize = 4;
const MESSAGE_IN_OFFSET: usize = 16;
const MESSAGE_IN_LEN: usize = 16;

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

impl UcsiVersion {
    /// Major version digit.
    pub fn major(self) -> u8 {
        (self.0 >> 8) as u8
    }
    /// Minor version digit.
    pub fn minor(self) -> u8 {
        ((self.0 >> 4) & 0xf) as u8
    }
}

impl fmt::Display for UcsiVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major(), self.minor())
    }
}

/// Command Status and Connector Change Indicator (UCSI spec 4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cci(pub u32);

impl Cci {
    /// Length of the returned MESSAGE IN data (bits 15..8).
    pub fn data_len(self) -> u8 {
        (self.0 >> 8) as u8
    }
    /// Command was not supported (bit 25).
    pub fn not_supported(self) -> bool {
        self.0 & (1 << 25) != 0
    }
    /// Busy (bit 28).
    pub fn busy(self) -> bool {
        self.0 & (1 << 28) != 0
    }
    /// Command error (bit 30).
    pub fn error(self) -> bool {
        self.0 & (1 << 30) != 0
    }
    /// Command complete (bit 31).
    pub fn cmd_complete(self) -> bool {
        self.0 & (1 << 31) != 0
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
    /// BCD-coded USB Type-C spec version.
    pub bcd_usb_type_c_version: u16,
}

/// Connector operation-mode flags (subset used by the host UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OperationMode {
    /// Dual-role port.
    pub drp: bool,
    /// USB 2.0 capable.
    pub usb2: bool,
    /// USB 3.x capable.
    pub usb3: bool,
}

/// Per-connector capabilities (GET_CONNECTOR_CAPABILITY response).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UcsiConnectorCapability {
    /// Supported operation modes.
    pub operation_mode: OperationMode,
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
    /// The mailbox buffer was not exactly [`MAILBOX_LEN`] bytes.
    WrongLength {
        /// Expected byte count.
        expected: usize,
        /// Actual byte count.
        actual: usize,
    },
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
            Self::WrongLength { expected, actual } => {
                write!(f, "mailbox length {actual} bytes, expected {expected}")
            }
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

/// A decoded UCSI mailbox: validated VERSION + CCI plus the raw MESSAGE IN bytes.
#[derive(Debug, Clone)]
pub struct Mailbox {
    version: UcsiVersion,
    cci: Cci,
    message_in: [u8; MESSAGE_IN_LEN],
}

impl Mailbox {
    /// Validate VERSION and CCI, returning the decoded mailbox.
    ///
    /// Rejects a wrong-sized buffer, an unsupported VERSION, or a CCI that is
    /// not command-complete / reports error / not-supported.
    pub fn decode(bytes: &[u8]) -> Result<Self, MailboxError> {
        if bytes.len() != MAILBOX_LEN {
            return Err(MailboxError::WrongLength {
                expected: MAILBOX_LEN,
                actual: bytes.len(),
            });
        }
        let version = UcsiVersion(u16::from_le_bytes([bytes[VERSION_OFFSET], bytes[VERSION_OFFSET + 1]]));
        if version.0 != UCSI_VERSION_1_2 {
            return Err(MailboxError::UnsupportedVersion(version.0));
        }
        let cci = Cci(u32::from_le_bytes(
            bytes[CCI_OFFSET..CCI_OFFSET + 4].try_into().expect("4-byte CCI slice"),
        ));
        if !cci.cmd_complete() {
            return Err(MailboxError::NotComplete);
        }
        if cci.error() {
            return Err(MailboxError::CommandError);
        }
        if cci.not_supported() {
            return Err(MailboxError::NotSupported);
        }
        let mut message_in = [0u8; MESSAGE_IN_LEN];
        message_in.copy_from_slice(&bytes[MESSAGE_IN_OFFSET..MESSAGE_IN_OFFSET + MESSAGE_IN_LEN]);
        Ok(Self {
            version,
            cci,
            message_in,
        })
    }

    /// UCSI version reported in the mailbox.
    pub fn version(&self) -> UcsiVersion {
        self.version
    }

    /// Raw CCI reported in the mailbox.
    pub fn cci(&self) -> Cci {
        self.cci
    }

    /// The first `expected` MESSAGE IN bytes, after checking CCI data length.
    fn data(&self, expected: usize) -> Result<&[u8], MailboxError> {
        let actual = self.cci.data_len() as usize;
        if actual != expected {
            return Err(MailboxError::UnexpectedDataLen { expected, actual });
        }
        Ok(&self.message_in[..expected])
    }

    /// Decode a GET_CAPABILITY (16-byte) response.
    pub fn capability(&self) -> Result<UcsiCapability, MailboxError> {
        let d = self.data(16)?;
        let attributes = u32::from_le_bytes([d[0], d[1], d[2], d[3]]);
        Ok(UcsiCapability {
            num_connectors: d[4],
            usb_pd_supported: attributes & (1 << 2) != 0,
            bcd_pd_version: u16::from_le_bytes([d[12], d[13]]),
            bcd_usb_type_c_version: u16::from_le_bytes([d[14], d[15]]),
        })
    }

    /// Decode a GET_CONNECTOR_CAPABILITY (2-byte) response.
    pub fn connector_capability(&self) -> Result<UcsiConnectorCapability, MailboxError> {
        let d = self.data(2)?;
        let raw = u16::from_le_bytes([d[0], d[1]]);
        let op = (raw & 0xff) as u8;
        Ok(UcsiConnectorCapability {
            operation_mode: OperationMode {
                drp: op & (1 << 2) != 0,
                usb2: op & (1 << 5) != 0,
                usb3: op & (1 << 6) != 0,
            },
            provider: raw & (1 << 8) != 0,
            consumer: raw & (1 << 9) != 0,
        })
    }

    /// Decode a GET_CONNECTOR_STATUS (11-byte) response.
    pub fn connector_status(&self) -> Result<UcsiConnectorStatus, MailboxError> {
        let d = self.data(11)?;
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
}

/// Read bit `index` from a little-endian byte slice (bit 0 = LSB of byte 0).
fn bit(bytes: &[u8], index: usize) -> bool {
    (bytes[index / 8] >> (index % 8)) & 1 == 1
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
        buf[VERSION_OFFSET..VERSION_OFFSET + 2].copy_from_slice(&UCSI_VERSION_1_2.to_le_bytes());
        buf[CCI_OFFSET..CCI_OFFSET + 4].copy_from_slice(&cci.to_le_bytes());
        buf[MESSAGE_IN_OFFSET..MESSAGE_IN_OFFSET + message_in.len()].copy_from_slice(message_in);
        buf
    }

    // ── control ───────────────────────────────────────────────────────────────

    #[test]
    fn control_places_opcode_and_connector() {
        let c = control(opcode::GET_CONNECTOR_STATUS, 1);
        assert_eq!(c[0], 0x12);
        assert_eq!(c[1], 0x00);
        assert_eq!(c[2], 0x01);
        assert_eq!(&c[3..], &[0u8; 5]);
    }

    // ── version / display ─────────────────────────────────────────────────────

    #[test]
    fn version_display_is_major_minor() {
        assert_eq!(UcsiVersion(0x0120).to_string(), "1.2");
    }

    // ── decode: VERSION / CCI / length validation ─────────────────────────────

    #[test]
    fn decode_rejects_wrong_length() {
        assert_eq!(
            Mailbox::decode(&[0u8; 47]).unwrap_err(),
            MailboxError::WrongLength {
                expected: 48,
                actual: 47
            }
        );
    }

    #[test]
    fn decode_rejects_unsupported_version() {
        let mut buf = mailbox(cci_complete(16), &[]);
        buf[0..2].copy_from_slice(&0x0100u16.to_le_bytes());
        assert_eq!(
            Mailbox::decode(&buf).unwrap_err(),
            MailboxError::UnsupportedVersion(0x0100)
        );
    }

    #[test]
    fn decode_rejects_incomplete_cci() {
        let buf = mailbox(0, &[]);
        assert_eq!(Mailbox::decode(&buf).unwrap_err(), MailboxError::NotComplete);
    }

    #[test]
    fn decode_rejects_error_cci() {
        let buf = mailbox((1 << 31) | (1 << 30), &[]);
        assert_eq!(Mailbox::decode(&buf).unwrap_err(), MailboxError::CommandError);
    }

    #[test]
    fn decode_rejects_not_supported_cci() {
        let buf = mailbox((1 << 31) | (1 << 25), &[]);
        assert_eq!(Mailbox::decode(&buf).unwrap_err(), MailboxError::NotSupported);
    }

    #[test]
    fn decode_accepts_valid_header() {
        let buf = mailbox(cci_complete(16), &[]);
        let mb = Mailbox::decode(&buf).expect("valid mailbox");
        assert_eq!(mb.version(), UcsiVersion(0x0120));
        assert_eq!(mb.cci().data_len(), 16);
    }

    // ── capability (16 bytes) ─────────────────────────────────────────────────

    #[test]
    fn capability_decodes_fixture() {
        // attributes bit2 (USB PD), num_connectors=1, bcdPD=0x0300, bcdTypeC=0x0200.
        let mut msg = [0u8; 16];
        msg[0] = 0b0000_0100;
        msg[4] = 1;
        msg[12..14].copy_from_slice(&0x0300u16.to_le_bytes());
        msg[14..16].copy_from_slice(&0x0200u16.to_le_bytes());
        let mb = Mailbox::decode(&mailbox(cci_complete(16), &msg)).unwrap();
        assert_eq!(
            mb.capability().unwrap(),
            UcsiCapability {
                num_connectors: 1,
                usb_pd_supported: true,
                bcd_pd_version: 0x0300,
                bcd_usb_type_c_version: 0x0200,
            }
        );
    }

    #[test]
    fn capability_rejects_wrong_data_len() {
        let mb = Mailbox::decode(&mailbox(cci_complete(2), &[0u8; 16])).unwrap();
        assert_eq!(
            mb.capability(),
            Err(MailboxError::UnexpectedDataLen {
                expected: 16,
                actual: 2
            })
        );
    }

    // ── connector capability (2 bytes) ────────────────────────────────────────

    #[test]
    fn connector_capability_decodes_fixture() {
        // operation_mode = drp|usb2|usb3, provider + consumer.
        let op = (1 << 2) | (1 << 5) | (1 << 6);
        let raw: u16 = op | (1 << 8) | (1 << 9);
        let mb = Mailbox::decode(&mailbox(cci_complete(2), &raw.to_le_bytes())).unwrap();
        assert_eq!(
            mb.connector_capability().unwrap(),
            UcsiConnectorCapability {
                operation_mode: OperationMode {
                    drp: true,
                    usb2: true,
                    usb3: true,
                },
                provider: true,
                consumer: true,
            }
        );
    }

    // ── connector status (11 bytes) ───────────────────────────────────────────

    #[test]
    fn connector_status_decodes_connected_sink() {
        // connect_status bit19, power_direction bit20=0 (sink), partner usb bit21.
        let mut msg = [0u8; 11];
        msg[2] = (1 << 3) | (1 << 5); // bit19 (connect) + bit21 (partner usb)
        let mb = Mailbox::decode(&mailbox(cci_complete(11), &msg)).unwrap();
        assert_eq!(
            mb.connector_status().unwrap(),
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
        let mb = Mailbox::decode(&mailbox(cci_complete(11), &msg)).unwrap();
        assert_eq!(mb.connector_status().unwrap().power_direction, PowerDirection::Source);
    }
}
