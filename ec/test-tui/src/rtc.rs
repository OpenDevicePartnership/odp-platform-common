use crate::common;
use crossterm::event::Event;
use embedded_mcu_hal::time::Datetime;
use ratatui::{
    prelude::*,
    style::{Color, palette::tailwind},
    widgets::Paragraph,
};
use std::sync::Arc;
use time_alarm_service_messages::{
    AcpiDaylightSavingsTimeStatus, AcpiTimeZone, AcpiTimerId, AcpiTimestamp, AlarmExpiredWakePolicy, AlarmTimerSeconds,
    TimeAlarmDeviceCapabilities, TimerStatus,
};

use crate::app::Module;
use ec_test_lib::RtcSource;

const LABEL_COLOR: Color = tailwind::SLATE.c200;

mod rtc_timer {
    use super::*;
    pub struct RtcTimer {
        timer_id: AcpiTimerId,

        value: Option<color_eyre::Result<AlarmTimerSeconds>>,
        wake_policy: Option<color_eyre::Result<AlarmExpiredWakePolicy>>,
        timer_status: Option<color_eyre::Result<TimerStatus>>,
    }

    impl RtcTimer {
        pub fn update(&mut self, source: &impl RtcSource) {
            self.value = Some(source.get_timer_value(self.timer_id).map_err(Into::into));
            self.wake_policy = Some(source.get_expired_timer_wake_policy(self.timer_id).map_err(Into::into));
            self.timer_status = Some(source.get_wake_status(self.timer_id).map_err(Into::into));
        }

        pub fn new(timer_id: AcpiTimerId) -> Self {
            Self {
                timer_id,
                value: None,
                wake_policy: None,
                timer_status: None,
            }
        }

        pub fn render(&self, title: &str, area: Rect, buf: &mut Buffer) {
            let is_healthy = matches!(self.value, Some(Ok(_)))
                && matches!(self.wake_policy, Some(Ok(_)))
                && matches!(self.timer_status, Some(Ok(_)));
            let title = common::title_str_with_status(title, is_healthy);

            Paragraph::new(vec![
                Line::raw(format_option_result(
                    "Time remaining: ",
                    &self.value,
                    |value| match *value {
                        AlarmTimerSeconds::DISABLED => "Timer not set".to_string(),
                        seconds => format!("{} seconds", seconds.0),
                    },
                )),
                Line::raw(format_option_result(
                    "Wake policy:    ",
                    &self.wake_policy,
                    |wake_policy| match *wake_policy {
                        AlarmExpiredWakePolicy::NEVER => "never".to_string(),
                        AlarmExpiredWakePolicy::INSTANTLY => "instantly".to_string(),
                        wake_policy => format!("after {} seconds", wake_policy.0),
                    },
                )),
                Line::raw(format_option_result(
                    "Timer status:   ",
                    &self.timer_status,
                    |timer_status| {
                        format!(
                            "{}, {}",
                            if timer_status.timer_expired() {
                                "expired".to_string()
                            } else {
                                "not expired".to_string()
                            },
                            if timer_status.timer_triggered_wake() {
                                "triggered wake".to_string()
                            } else {
                                "did not trigger wake".to_string()
                            }
                        )
                    },
                )),
            ])
            .block(common::title_block(&title, 0, LABEL_COLOR))
            .render(area, buf);
        }
    }

    fn format_option_result<T>(
        label: &str,
        opt: &Option<color_eyre::Result<T>>,
        f: impl FnOnce(&T) -> String,
    ) -> String {
        match opt {
            None => format!("{label}Pending..."),
            Some(Ok(value)) => format!("{label}{}", f(value)),
            Some(Err(err)) => format!("{label}Error: {err}"),
        }
    }
}

use rtc_timer::RtcTimer;

pub struct Rtc<S: RtcSource> {
    source: Arc<S>,
    timers: [RtcTimer; 2],

    capabilities: Option<color_eyre::Result<TimeAlarmDeviceCapabilities>>,
    timestamp: Option<color_eyre::Result<AcpiTimestamp>>,
}

