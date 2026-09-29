//! Experimental, build-specific native PCC interface client.

const PROTOCOL_VERSION: u32 = 1;
const RESPONSE_SIZE: usize = 48;
/// Largest message exchanged through the native PCC bridge.
pub const MESSAGE_SIZE: usize = 256;
const EXECUTE_REQUEST_SIZE: usize = 16 + MESSAGE_SIZE;
const EXECUTE_RESPONSE_SIZE: usize = 8 + MESSAGE_SIZE;

/// Encode an execute request for PCC subspace 0.
pub fn encode_execute(command: u8, message: &[u8]) -> Result<[u8; EXECUTE_REQUEST_SIZE], &'static str> {
    if message.len() > MESSAGE_SIZE {
        return Err("PCC message exceeds the bridge capacity");
    }
    let mut bytes = [0u8; EXECUTE_REQUEST_SIZE];
    bytes[0..4].copy_from_slice(&PROTOCOL_VERSION.to_le_bytes());
    bytes[8..12].copy_from_slice(&u32::from(command).to_le_bytes());
    bytes[12..16].copy_from_slice(&(message.len() as u32).to_le_bytes());
    bytes[16..16 + message.len()].copy_from_slice(message);
    Ok(bytes)
}

/// Result of one native PCC transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecuteResponse {
    /// NTSTATUS from validation, acquisition, execution or release.
    pub status: i32,
    /// Communication area contents after completion; valid only on success.
    pub message: [u8; MESSAGE_SIZE],
}

impl TryFrom<&[u8]> for ExecuteResponse {
    type Error = &'static str;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        if bytes.len() != EXECUTE_RESPONSE_SIZE {
            return Err("invalid PCC execute response size");
        }
        let word = |offset: usize| {
            u32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]])
        };
        if word(0) != PROTOCOL_VERSION {
            return Err("invalid PCC execute protocol version");
        }
        let mut message = [0u8; MESSAGE_SIZE];
        message.copy_from_slice(&bytes[8..]);
        Ok(Self {
            status: word(4) as i32,
            message,
        })
    }
}

/// Metadata returned by the opt-in native PCC bridge probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeResponse {
    /// NTSTATUS returned by build validation or the native interface query.
    pub query_status: i32,
    /// Native PCC interface version, not the IOCTL protocol version.
    pub interface_version: u32,
    /// Selected PCCT subspace index.
    pub subspace_id: u32,
    /// Selected PCCT subspace type.
    pub subspace_type: u32,
    /// Size of the returned communication area, excluding the Type 3 header.
    pub subspace_size: u32,
    /// Native interface capability flags.
    pub flags: u32,
    /// Nominal command latency in microseconds.
    pub nominal_latency: u32,
    /// Maximum periodic command rate returned by the native engine.
    pub maximum_periodic_rate: u32,
    /// Loaded ACPI image timestamp used by the experimental build gate.
    pub acpi_timestamp: u32,
    /// Loaded ACPI image size used by the experimental build gate.
    pub acpi_image_size: u32,
}

impl TryFrom<&[u8]> for ProbeResponse {
    type Error = &'static str;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        if bytes.len() != RESPONSE_SIZE {
            return Err("invalid PCC probe response size");
        }
        let mut words = [0u32; RESPONSE_SIZE / 4];
        let (fields, _) = bytes.as_chunks::<4>();
        for (word, field) in words.iter_mut().zip(fields) {
            *word = u32::from_le_bytes(*field);
        }
        if words[0] != PROTOCOL_VERSION || words[11] != 0 {
            return Err("invalid PCC probe protocol version or reserved field");
        }
        let response = Self {
            query_status: words[1] as i32,
            interface_version: words[2],
            subspace_id: words[3],
            subspace_type: words[4],
            subspace_size: words[5],
            flags: words[6],
            nominal_latency: words[7],
            maximum_periodic_rate: words[8],
            acpi_timestamp: words[9],
            acpi_image_size: words[10],
        };
        if response.query_status >= 0
            && (response.query_status != 0
                || response.interface_version != 1
                || response.subspace_id != 0
                || response.subspace_type != 3
                || response.subspace_size != 4080
                || response.flags & !1 != 0)
        {
            return Err("unexpected native PCC interface metadata");
        }
        Ok(response)
    }
}

/// Client of the existing test bridge's experimental native PCC IOCTLs.
pub struct Pcc {
    device: crate::windows::WindowsDevice,
}

impl Pcc {
    /// Open the test bridge's device interface.
    pub fn new() -> Result<Self, crate::windows::Error> {
        let interface = windows::core::GUID::from_u128(0xcdc35b6e_0be4_4936_bf5f_5537380a7c1a);
        Ok(Self {
            device: crate::windows::WindowsDevice::new(&interface)?,
        })
    }

