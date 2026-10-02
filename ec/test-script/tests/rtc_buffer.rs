// SPDX-License-Identifier: MIT

use std::cell::Cell;

use battery_service_interface::{BixFixedStrings, BstReturn};
use ec_test_lib::{BatterySource, ErrorKind, ErrorType, RtcSource, ThermalSource, Threshold, mock::Mock};
use ec_test_script::run_script;
use time_alarm_service_interface::{
    AcpiTimerId, AcpiTimestamp, AlarmExpiredWakePolicy, AlarmTimerSeconds, TimeAlarmDeviceCapabilities, TimerStatus,
};

#[test]
fn set_real_time_passes_the_decoded_timestamp_to_source() {
    for (milliseconds, daylight, pad1) in [(0u16, 0, 0), (999, 1, 1), (0, 3, 255)] {
        let [ms_lo, ms_hi] = milliseconds.to_le_bytes();
        let mut bytes = [
            234, 7, 9, 17, 12, 34, 56, pad1, ms_lo, ms_hi, 0x20, 0xFE, daylight, 0, 0, 0,
        ];
        let initializer = bytes.map(|b| b.to_string()).join(", ");
        let mock = Mock::default();
        let summary = run_script(
            &mock,
            &format!("rtc.set_real_time(Buffer(16) {{ {initializer} }}) => is_ok;"),
        )
        .unwrap();
        assert_eq!((summary.passed, summary.failed), (1, 0));
        let timestamp = mock.get_real_time().unwrap();
        assert_eq!(timestamp, AcpiTimestamp::try_from_bytes(&bytes).unwrap());
        assert_eq!(timestamp.datetime.nanoseconds(), u32::from(milliseconds) * 1_000_000);
        // The existing typed serializer emits the _GRT valid byte, not the input Pad1.
        bytes[7] = 1;
        assert_eq!(timestamp.as_bytes(), bytes);
    }
}

#[test]
fn source_failure_fails_is_ok_and_passes_is_err() {
    let source = SetRealTimeFailure(Cell::new(0));
    let call = "rtc.set_real_time(Buffer(16) { 234,7,9,17,12,0,0,0,0,0,0,0,0,0,0,0 })";
    for (verb, expected) in [("is_ok", (0, 1)), ("is_err", (1, 0))] {
        let summary = run_script(&source, &format!("{call} => {verb};")).unwrap();
        assert_eq!((summary.passed, summary.failed), expected, "{verb}");
    }
    assert!(
        run_script(
            &source,
            &format!("{call} => is_ok; rtc.set_real_time(Buffer(16) {{ 0 }}) => is_err;"),
        )
        .is_err()
    );
    assert_eq!(source.0.get(), 2);
}

struct SetRealTimeFailure(Cell<usize>);

impl ErrorType for SetRealTimeFailure {
    type Error = ErrorKind;
}

impl RtcSource for SetRealTimeFailure {
    fn set_real_time(&self, _: AcpiTimestamp) -> Result<(), Self::Error> {
        self.0.set(self.0.get() + 1);
        Err(ErrorKind::Io)
    }
    fn get_capabilities(&self) -> Result<TimeAlarmDeviceCapabilities, Self::Error> {
        unreachable!()
    }
    fn get_real_time(&self) -> Result<AcpiTimestamp, Self::Error> {
        unreachable!()
    }
    fn get_wake_status(&self, _: AcpiTimerId) -> Result<TimerStatus, Self::Error> {
        unreachable!()
    }
    fn get_expired_timer_wake_policy(&self, _: AcpiTimerId) -> Result<AlarmExpiredWakePolicy, Self::Error> {
        unreachable!()
    }
    fn get_timer_value(&self, _: AcpiTimerId) -> Result<AlarmTimerSeconds, Self::Error> {
        unreachable!()
    }
    fn set_timer_value(&self, _: AcpiTimerId, _: AlarmTimerSeconds) -> Result<(), Self::Error> {
        unreachable!()
    }
    fn set_expired_timer_wake_policy(&self, _: AcpiTimerId, _: AlarmExpiredWakePolicy) -> Result<(), Self::Error> {
        unreachable!()
    }
    fn clear_wake_status(&self, _: AcpiTimerId) -> Result<(), Self::Error> {
        unreachable!()
    }
}

impl ThermalSource for SetRealTimeFailure {
    fn get_temperature(&self) -> Result<f64, Self::Error> {
        unreachable!()
    }
    fn get_rpm(&self) -> Result<f64, Self::Error> {
        unreachable!()
    }
    fn get_min_rpm(&self) -> Result<f64, Self::Error> {
        unreachable!()
    }
    fn get_max_rpm(&self) -> Result<f64, Self::Error> {
        unreachable!()
    }
    fn get_threshold(&self, _: Threshold) -> Result<f64, Self::Error> {
        unreachable!()
    }
    fn set_threshold(&self, _: Threshold, _: f64) -> Result<(), Self::Error> {
        unreachable!()
    }
    fn set_rpm(&self, _: f64) -> Result<(), Self::Error> {
        unreachable!()
    }
}

impl BatterySource for SetRealTimeFailure {
    fn get_bst(&self) -> Result<BstReturn, Self::Error> {
        unreachable!()
    }
    fn get_bix(&self) -> Result<BixFixedStrings, Self::Error> {
        unreachable!()
    }
    fn set_btp(&self, _: u32) -> Result<(), Self::Error> {
        unreachable!()
    }
}