impl<S: RtcSource> Module for Rtc<S> {
    fn title(&self) -> &'static str {
        "RTC Information"
    }

    fn update(&mut self) {
        // Capabilities are static — keep retrying until we get a successful read.
        if !matches!(self.capabilities, Some(Ok(_))) {
            self.capabilities = Some(self.source.get_capabilities().map_err(Into::into));
        }
        self.timestamp = Some(self.source.get_real_time().map_err(Into::into));
        for timer in &mut self.timers {
            timer.update(&self.source);
        }
    }

    fn handle_event(&mut self, _evt: &Event) {}

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let is_healthy = matches!(self.capabilities, Some(Ok(_))) && matches!(self.timestamp, Some(Ok(_)));
        let title = common::title_str_with_status("Real-time Clock", is_healthy);
        let title = common::title_block(&title, 0, LABEL_COLOR);

        let [general_area, timers_area] = common::area_split(area, Direction::Vertical, 70, 30);
        let [ac_area, dc_area] = common::area_split(timers_area, Direction::Horizontal, 50, 50);

        let time_messages = match &self.timestamp {
            None => vec!["RTC time not yet retrieved".to_string()],
            Some(Ok(timestamp)) => vec![
                format!("Time:      {}", format_time(timestamp.datetime)),
                format!("Time Zone: {}", format_time_zone(timestamp.time_zone)),
                format!("DST:       {}", format_dst(timestamp.dst_status)),
                "".to_string(),
            ],
            Some(Err(err)) => vec![format!("Error retrieving RTC time: {}", err)],
        };

        let capabilities_messages: Vec<String> = match &self.capabilities {
            None => vec!["RTC capabilities not yet retrieved".to_string()],
            Some(Ok(capabilities)) => format_capabilities(capabilities),
            Some(Err(err)) => vec![format!("Error retrieving RTC capabilities: {}", err)],
        };

        let all_messages: Vec<Line<'_>> = time_messages
            .into_iter()
            .chain(capabilities_messages)
            .map(Line::raw)
            .collect();

        Paragraph::new(all_messages).block(title).render(general_area, buf);

        self.get_timer(AcpiTimerId::AcPower)
            .render("AC Power Timer", ac_area, buf);
        self.get_timer(AcpiTimerId::DcPower)
            .render("DC Power Timer", dc_area, buf);
    }
}

fn format_dst(dst: AcpiDaylightSavingsTimeStatus) -> &'static str {
    match dst {
        AcpiDaylightSavingsTimeStatus::NotObserved => "Not Observed",
        AcpiDaylightSavingsTimeStatus::NotAdjusted => "No",
        AcpiDaylightSavingsTimeStatus::Adjusted => "Yes",
    }
}

fn format_capabilities(capabilities: &TimeAlarmDeviceCapabilities) -> Vec<String> {
    fn as_supported(supported: bool) -> &'static str {
        if supported { "Supported" } else { "Not Supported" }
    }
    vec![
        "Capabilities:".to_string(),
        format!(
            "  Real time:       {}",
            as_supported(capabilities.realtime_implemented())
        ),
        format!(
            "  Get Wake Status: {}",
            as_supported(capabilities.get_wake_status_supported())
        ),
        format!(
            "  Accuracy:        {}",
            if capabilities.realtime_accuracy_in_milliseconds() {
                "Milliseconds"
            } else {
                "Seconds"
            }
        ),
        format!(
            "  AC Wake:         {}",
            as_supported(capabilities.ac_wake_implemented())
        ),
        format!(
            "  AC S4 Wake:      {}",
            as_supported(capabilities.ac_s4_wake_supported())
        ),
        format!(
            "  AC S5 Wake:      {}",
            as_supported(capabilities.ac_s5_wake_supported())
        ),
        format!(
            "  DC Wake:         {}",
            as_supported(capabilities.dc_wake_implemented())
        ),
        format!(
            "  DC S4 Wake:      {}",
            as_supported(capabilities.dc_s4_wake_supported())
        ),
        format!(
            "  DC S5 Wake:      {}",
            as_supported(capabilities.dc_s5_wake_supported())
        ),
    ]
}

fn format_time(time: Datetime) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        time.year(),
        u8::from(time.month()),
        time.day(),
        time.hour(),
        time.minute(),
        time.second()
    )
}