    /// Query native PCC subspace 0 without reading or writing the mailbox.
    pub fn probe(&self) -> Result<ProbeResponse, crate::windows::Error> {
        const IOCTL_ECTEST_PCC_PROBE: u32 = 0x0022_E000;
        let mut input = [0u8; 8];
        input[..4].copy_from_slice(&PROTOCOL_VERSION.to_le_bytes());
        let mut output = [0u8; RESPONSE_SIZE];
        self.device.ioctl(IOCTL_ECTEST_PCC_PROBE, &input, &mut output)?;
        ProbeResponse::try_from(output.as_slice()).map_err(|_| crate::windows::Error::InvalidData)
    }

    /// Send one command through Windows' native PCC transport on subspace 0.
    pub fn execute(&self, command: u8, message: &[u8]) -> Result<ExecuteResponse, crate::windows::Error> {
        const IOCTL_ECTEST_PCC_EXECUTE: u32 = 0x0022_E004;
        let input = encode_execute(command, message).map_err(|_| crate::windows::Error::InvalidData)?;
        let mut output = [0u8; EXECUTE_RESPONSE_SIZE];
        self.device.ioctl(IOCTL_ECTEST_PCC_EXECUTE, &input, &mut output)?;
        ExecuteResponse::try_from(output.as_slice()).map_err(|_| crate::windows::Error::InvalidData)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_response() -> Vec<u8> {
        [1u32, 0, 1, 0, 3, 4080, 1, 100_000, 0, 0xF61F_B868, 0xE1000, 0]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect()
    }

    #[test]
    fn decodes_native_interface_metadata() {
        let bytes = valid_response();
        let response = ProbeResponse::try_from(bytes.as_slice()).unwrap();
        assert_eq!(response.query_status, 0);
        assert_eq!(response.interface_version, 1);
        assert_eq!(response.subspace_id, 0);
        assert_eq!(response.subspace_type, 3);
        assert_eq!(response.subspace_size, 4080);
        assert_eq!(response.acpi_timestamp, 0xF61F_B868);
    }

    #[test]
    fn preserves_native_query_failure() {
        let mut bytes = [0u8; RESPONSE_SIZE];
        bytes[..4].copy_from_slice(&PROTOCOL_VERSION.to_le_bytes());
        bytes[4..8].copy_from_slice(&0xC000_00BBu32.to_le_bytes());
        let response = ProbeResponse::try_from(bytes.as_slice()).unwrap();
        assert_eq!(response.query_status as u32, 0xC000_00BB);
    }

    #[test]
    fn rejects_malformed_probe_responses() {
        let bytes = valid_response();
        for length in 0..bytes.len() {
            assert!(ProbeResponse::try_from(&bytes[..length]).is_err());
        }
        for offset in [0, 4, 8, 12, 16, 20, 24, 44] {
            let mut invalid = bytes.clone();
            invalid[offset..offset + 4].copy_from_slice(&2u32.to_le_bytes());
            assert!(ProbeResponse::try_from(invalid.as_slice()).is_err());
        }
        let mut oversized = bytes;
        oversized.push(0);
        assert!(ProbeResponse::try_from(oversized.as_slice()).is_err());
    }

    #[test]
    fn encodes_execute_request_layout() {
        let bytes = encode_execute(1, b"abc").unwrap();
        assert_eq!(&bytes[..16], &[1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 3, 0, 0, 0]);
        assert_eq!(&bytes[16..19], b"abc");
        assert!(bytes[19..].iter().all(|byte| *byte == 0));
        assert!(encode_execute(1, &[0; MESSAGE_SIZE]).is_ok());
        assert!(encode_execute(1, &[0; MESSAGE_SIZE + 1]).is_err());
    }

    #[test]
    fn decodes_execute_response_and_rejects_malformed_buffers() {
        let mut bytes = vec![0u8; EXECUTE_RESPONSE_SIZE];
        bytes[0..4].copy_from_slice(&PROTOCOL_VERSION.to_le_bytes());
        bytes[4..8].copy_from_slice(&0xC000_00B5u32.to_le_bytes());
        bytes[8] = 0x5A;
        let response = ExecuteResponse::try_from(bytes.as_slice()).unwrap();
        assert_eq!(response.status as u32, 0xC000_00B5);
        assert_eq!(response.message[0], 0x5A);
        assert!(ExecuteResponse::try_from(&bytes[..EXECUTE_RESPONSE_SIZE - 1]).is_err());
        bytes[0] = 2;
        assert!(ExecuteResponse::try_from(bytes.as_slice()).is_err());
    }
}
