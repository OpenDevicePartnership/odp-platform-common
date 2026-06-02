use crate::common;
use crate::common::SYMBOLS;
use crate::state::{Fetched, RtcState, TimerData};
use embedded_mcu_hal::time::Datetime;
use ratatui::{
    buffer::Buffer,
    crossterm::event::Event,
    layout::{Constraint, Layout, Rect},
    prelude::*,
    style::{Color, Style, Stylize, palette::tailwind},
    text::{Line, Span},
    widgets::{Block, Paragraph},
};
use time_alarm_service_interface::{
    AcpiDaylightSavingsTimeStatus, AcpiTimeZone, AcpiTimerId, AlarmExpiredWakePolicy, AlarmTimerSeconds,
    TimeAlarmDeviceCapabilities,
};

const LABEL_COLOR: Color = tailwind::VIOLET.c300;

// ── TimerData render helpers ──────────────────────────────────────────────────

impl TimerData {
    /// One-line summary for the dashboard card.
    pub(crate) fn summary(&self) -> String {
        match &self.value {
            None => "Pending...".to_string(),
            Some(Err(_)) => "Error".to_string(),
            Some(Ok(v)) => {
                let time_part = match *v {
                    AlarmTimerSeconds::DISABLED => "Not set".to_string(),
                    s => format!("{}s remaining", s.0),
                };
                let expired = if matches!(&self.timer_status, Some(Ok(s)) if s.timer_expired()) {
                    format!("  {} expired", SYMBOLS.warning)
                } else {
                    String::new()
                };
                format!("{time_part}{expired}")
            }
        }
    }

    pub(crate) fn render_panel(&self, title: &str, area: Rect, buf: &mut Buffer) {
        let is_healthy = matches!(self.value, Some(Ok(_)))
            && matches!(self.wake_policy, Some(Ok(_)))
            && matches!(self.timer_status, Some(Ok(_)));

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
                            "expired"
                        } else {
                            "not expired"
                        },
                        if timer_status.timer_triggered_wake() {
                            "triggered wake"
                        } else {
                            "did not trigger wake"
                        }
                    )
                },
            )),
        ])
        .block(common::title_block(
            common::status_title(title, is_healthy),
            0,
            LABEL_COLOR,
        ))
        .render(area, buf);
    }
}

// ── UI module ─────────────────────────────────────────────────────────────────

/// RTC UI module — stateless; all data is read from [`AppState`].
pub struct Rtc;

impl Rtc {
    pub fn new() -> Self {
        Self
    }
}

impl Rtc {
    pub(crate) fn handle_event(&mut self, _evt: &Event) {}

    pub(crate) fn render(&self, state: &RtcState, area: Rect, buf: &mut Buffer) {
        use Constraint::{Length, Min, Percentage};

        let rtc = state;
        let is_healthy = matches!(rtc.capabilities, Some(Ok(_))) && matches!(rtc.timestamp, Some(Ok(_)));

        let [time_area, bottom_area] = Layout::vertical([Length(5), Min(0)]).areas(area);
        let [caps_area, timers_area] = Layout::horizontal([Percentage(50), Percentage(50)]).areas(bottom_area);
        let [ac_area, dc_area] = Layout::vertical([Percentage(50), Percentage(50)]).areas(timers_area);

        self.render_time_display(state, time_area, buf, is_healthy);
        self.render_capabilities(state, caps_area, buf);
        rtc.timers[AcpiTimerId::AcPower as usize].render_panel("AC Power Timer", ac_area, buf);
        rtc.timers[AcpiTimerId::DcPower as usize].render_panel("DC Power Timer", dc_area, buf);
    }

