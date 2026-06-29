use crate::{BatterySource, ErrorType, RtcSource, ThermalSource, Threshold, common};
use battery_service_interface::{BixFixedStrings, BstReturn, Btp};
use battery_service_relay::{AcpiBatteryRequest, AcpiBatteryResponse};
use embedded_services::relay::SerializableMessage;
use mctp_rs::{
    EC_EID, MctpMedium, MctpMessageHeaderTrait, MctpMessageTag, MctpMessageTrait, MctpPacketContext, MctpPacketError,
    MctpPacketResult, MctpReplyContext, MctpSequenceNumber, MctpSerialMedium, SP_EID,
};
use serialport::SerialPort;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use thermal_service_relay::{ThermalRequest, ThermalResponse};
use time_alarm_service_interface::{
    AcpiTimerId, AcpiTimestamp, AlarmExpiredWakePolicy, AlarmTimerSeconds, TimeAlarmDeviceCapabilities, TimerStatus,
};
use time_alarm_service_relay::{AcpiTimeAlarmRequest, AcpiTimeAlarmResponse};

/// Errors produced by serial data source operations.
#[derive(Debug)]
pub enum Error {
    /// Serial port I/O error (read, write, flush, clear)
    Io(String),
    /// Serial protocol framing error (invalid MCTP packet length, buffer overflow, etc.)
    Protocol(String),
    /// Message serialization or deserialization error
    Serialization(String),
    /// Response had an unexpected format
    UnexpectedResponse,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(msg) => write!(f, "serial I/O error: {msg}"),
            Self::Protocol(msg) => write!(f, "serial protocol error: {msg}"),
            Self::Serialization(msg) => write!(f, "serialization error: {msg}"),
            Self::UnexpectedResponse => write!(f, "unexpected response"),
        }
    }
}

impl std::error::Error for Error {}

impl crate::Error for Error {
    fn kind(&self) -> crate::ErrorKind {
        match self {
            Self::Io(_) => crate::ErrorKind::Io,
            Self::Protocol(_) => crate::ErrorKind::Protocol,
            Self::Serialization(_) => crate::ErrorKind::Serialization,
            Self::UnexpectedResponse => crate::ErrorKind::UnexpectedResponse,
        }
    }
}

// If it took longer than a second to receive a response, something is definitely wrong
const READ_TIMEOUT: Duration = Duration::from_millis(1000);

/// MCTP message-type byte for ODP relay traffic (matches the EC decoder in
/// `embedded_services::relay` and the SP encoder in `ec_relay.rs`).
const ODP_MESSAGE_TYPE: u8 = 0x7D;

/// DSP0253 serial framing END flag — one terminates each framed packet.
const SERIAL_END_FLAG: u8 = 0x7E;

/// Length of the 4-byte big-endian OdpHeader prefixed to each ODP body.
const ODP_HEADER_SZ: usize = 4;

/// Buffer size for request serialization, wire framing, and response
/// reassembly. Matches the EC uart-service `BUF_SIZE` (256): the EC rejects
/// any frame larger than this rather than emitting it, so a received frame can
/// never exceed it.
const BUFFER_SZ: usize = 256;

const THERMAL_VAR_LEN: u16 = 4;
const BATTERY_INSTANCE: u8 = 0;

#[derive(Clone, Copy, Debug)]
enum Destination {
    Battery,
    Thermal,
    TimeAlarm,
}

impl From<Destination> for u8 {
    fn from(dst: Destination) -> Self {
        match dst {
            Destination::Battery => 0x08,
            Destination::Thermal => 0x09,
            Destination::TimeAlarm => 0x0B,
        }
    }
}

/// 4 big-endian bytes of an OdpHeader, wrapped so
/// [`MctpPacketContext::serialize_packet`] frames them without a full
/// `SerializableMessage` impl. Mirrors the SP's shim in `ec_relay.rs`.
struct OdpRawHeader([u8; 4]);

impl MctpMessageHeaderTrait for OdpRawHeader {
    fn serialize<M: MctpMedium>(self, buffer: &mut [u8]) -> MctpPacketResult<usize, M> {
        if buffer.len() < ODP_HEADER_SZ {
            return Err(MctpPacketError::SerializeError("buffer too small for odp raw header"));
        }
        buffer[..ODP_HEADER_SZ].copy_from_slice(&self.0);
        Ok(ODP_HEADER_SZ)
    }

