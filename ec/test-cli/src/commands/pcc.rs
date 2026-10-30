#[cfg(target_os = "windows")]
use crate::cli::PccCommand;
#[cfg(target_os = "windows")]
use ec_test_lib::pcc::Pcc;

fn ping_request(sequence: u32) -> [u8; 8] {
    let mut request = *b"PING\0\0\0\0";
    request[4..].copy_from_slice(&sequence.to_le_bytes());
    request
}

fn pong_sequence(message: &[u8], expected: u32) -> Result<u32, &'static str> {
    if message.len() < 8 || &message[..4] != b"PONG" {
        return Err("PCC reply is not PONG");
    }
    let sequence = u32::from_le_bytes([message[4], message[5], message[6], message[7]]);
    if sequence != expected {
        return Err("PCC PONG sequence mismatch");
    }
    Ok(sequence)
}

#[cfg(target_os = "windows")]
pub fn run(command: &PccCommand) -> Result<(), Box<dyn std::error::Error>> {
    let pcc = Pcc::new()?;
    match command {
        PccCommand::Probe => {
            let response = pcc.probe()?;
            println!(
                "PCC probe: status=0x{:08x} interface={} subspace={} type={} size={} flags=0x{:x} latency_us={} max_rate={} acpi_timestamp=0x{:08x} acpi_image_size=0x{:x}",
                response.query_status as u32,
                response.interface_version,
                response.subspace_id,
                response.subspace_type,
                response.subspace_size,
                response.flags,
                response.nominal_latency,
                response.maximum_periodic_rate,
                response.acpi_timestamp,
                response.acpi_image_size
            );
            if response.query_status < 0 {
                return Err(format!("PCC probe failed: NTSTATUS 0x{:08x}", response.query_status as u32).into());
            }
        }
        PccCommand::Ping { sequence } => {
            let sequence = match *sequence {
                Some(sequence) => sequence,
                None => std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .subsec_nanos(),
            };
            let response = pcc.execute(1, &ping_request(sequence))?;
            println!("PCC execute: status=0x{:08x}", response.status as u32);
            if response.status < 0 {
                return Err(format!("PCC execute failed: NTSTATUS 0x{:08x}", response.status as u32).into());
            }
            let reply_sequence = pong_sequence(&response.message, sequence)?;
            println!("PCC response: PONG sequence={reply_sequence}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_contains_marker_and_little_endian_sequence() {
        assert_eq!(ping_request(0x1234_5678), *b"PING\x78\x56\x34\x12");
    }

    #[test]
    fn pong_requires_matching_marker_and_sequence() {
        assert_eq!(pong_sequence(b"PONG\x78\x56\x34\x12", 0x1234_5678), Ok(0x1234_5678));
        assert!(pong_sequence(b"PING\x78\x56\x34\x12", 0x1234_5678).is_err());
        assert!(pong_sequence(b"PONG\x78\x56\x34\x12", 0x1234_5679).is_err());
        assert!(pong_sequence(b"PONG", 0x1234_5678).is_err());
    }
}