    pub(crate) fn render_card(&self, state: &RtcState, area: Rect, buf: &mut Buffer) {
        use Constraint::{Length, Min};

        let rtc = state;
        let is_healthy = matches!(rtc.timestamp, Some(Ok(_)));

        let block = Block::bordered()
            .title(common::status_title("RTC", is_healthy))
            .border_style(tailwind::VIOLET.c700);
        let inner = block.inner(area);
        block.render(area, buf);

        let [time_area, meta_area, divider_area, timers_area] =
            Layout::vertical([Length(2), Length(2), Length(1), Min(0)]).areas(inner);

        // Time + date
        let (time_str, date_str, tz_str, dst_str) = match &rtc.timestamp {
            Some(Ok(ts)) => (
                format_time_hms(ts.datetime),
                format_date(ts.datetime),
                format_time_zone(ts.time_zone),
                format!("DST: {}", format_dst(ts.dst_status)),
            ),
            Some(Err(_)) => ("Error".into(), String::new(), String::new(), String::new()),
            None => ("Pending".into(), String::new(), String::new(), String::new()),
        };

        Paragraph::new(vec![
            Line::from(Span::styled(time_str, Style::default().fg(Color::White).bold())),
            Line::from(Span::styled(date_str, Style::default().fg(tailwind::VIOLET.c300))),
        ])
        .render(time_area, buf);

        let accuracy_str = match &rtc.capabilities {
            Some(Ok(caps)) => {
                if caps.realtime_accuracy_in_milliseconds() {
                    "ms accuracy"
                } else {
                    "s accuracy"
                }
            }
            _ => "",
        };
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(tz_str, Style::default().fg(tailwind::SLATE.c400)),
                Span::raw("  "),
                Span::styled(dst_str, Style::default().fg(tailwind::SLATE.c500)),
            ]),
            Line::from(Span::styled(accuracy_str, Style::default().fg(tailwind::SLATE.c600))),
        ])
        .render(meta_area, buf);

        let sep = SYMBOLS.h_line;
        Line::from(Span::styled(
            format!("{sep}{sep}{sep} Timers {}", sep.repeat(21)),
            Style::default().fg(tailwind::SLATE.c700),
        ))
        .render(divider_area, buf);

        Paragraph::new(vec![
            timer_summary_line("AC", &rtc.timers[AcpiTimerId::AcPower as usize]),
            timer_summary_line("DC", &rtc.timers[AcpiTimerId::DcPower as usize]),
        ])
        .render(timers_area, buf);
    }
}

// ── Render helpers ────────────────────────────────────────────────────────────

impl Rtc {
    fn render_time_display(&self, state: &RtcState, area: Rect, buf: &mut Buffer, is_healthy: bool) {
        let block = Block::bordered()
            .title(common::status_title("Real-Time Clock", is_healthy))
            .border_style(tailwind::VIOLET.c600);
        let inner = block.inner(area);
        block.render(area, buf);

        let lines: Vec<Line<'_>> = match &state.timestamp {
            None => vec![Line::raw("Pending...")],
            Some(Err(e)) => vec![Line::raw(format!("Error: {e}"))],
            Some(Ok(ts)) => vec![
                Line::from(Span::styled(
                    format_time_hms(ts.datetime),
                    Style::default().fg(Color::White).bold(),
                ))
                .centered(),
                Line::from(Span::styled(
                    format_date(ts.datetime),
                    Style::default().fg(tailwind::VIOLET.c300),
                ))
                .centered(),
                Line::from(vec![
                    Span::styled(
                        format_time_zone(ts.time_zone),
                        Style::default().fg(tailwind::SLATE.c400),
                    ),
                    Span::raw(format!("  {}  DST: ", SYMBOLS.mid_dot)),
                    Span::styled(format_dst(ts.dst_status), Style::default().fg(tailwind::SLATE.c400)),
                ])
                .centered(),
            ],
        };
        Paragraph::new(lines).render(inner, buf);
    }

    fn render_capabilities(&self, state: &RtcState, area: Rect, buf: &mut Buffer) {
        let lines: Vec<Line<'_>> = match &state.capabilities {
            None => vec![Line::raw("Pending...")],
            Some(Err(e)) => vec![Line::raw(format!("Error: {e}"))],
            Some(Ok(caps)) => format_capabilities(caps).into_iter().map(Line::raw).collect(),
        };
        let is_ok = matches!(state.capabilities, Some(Ok(_)));
        Paragraph::new(lines)
            .block(
                Block::bordered()
                    .title(common::status_title("Capabilities", is_ok))
                    .border_style(tailwind::VIOLET.c800),
            )
            .render(area, buf);
    }
}

// ── Free helper functions ─────────────────────────────────────────────────────

fn timer_summary_line<'a>(label: &'a str, timer: &TimerData) -> Line<'a> {
    common::metric_row(label, timer.summary(), tailwind::VIOLET.c400)
}

fn format_option_result<T>(label: &str, opt: &Fetched<T>, f: impl FnOnce(&T) -> String) -> String {
    match opt {
        None => format!("{label}Pending..."),
        Some(Ok(value)) => format!("{label}{}", f(value)),
        Some(Err(err)) => format!("{label}Error: {err}"),
    }
}

fn format_time_hms(time: Datetime) -> String {
    format!("{:02}:{:02}:{:02}", time.hour(), time.minute(), time.second())
}

fn format_date(time: Datetime) -> String {
    format!("{:04}-{:02}-{:02}", time.year(), u8::from(time.month()), time.day())
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

#[cfg(test)]
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_mcu_hal::time::{DatetimeFields, Month};
    use time_alarm_service_interface::AcpiTimeZoneOffset;

    fn make_datetime(year: u16, month: Month, day: u8, hour: u8, min: u8, sec: u8) -> Datetime {
        Datetime::new(DatetimeFields {
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