    fn deserialize<M: MctpMedium>(buffer: &[u8]) -> MctpPacketResult<(Self, &[u8]), M> {
        if buffer.len() < ODP_HEADER_SZ {
            return Err(MctpPacketError::HeaderParseError("buffer too small for odp raw header"));
        }
        let mut h = [0u8; 4];
        h.copy_from_slice(&buffer[..ODP_HEADER_SZ]);
        Ok((OdpRawHeader(h), &buffer[ODP_HEADER_SZ..]))
    }
}

/// ODP message body — the bytes following the OdpHeader.
struct OdpRawMessage<'b>(&'b [u8]);

impl<'buf> MctpMessageTrait<'buf> for OdpRawMessage<'buf> {
    type Header = OdpRawHeader;
    const MESSAGE_TYPE: u8 = ODP_MESSAGE_TYPE;

    fn serialize<M: MctpMedium>(self, buffer: &mut [u8]) -> MctpPacketResult<usize, M> {
        let n = self.0.len();
        if buffer.len() < n {
            return Err(MctpPacketError::SerializeError("buffer too small for odp raw body"));
        }
        buffer[..n].copy_from_slice(self.0);
        Ok(n)
    }

    fn deserialize<M: MctpMedium>(_h: &OdpRawHeader, buffer: &'buf [u8]) -> MctpPacketResult<Self, M> {
        Ok(OdpRawMessage(buffer))
    }
}

/// Build a 4-byte BE OdpHeader: bit 25 = is_request, bits 23..16 = service_id,
/// bit 15 = is_error (0 here), bits 14..0 = message_id. The command
/// discriminant rides in `message_id`; there is no separate command-code field.
fn build_odp_header(is_request: bool, service_id: u8, message_id: u16) -> [u8; 4] {
    let mut raw: u32 = 0;
    if is_request {
        raw |= 1 << 25;
    }
    raw |= (service_id as u32) << 16;
    raw |= (message_id as u32) & 0x7FFF;
    raw.to_be_bytes()
}

/// Parse a 4-byte BE OdpHeader into `(is_request, service_id, is_error, message_id)`.
fn parse_odp_header(bytes: &[u8]) -> Result<(bool, u8, bool, u16), Error> {
    if bytes.len() < ODP_HEADER_SZ {
        return Err(Error::Protocol("OdpHeader buffer < 4 bytes".into()));
    }
    let raw = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    Ok((
        (raw & (1 << 25)) != 0,
        ((raw >> 16) & 0xFF) as u8,
        (raw & (1 << 15)) != 0,
        (raw & 0x7FFF) as u16,
    ))
}

/// `MctpReplyContext` for a host→EC serial request. `MctpSerialMedium` has no
/// per-medium addressing; ec-test-cli drives the EC exactly as the SP does, so
/// it reuses the SP's `(SP_EID → EC_EID)` framing.
fn serial_reply_context() -> MctpReplyContext<MctpSerialMedium> {
    MctpReplyContext {
        source_endpoint_id: SP_EID,
        destination_endpoint_id: EC_EID,
        packet_sequence_number: MctpSequenceNumber::new(0),
        message_tag: MctpMessageTag::try_from(0).expect("tag 0 always valid"),
        medium_context: (),
    }
}

/// Read one DSP0253-framed packet (up to and including the next
/// [`SERIAL_END_FLAG`]) into `buf`; returns the number of bytes read.
/// Byte-stuffing guarantees the only unescaped `0x7E` is the frame terminator.
fn read_framed_packet(port: &mut dyn SerialPort, buf: &mut [u8]) -> Result<usize, Error> {
    let mut filled = 0usize;
    loop {
        if filled >= buf.len() {
            return Err(Error::Protocol("framed packet exceeds buffer".into()));
        }
        let mut byte = [0u8; 1];
        port.read_exact(&mut byte).map_err(|e| Error::Io(format!("{e}")))?;
        buf[filled] = byte[0];
        filled += 1;
        if byte[0] == SERIAL_END_FLAG {
            return Ok(filled);
        }
    }
}

#[derive(Clone)]
pub struct Serial {
    port: Arc<Mutex<Box<dyn SerialPort>>>,
    sensor_instance: u8,
    fan_instance: u8,
}