fn format_time_zone(tz: AcpiTimeZone) -> String {
    match tz {
        AcpiTimeZone::Unknown => "Unknown".to_string(),
        AcpiTimeZone::MinutesFromUtc(offset) => format!(
            "UTC{:+03}:{:02}",
            offset.minutes_from_utc() / 60,
            offset.minutes_from_utc().abs() % 60
        ),
    }
}

impl<S: RtcSource> Rtc<S> {
    pub fn new(source: Arc<S>) -> Self {
        let mut result = Self {
            source,
            capabilities: None,
            timestamp: None,
            timers: [RtcTimer::new(AcpiTimerId::AcPower), RtcTimer::new(AcpiTimerId::DcPower)],
        };

        result.update();
        result
    }

    fn get_timer(&self, timer_id: AcpiTimerId) -> &RtcTimer {
        &self.timers[timer_id as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_mcu_hal::time::{Month, UncheckedDatetime};
    use time_alarm_service_messages::AcpiTimeZoneOffset;

    fn make_datetime(year: u16, month: Month, day: u8, hour: u8, min: u8, sec: u8) -> Datetime {
        Datetime::new(UncheckedDatetime {
            year,
            month,
            day,
            hour,
            minute: min,
            second: sec,
            ..Default::default()
        })
        .expect("valid datetime")
    }

    // ── format_time ──────────────────────────────────────────────────────────

    #[test]
    fn format_time_produces_iso_like_string() {
        let dt = make_datetime(2024, Month::March, 15, 10, 30, 45);
        assert_eq!(format_time(dt), "2024-03-15 10:30:45");
    }

    #[test]
    fn format_time_pads_single_digit_fields() {
        let dt = make_datetime(2000, Month::January, 1, 0, 0, 0);
        assert_eq!(format_time(dt), "2000-01-01 00:00:00");
    }

    // ── format_time_zone ─────────────────────────────────────────────────────

    #[test]
    fn format_time_zone_unknown() {
        assert_eq!(format_time_zone(AcpiTimeZone::Unknown), "Unknown");
    }

    #[test]
    fn format_time_zone_negative_offset() {
        let offset = AcpiTimeZoneOffset::new(-8 * 60).expect("valid offset");
        assert_eq!(format_time_zone(AcpiTimeZone::MinutesFromUtc(offset)), "UTC-08:00");
    }

    #[test]
    fn format_time_zone_positive_half_hour_offset() {
        let offset = AcpiTimeZoneOffset::new(5 * 60 + 30).expect("valid offset");
        assert_eq!(format_time_zone(AcpiTimeZone::MinutesFromUtc(offset)), "UTC+05:30");
    }

    // ── format_dst ───────────────────────────────────────────────────────────

    #[test]
    fn format_dst_not_observed() {
        assert_eq!(format_dst(AcpiDaylightSavingsTimeStatus::NotObserved), "Not Observed");
    }

    #[test]
    fn format_dst_not_adjusted() {
        assert_eq!(format_dst(AcpiDaylightSavingsTimeStatus::NotAdjusted), "No");
    }

    #[test]
    fn format_dst_adjusted() {
        assert_eq!(format_dst(AcpiDaylightSavingsTimeStatus::Adjusted), "Yes");
    }

    // ── format_capabilities ──────────────────────────────────────────────────

    #[test]
    fn format_capabilities_has_correct_entry_count() {
        let caps = TimeAlarmDeviceCapabilities(0);
        let lines = format_capabilities(&caps);
        // Header + 9 capability entries.
        assert_eq!(lines.len(), 10);
    }

    #[test]
    fn format_capabilities_all_not_supported_when_zero() {
        let caps = TimeAlarmDeviceCapabilities(0);
        let lines = format_capabilities(&caps);
        // The accuracy entry (index 3) uses "Seconds"/"Milliseconds" — skip it.
        // All other data entries should say "Not Supported".
        for (i, line) in lines[1..].iter().enumerate() {
            if line.contains("Accuracy") {
                assert!(line.contains("Seconds"), "accuracy line unexpected: {line}");
            } else {
                assert!(
                    line.contains("Not Supported"),
                    "entry {i}: expected 'Not Supported' in: {line}"
                );
            }
        }
    }
}