impl Serial {
    pub fn new(
        path: &str,
        baud_rate: u32,
        flow_control: bool,
        sensor_instance: u8,
        fan_instance: u8,
    ) -> Result<Self, Error> {
        let flow_control = if flow_control {
            serialport::FlowControl::Hardware
        } else {
            serialport::FlowControl::None
        };

        let port = serialport::new(path, baud_rate)
            .flow_control(flow_control)
            .timeout(READ_TIMEOUT)
            .open()
            .map_err(|e| Error::Io(format!("{e}")))?;
        port.clear(serialport::ClearBuffer::All)
            .map_err(|e| Error::Io(format!("{e}")))?;

        Ok(Self {
            port: Arc::new(Mutex::new(port)),
            sensor_instance,
            fan_instance,
        })
    }
}

impl Serial {
    fn send<REQ: SerializableMessage + Copy, RESP: SerializableMessage>(
        &self,
        dst: Destination,
        request: REQ,
    ) -> Result<RESP, Error> {
        // Serialize the ODP request body (the bytes after the 4-byte OdpHeader).
        let mut body_buf = [0u8; BUFFER_SZ];
        let body_len = request
            .serialize(&mut body_buf)
            .map_err(|e| Error::Serialization(format!("{e:?}")))?;
        let odp_header = build_odp_header(true, dst.into(), request.discriminant());

        let mut port = self
            .port
            .lock()
            .map_err(|_| Error::Io("serial port mutex poisoned".into()))?;

        // Clear any stale bytes left over if a previous call bailed mid-frame.
        port.clear(serialport::ClearBuffer::Input)
            .map_err(|e| Error::Io(format!("{e}")))?;

        // Serialize + DSP0253-frame the request via MctpPacketContext, writing
        // each emitted packet out the port. A dedicated TX buffer/context keeps
        // the response assembly buffer free.
        {
            let mut tx_buf = [0u8; BUFFER_SZ];
            let mut tx_ctx = MctpPacketContext::<MctpSerialMedium>::new(MctpSerialMedium, &mut tx_buf);
            let mut state = tx_ctx
                .serialize_packet(
                    serial_reply_context(),
                    (OdpRawHeader(odp_header), OdpRawMessage(&body_buf[..body_len])),
                )
                .map_err(|e| Error::Serialization(format!("{e:?}")))?;
            while let Some(pkt) = state.next() {
                let pkt = pkt.map_err(|e| Error::Serialization(format!("{e:?}")))?;
                port.write_all(pkt).map_err(|e| Error::Io(format!("{e}")))?;
            }
        }
        port.flush().map_err(|e| Error::Io(format!("{e}")))?;

        // Read framed packets until MctpPacketContext reassembles a complete
        // message — multi-packet responses return `None` until the EOM packet —
        // then parse the ODP response header and hand the payload to RESP.
        let mut assembly = [0u8; BUFFER_SZ];
        let mut rx_ctx = MctpPacketContext::<MctpSerialMedium>::new(MctpSerialMedium, &mut assembly);
        let response = loop {
            let mut rx_packet = [0u8; BUFFER_SZ];
            let n = read_framed_packet(&mut **port, &mut rx_packet)?;
            // Intermediate packets of a multi-packet response return `None`; keep
            // reading until the EOM packet yields the reassembled message.
            let Some(message) = rx_ctx
                .deserialize_packet(&rx_packet[..n])
                .map_err(|e| Error::Protocol(format!("{e:?}")))?
            else {
                continue;
            };
            if message.message_buffer.message_type() != ODP_MESSAGE_TYPE {
                return Err(Error::UnexpectedResponse);
            }
            let body = message.message_buffer.body();
            let (is_request, service_id, is_error, message_id) = parse_odp_header(body)?;
            // A reply must come back from the addressed service flagged as a
            // (non-error) response; otherwise the stream is desynced or the
            // EC reported a failure whose error payload we don't decode here.
            if is_request || service_id != u8::from(dst) {
                return Err(Error::UnexpectedResponse);
            }
            if is_error {
                return Err(Error::Protocol(format!(
                    "EC returned an error response (service {service_id:#04x}, message {message_id})"
                )));
            }
            // Deserialize while `body` still borrows the assembly buffer, so no
            // owned copy of the payload is needed.
            break RESP::deserialize(message_id, &body[ODP_HEADER_SZ..])
                .map_err(|e| Error::Serialization(format!("deserialization: {e:?}")))?;
        };

        Ok(response)
    }

    fn thermal_get_var(&self, guid: uuid::Uuid) -> Result<f64, Error> {
        let request = ThermalRequest::ThermalGetVarRequest {
            instance_id: self.fan_instance,
            len: THERMAL_VAR_LEN,
            var_uuid: guid.to_bytes_le(),
        };
        let response = self.send(Destination::Thermal, request)?;

        if let ThermalResponse::ThermalGetVarResponse { val } = response {
            Ok(val as f64)
        } else {
            Err(Error::UnexpectedResponse)
        }
    }

    fn thermal_set_var(&self, guid: uuid::Uuid, raw: u32) -> Result<(), Error> {
        let request = ThermalRequest::ThermalSetVarRequest {
            instance_id: self.fan_instance,
            len: THERMAL_VAR_LEN,
            var_uuid: guid.to_bytes_le(),
            set_var: raw,
        };
        let response = self.send(Destination::Thermal, request)?;

        if let ThermalResponse::ThermalSetVarResponse = response {
            Ok(())
        } else {
            Err(Error::UnexpectedResponse)
        }
    }
}

impl ErrorType for Serial {
    type Error = Error;
}

impl ThermalSource for Serial {
    fn get_temperature(&self) -> Result<f64, Self::Error> {
        let request = ThermalRequest::ThermalGetTmpRequest {
            instance_id: self.sensor_instance,
        };
        let response = self.send(Destination::Thermal, request)?;

        if let ThermalResponse::ThermalGetTmpResponse { temperature } = response {
            Ok(common::dk_to_c(temperature.0))
        } else {
            Err(Error::UnexpectedResponse)
        }
    }

    fn get_rpm(&self) -> Result<f64, Self::Error> {
        self.thermal_get_var(common::guid::FAN_CURRENT_RPM)
    }

    fn get_min_rpm(&self) -> Result<f64, Self::Error> {
        self.thermal_get_var(common::guid::FAN_MIN_RPM)
    }

    fn get_max_rpm(&self) -> Result<f64, Self::Error> {
        self.thermal_get_var(common::guid::FAN_MAX_RPM)
    }

    fn get_threshold(&self, threshold: Threshold) -> Result<f64, Self::Error> {
        let raw = match threshold {
            Threshold::On => self.thermal_get_var(common::guid::FAN_ON_TEMP),
            Threshold::Ramping => self.thermal_get_var(common::guid::FAN_RAMP_TEMP),
            Threshold::Max => self.thermal_get_var(common::guid::FAN_MAX_TEMP),
        }?;
        Ok(common::dk_to_c(raw as u32))
    }

    fn set_threshold(&self, threshold: Threshold, value: f64) -> Result<(), Self::Error> {
        let guid = match threshold {
            Threshold::On => common::guid::FAN_ON_TEMP,
            Threshold::Ramping => common::guid::FAN_RAMP_TEMP,
            Threshold::Max => common::guid::FAN_MAX_TEMP,
        };
        self.thermal_set_var(guid, common::c_to_dk(value))
    }

    fn set_rpm(&self, rpm: f64) -> Result<(), Self::Error> {
        self.thermal_set_var(common::guid::FAN_CURRENT_RPM, rpm as u32)
    }
}

impl BatterySource for Serial {
    fn get_bst(&self) -> Result<BstReturn, Self::Error> {
        let request = AcpiBatteryRequest::GetBst {
            battery_id: BATTERY_INSTANCE,
        };
        let response = self.send(Destination::Battery, request)?;

        if let AcpiBatteryResponse::GetBst { bst } = response {
            Ok(bst)
        } else {
            Err(Error::UnexpectedResponse)
        }
    }

    fn get_bix(&self) -> Result<BixFixedStrings, Self::Error> {
        let request = AcpiBatteryRequest::GetBix {
            battery_id: BATTERY_INSTANCE,
        };
        let response = self.send(Destination::Battery, request)?;

        if let AcpiBatteryResponse::GetBix { bix } = response {
            Ok(bix)
        } else {
            Err(Error::UnexpectedResponse)
        }
    }

    fn set_btp(&self, trip_point: u32) -> Result<(), Self::Error> {
        let request = AcpiBatteryRequest::SetBtp {
            battery_id: BATTERY_INSTANCE,
            btp: Btp { trip_point },
        };
        let response = self.send(Destination::Battery, request)?;

        if matches!(response, AcpiBatteryResponse::SetBtp {}) {
            Ok(())
        } else {
            Err(Error::UnexpectedResponse)
        }
    }
}

impl RtcSource for Serial {
    fn get_capabilities(&self) -> Result<TimeAlarmDeviceCapabilities, Self::Error> {
        let request = AcpiTimeAlarmRequest::GetCapabilities;
        let response = self.send(Destination::TimeAlarm, request)?;

        if let AcpiTimeAlarmResponse::Capabilities(capabilities) = response {
            Ok(capabilities)
        } else {
            Err(Error::UnexpectedResponse)
        }
    }

    fn get_real_time(&self) -> Result<AcpiTimestamp, Self::Error> {
        let request = AcpiTimeAlarmRequest::GetRealTime;
        let response = self.send(Destination::TimeAlarm, request)?;

        if let AcpiTimeAlarmResponse::RealTime(timestamp) = response {
            Ok(timestamp)
        } else {
            Err(Error::UnexpectedResponse)
        }
    }

    fn get_wake_status(&self, timer_id: AcpiTimerId) -> Result<TimerStatus, Self::Error> {
        let request = AcpiTimeAlarmRequest::GetWakeStatus(timer_id);
        let response = self.send(Destination::TimeAlarm, request)?;

        if let AcpiTimeAlarmResponse::TimerStatus(status) = response {
            Ok(status)
        } else {
            Err(Error::UnexpectedResponse)
        }
    }

    fn get_expired_timer_wake_policy(&self, timer_id: AcpiTimerId) -> Result<AlarmExpiredWakePolicy, Self::Error> {
        let request = AcpiTimeAlarmRequest::GetExpiredTimerPolicy(timer_id);
        let response = self.send(Destination::TimeAlarm, request)?;

        if let AcpiTimeAlarmResponse::WakePolicy(policy) = response {
            Ok(policy)
        } else {
            Err(Error::UnexpectedResponse)
        }
    }

    fn get_timer_value(&self, timer_id: AcpiTimerId) -> Result<AlarmTimerSeconds, Self::Error> {
        let request = AcpiTimeAlarmRequest::GetTimerValue(timer_id);
        let response = self.send(Destination::TimeAlarm, request)?;

        if let AcpiTimeAlarmResponse::TimerSeconds(seconds) = response {
            Ok(seconds)
        } else {
            Err(Error::UnexpectedResponse)
        }
    }

    fn set_real_time(&self, timestamp: AcpiTimestamp) -> Result<(), Self::Error> {
        let response = self.send(Destination::TimeAlarm, AcpiTimeAlarmRequest::SetRealTime(timestamp))?;
        if matches!(response, AcpiTimeAlarmResponse::OkNoData) {
            Ok(())
        } else {
            Err(Error::UnexpectedResponse)
        }
    }

    fn set_timer_value(&self, timer_id: AcpiTimerId, value: AlarmTimerSeconds) -> Result<(), Self::Error> {
        let response = self.send(
            Destination::TimeAlarm,
            AcpiTimeAlarmRequest::SetTimerValue(timer_id, value),
        )?;
        if matches!(response, AcpiTimeAlarmResponse::OkNoData) {
            Ok(())
        } else {
            Err(Error::UnexpectedResponse)
        }
    }

    fn set_expired_timer_wake_policy(
        &self,
        timer_id: AcpiTimerId,
        policy: AlarmExpiredWakePolicy,
    ) -> Result<(), Self::Error> {
        let response = self.send(
            Destination::TimeAlarm,
            AcpiTimeAlarmRequest::SetExpiredTimerPolicy(timer_id, policy),
        )?;
        if matches!(response, AcpiTimeAlarmResponse::OkNoData) {
            Ok(())
        } else {
            Err(Error::UnexpectedResponse)
        }
    }

    fn clear_wake_status(&self, timer_id: AcpiTimerId) -> Result<(), Self::Error> {
        let response = self.send(Destination::TimeAlarm, AcpiTimeAlarmRequest::ClearWakeStatus(timer_id))?;
        if matches!(response, AcpiTimeAlarmResponse::OkNoData) {
            Ok(())
        } else {
            Err(Error::UnexpectedResponse)
        }
    }
}
