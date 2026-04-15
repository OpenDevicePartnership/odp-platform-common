use crate::common;
use crate::state::{AppState, BatteryCommand, BatteryState};
use crate::widgets::battery;
use battery_service_messages::{
    BatteryState as BatteryStateFlag, BatterySwapCapability, BatteryTechnology, PowerUnit,
};
use core::ffi::CStr;
use ec_test_lib::BatterySource;
use std::sync::mpsc;
use tracing::{debug, instrument, warn};

use ratatui::style::Modifier;
use ratatui::text::Text;
use ratatui::widgets::{Row, StatefulWidget, Table, Widget};
use ratatui::{
    buffer::Buffer,
    crossterm::event::{Event, KeyCode, KeyEventKind},
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Stylize, palette::tailwind},
    text::{Line, Span},
    widgets::{Block, Gauge, Paragraph},
};
use tui_input::{Input, backend::crossterm::EventHandler};

const BATGAUGE_COLOR_HIGH: Color = tailwind::GREEN.c400;
const BATGAUGE_COLOR_MEDIUM: Color = tailwind::AMBER.c400;
const BATGAUGE_COLOR_LOW: Color = tailwind::RED.c400;
const LABEL_COLOR: Color = tailwind::SKY.c300;

fn str_from_bytes(bytes: &[u8]) -> String {
    CStr::from_bytes_until_nul(bytes)
        .ok()
        .and_then(|c| c.to_str().ok())
        .unwrap_or("Invalid")
        .to_owned()
}

#[cfg(test)]
fn charge_state_as_str(state: BatteryStateFlag) -> &'static str {
    if state.contains(BatteryStateFlag::DISCHARGING) {
        "Discharging"
    } else {
        "Charging"
    }
}

fn power_unit_as_capacity_str(power_unit: PowerUnit) -> &'static str {
    match power_unit {
        PowerUnit::MilliWatts => "mWh",
        PowerUnit::MilliAmps => "mAh",
    }
}

fn power_unit_as_rate_str(power_unit: PowerUnit) -> &'static str {
    match power_unit {
        PowerUnit::MilliWatts => "mW",
        PowerUnit::MilliAmps => "mA",
    }
}

fn bat_tech_as_str(bat_tech: BatteryTechnology) -> &'static str {
    match bat_tech {
        BatteryTechnology::Primary => "Primary",
        BatteryTechnology::Secondary => "Secondary",
    }
}

fn swap_cap_as_str(swap_cap: BatterySwapCapability) -> &'static str {
    match swap_cap {
        BatterySwapCapability::NonSwappable => "Non swappable",
        BatterySwapCapability::ColdSwappable => "Cold swappable",
        BatterySwapCapability::HotSwappable => "Hot swappable",
    }
}

// ── Fetch helpers (used by the background updater) ────────────────────────────

/// Fetch the latest BST reading into `state`.
#[instrument(skip_all)]
pub(crate) fn poll_bst(state: &mut BatteryState, source: &impl BatterySource) {
    match source.get_bst() {
        Ok(bst) => {
            debug!(
                remaining_capacity = bst.battery_remaining_capacity,
                voltage_mv = bst.battery_present_voltage,
                "BST read OK"
            );
            state.bst = bst;
            state.bst_success = true;
        }
        Err(e) => {
            warn!(error = %e, "BST read failed");
            state.bst_success = false;
        }
    }
}

/// Fetch static BIX info into `state` (call until `state.bix_success` is true).
#[instrument(skip_all)]
pub(crate) fn poll_bix(state: &mut BatteryState, source: &impl BatterySource) {
    match source.get_bix() {
        Ok(bix) => {
            debug!("BIX read OK");
            state.bix = bix;
            state.bix_success = true;
        }
        Err(e) => {
            warn!(error = %e, "BIX read failed");
            state.bix_success = false;
        }
    }
}

// ── UI module ─────────────────────────────────────────────────────────────────

/// Battery UI module.  Holds only UI-local state: the BTP text input and the
/// command channel for sending write-backs to the background updater.
pub struct Battery {
    btp_input: Input,
    cmd_tx: mpsc::Sender<BatteryCommand>,
}

impl Battery {
    pub fn new(cmd_tx: mpsc::Sender<BatteryCommand>) -> Self {
        Self {
            btp_input: Input::default(),
            cmd_tx,
        }
    }
}

impl Battery {
    pub(crate) fn handle_event(&mut self, evt: &Event) {
        if let Event::Key(key) = evt
            && key.code == KeyCode::Enter
            && key.kind == KeyEventKind::Press
        {
            let raw = self.btp_input.value_and_reset();
            if let Ok(btp) = raw.parse::<u32>() {
                debug!(btp, "user requested battery trip-point change");
                let _ = self.cmd_tx.send(BatteryCommand::SetBtp(btp));
            } else {
                warn!(input = raw, "invalid BTP value; expected a non-negative integer");
            }
        } else {
            let _ = self.btp_input.handle_event(evt);
        }
    }

    pub(crate) fn render(&self, state: &AppState, area: Rect, buf: &mut Buffer) {
        use Constraint::{Min, Percentage};
        let [strip_area, bottom_area] =
            Layout::vertical([Percentage(22), Min(0)]).areas(area);
        let [bix_area, chart_area] =
            Layout::horizontal([Percentage(50), Percentage(50)]).areas(bottom_area);

        self.render_status_strip(state, strip_area, buf);
        self.render_bix(state, bix_area, buf);
        self.render_bst_chart(state, chart_area, buf);
    }

    pub(crate) fn render_card(&self, state: &AppState, area: Rect, buf: &mut Buffer) {
        let bat = &state.battery;
        let is_charging = bat.bst.battery_state.contains(BatteryStateFlag::CHARGING);
        let state_str = if is_charging { "▲ Charging" } else { "▼ Discharging" };
        let state_color = if is_charging { tailwind::GREEN.c400 } else { tailwind::AMBER.c400 };

        let bat_pct = bat_percent(bat.bst.battery_remaining_capacity, bat.bix.design_capacity);
        let health_pct = bat_health(bat.bix.last_full_charge_capacity, bat.bix.design_capacity);
        let cap_str = power_unit_as_capacity_str(bat.bix.power_unit);
        let rate_str = power_unit_as_rate_str(bat.bix.power_unit);
        let voltage_v = bat.bst.battery_present_voltage as f64 / 1000.0;

        let block = Block::bordered()
            .title(common::status_title("Battery", bat.bst_success))
            .border_style(tailwind::SKY.c700);
        let inner = block.inner(area);
        block.render(area, buf);

        let [state_area, gauge_area, details_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(0)])
                .areas(inner);

        Line::from(vec![
            Span::styled(state_str, Style::default().fg(state_color).bold()),
            Span::raw("  "),
            Span::styled(format!("{bat_pct}%"), Style::default().fg(Color::White).bold()),
        ])
        .render(state_area, buf);

        let gauge_color = bat_gauge_color(
            bat.bst.battery_remaining_capacity,
            bat.bix.design_cap_of_warning,
            bat.bix.design_cap_of_low,
        );
        Gauge::default()
            .gauge_style(gauge_color)
            .percent(bat_pct)
            .render(gauge_area, buf);

        let time_hint = estimate_time(
            bat.bst.battery_remaining_capacity,
            bat.bix.design_capacity,
            bat.bix.last_full_charge_capacity,
            bat.bst.battery_present_rate,
            is_charging,
        );
        let rate_line = match time_hint {
            Some(hint) => format!("{} {}  ({})", bat.bst.battery_present_rate, rate_str, hint),
            None => format!("{} {}", bat.bst.battery_present_rate, rate_str),
        };

        Paragraph::new(vec![
            common::metric_row(
                "Remaining",
                format!(
                    "{} / {} {}",
                    bat.bst.battery_remaining_capacity, bat.bix.design_capacity, cap_str
                ),
                LABEL_COLOR,
            ),
            common::metric_row("Rate     ", rate_line, LABEL_COLOR),
            common::metric_row("Voltage  ", format!("{voltage_v:.2} V"), LABEL_COLOR),
            common::metric_row(
                "Health   ",
                format!("{health_pct}%"),
                health_color(health_pct),
            ),
            common::metric_row("Cycles   ", format!("{}", bat.bix.cycle_count), LABEL_COLOR),
        ])
        .render(details_area, buf);
    }
}

// ── Pure computation helpers ──────────────────────────────────────────────────

fn bat_percent(remaining: u32, design: u32) -> u16 {
    (remaining * 100).checked_div(design).unwrap_or(0).clamp(0, 100) as u16
}

fn bat_gauge_color(remaining: u32, warning: u32, low: u32) -> Color {
    if remaining <= low {
        BATGAUGE_COLOR_LOW
    } else if remaining <= warning {
        BATGAUGE_COLOR_MEDIUM
    } else {
        BATGAUGE_COLOR_HIGH
    }
}

fn bat_health(last_full: u32, design: u32) -> u16 {
    (last_full * 100).checked_div(design).unwrap_or(0).clamp(0, 100) as u16
}

fn health_color(health_pct: u16) -> Color {
    if health_pct >= 80 {
        tailwind::GREEN.c400
    } else if health_pct >= 60 {
        tailwind::AMBER.c400
    } else {
        tailwind::RED.c500
    }
}

/// Returns a human-readable estimate of time to full (charging) or to empty (discharging).
/// Returns `None` when the rate is zero or the data is unavailable.
fn estimate_time(remaining: u32, design: u32, last_full: u32, rate: u32, is_charging: bool) -> Option<String> {
    if rate == 0 {
        return None;
    }
    let target = if is_charging { last_full.max(design) } else { 0 };
    let delta = if is_charging {
        target.saturating_sub(remaining)
    } else {
        remaining
    };
    let minutes = (delta as f64 / rate as f64 * 60.0).round() as u64;
    let h = minutes / 60;
    let m = minutes % 60;
    let label = if is_charging { "to full" } else { "to empty" };
    Some(if h > 0 {
        format!("~{h}h {m:02}m {label}")
    } else {
        format!("~{m}m {label}")
    })
}

// ── Render helpers (private, methods on Battery) ──────────────────────────────

impl Battery {
    fn render_status_strip(&self, state: &AppState, area: Rect, buf: &mut Buffer) {
        use Constraint::{Length, Min};
        let bat = &state.battery;
        let is_charging = bat.bst.battery_state.contains(BatteryStateFlag::CHARGING);
        let state_str = if is_charging { "▲ Charging" } else { "▼ Discharging" };
        let state_color = if is_charging { tailwind::GREEN.c400 } else { tailwind::AMBER.c400 };
        let pct = bat_percent(bat.bst.battery_remaining_capacity, bat.bix.design_capacity);
        let cap_str = power_unit_as_capacity_str(bat.bix.power_unit);
        let rate_str = power_unit_as_rate_str(bat.bix.power_unit);

        let block = Block::bordered()
            .title(
                Line::from(vec![
                    Span::styled(state_str, Style::default().fg(state_color).bold()),
                    Span::styled(
                        format!("  {pct}%"),
                        Style::default().fg(Color::White).bold(),
                    ),
                ])
            )
            .border_style(tailwind::SKY.c600);
        let inner = block.inner(area);
        block.render(area, buf);

        let [left_area, bat_area] = Layout::horizontal([Min(0), Length(6)]).areas(inner);
        let [gauge_area, details_area] = Layout::vertical([Length(1), Min(0)]).areas(left_area);

        let gauge_color = bat_gauge_color(
            bat.bst.battery_remaining_capacity,
            bat.bix.design_cap_of_warning,
            bat.bix.design_cap_of_low,
        );
        Gauge::default()
            .gauge_style(gauge_color)
            .percent(pct)
            .render(gauge_area, buf);

        Paragraph::new(vec![
            common::metric_row(
                "Remaining ",
                format!(
                    "{} / {} {}",
                    bat.bst.battery_remaining_capacity, bat.bix.design_capacity, cap_str
                ),
                LABEL_COLOR,
            ),
            common::metric_row(
                "Rate      ",
                format!("{} {}", bat.bst.battery_present_rate, rate_str),
                LABEL_COLOR,
            ),
            common::metric_row(
                "Voltage   ",
                format!("{} mV", bat.bst.battery_present_voltage),
                LABEL_COLOR,
            ),
        ])
        .render(details_area, buf);

        let mut bat_widget_state = battery::BatteryState::new(
            bat.bst.battery_remaining_capacity,
            is_charging,
        );
        battery::Battery::default()
            .color_high(BATGAUGE_COLOR_HIGH)
            .color_warning(BATGAUGE_COLOR_MEDIUM)
            .color_low(BATGAUGE_COLOR_LOW)
            .design_capacity(bat.bix.design_capacity)
            .warning_capacity(bat.bix.design_cap_of_warning)
            .low_capacity(bat.bix.design_cap_of_low)
            .render(bat_area, buf, &mut bat_widget_state);
    }

    fn render_bst_chart(&self, state: &AppState, area: Rect, buf: &mut Buffer) {
        let bat = &state.battery;
        let y_labels = [
            "0".bold(),
            Span::styled(
                format!("{}", bat.bix.design_capacity / 2),
                Style::default().bold(),
            ),
            Span::styled(format!("{}", bat.bix.design_capacity), Style::default().bold()),
        ];
        let graph = common::Graph {
            title: "Capacity vs Time".to_string(),
            color: tailwind::SKY.c400,
            samples: bat.samples.get(),
            x_axis: "Time (m)".to_string(),
            x_bounds: [0.0, 60.0],
            x_labels: common::time_labels(bat.t_min, crate::state::BATTERY_MAX_SAMPLES),
            y_axis: format!("Capacity ({})", power_unit_as_capacity_str(bat.bix.power_unit)),
            y_bounds: [0.0, bat.bix.design_capacity as f64],
            y_labels,
        };
        common::render_chart(area, buf, graph);
    }

    fn bix_rows(&self, bat: &BatteryState) -> Vec<Row<'static>> {
        let power_unit = bat.bix.power_unit;
        let cap = power_unit_as_capacity_str(power_unit);
        let rate = power_unit_as_rate_str(power_unit);

        vec![
            Row::new(vec![
                Text::raw("Revision").add_modifier(Modifier::BOLD),
                format!("{}", bat.bix.revision).into(),
            ]),
            Row::new(vec![
                Text::raw("Power Unit").add_modifier(Modifier::BOLD),
                rate.into(),
            ]),
            Row::new(vec![
                Text::raw("Design Capacity").add_modifier(Modifier::BOLD),
                format!("{} {}", bat.bix.design_capacity, cap).into(),
            ]),
            Row::new(vec![
                Text::raw("Last Full Capacity").add_modifier(Modifier::BOLD),
                format!("{} {}", bat.bix.last_full_charge_capacity, cap).into(),
            ]),
            Row::new(vec![
                Text::raw("Technology").add_modifier(Modifier::BOLD),
                bat_tech_as_str(bat.bix.battery_technology).into(),
            ]),
            Row::new(vec![
                Text::raw("Design Voltage").add_modifier(Modifier::BOLD),
                format!("{} mV", bat.bix.design_voltage).into(),
            ]),
            Row::new(vec![
                Text::raw("Warning Capacity").add_modifier(Modifier::BOLD),
                format!("{} {}", bat.bix.design_cap_of_warning, cap).into(),
            ]),
            Row::new(vec![
                Text::raw("Low Capacity").add_modifier(Modifier::BOLD),
                format!("{} {}", bat.bix.design_cap_of_low, cap).into(),
            ]),
            Row::new(vec![
                Text::raw("Cycle Count").add_modifier(Modifier::BOLD),
                format!("{}", bat.bix.cycle_count).into(),
            ]),
            Row::new(vec![
                Text::raw("Accuracy").add_modifier(Modifier::BOLD),
                format!("{}%", bat.bix.measurement_accuracy as f64 / 1000.0).into(),
            ]),
            Row::new(vec![
                Text::raw("Max Sample Time").add_modifier(Modifier::BOLD),
                format!("{} ms", bat.bix.max_sampling_time).into(),
            ]),
            Row::new(vec![
                Text::raw("Min Sample Time").add_modifier(Modifier::BOLD),
                format!("{} ms", bat.bix.min_sampling_time).into(),
            ]),
            Row::new(vec![
                Text::raw("Max Avg Interval").add_modifier(Modifier::BOLD),
                format!("{} ms", bat.bix.max_averaging_interval).into(),
            ]),
            Row::new(vec![
                Text::raw("Min Avg Interval").add_modifier(Modifier::BOLD),
                format!("{} ms", bat.bix.min_averaging_interval).into(),
            ]),
            Row::new(vec![
                Text::raw("Cap. Granularity 1").add_modifier(Modifier::BOLD),
                format!("{} {}", bat.bix.battery_capacity_granularity_1, cap).into(),
            ]),
            Row::new(vec![
                Text::raw("Cap. Granularity 2").add_modifier(Modifier::BOLD),
                format!("{} {}", bat.bix.battery_capacity_granularity_2, cap).into(),
            ]),
            Row::new(vec![
                Text::raw("Model Number").add_modifier(Modifier::BOLD),
                str_from_bytes(&bat.bix.model_number).into(),
            ]),
            Row::new(vec![
                Text::raw("Serial Number").add_modifier(Modifier::BOLD),
                str_from_bytes(&bat.bix.serial_number).into(),
            ]),
            Row::new(vec![
                Text::raw("Battery Type").add_modifier(Modifier::BOLD),
                str_from_bytes(&bat.bix.battery_type).into(),
            ]),
            Row::new(vec![
                Text::raw("OEM Info").add_modifier(Modifier::BOLD),
                str_from_bytes(&bat.bix.oem_info).into(),
            ]),
            Row::new(vec![
                Text::raw("Swap Capability").add_modifier(Modifier::BOLD),
                swap_cap_as_str(bat.bix.battery_swapping_capability).into(),
            ]),
        ]
    }

    fn render_bix(&self, state: &AppState, area: Rect, buf: &mut Buffer) {
        use Constraint::{Length, Min};
        let bat = &state.battery;
        let [table_area, btp_area] =
            Layout::vertical([Min(0), Length(3u16)]).areas(area);

        let table = Table::new(self.bix_rows(bat), [Constraint::Min(22), Constraint::Fill(1)])
            .block(
                Block::bordered()
                    .title(common::status_title("Battery Info (BIX)", bat.bix_success))
                    .fg(LABEL_COLOR),
            )
            .style(Style::new().white());
        Widget::render(table, table_area, buf);

        self.render_btp_input(bat, btp_area, buf);
    }

    fn render_btp_input(&self, bat: &BatteryState, area: Rect, buf: &mut Buffer) {
        let width = area.width.max(3) - 3;
        let scroll = self.btp_input.visual_scroll(width as usize);
        let title = format!(
            "Trippoint: {} {}  <set new value + Enter>",
            bat.btp,
            power_unit_as_capacity_str(bat.bix.power_unit)
        );
        let dot = if bat.btp_success {
            Span::styled("● ", Style::default().fg(tailwind::GREEN.c400))
        } else {
            Span::styled("● ", Style::default().fg(tailwind::RED.c500))
        };
        let block_title = Line::from(vec![dot, Span::raw(title)]);

        Paragraph::new(self.btp_input.value())
            .scroll((0, scroll as u16))
            .block(
                Block::bordered()
                    .title(block_title)
                    .border_style(Style::default().fg(tailwind::SKY.c700)),
            )
            .render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::test_support::TestError;
    use battery_service_messages::{BatterySwapCapability, BatteryTechnology, BixFixedStrings, BstReturn, PowerUnit};
    use ec_test_lib::{BatterySource, ErrorType};

    // ── test doubles ─────────────────────────────────────────────────────────

    struct OkSource;
    impl ErrorType for OkSource {
        type Error = TestError;
    }
    impl BatterySource for OkSource {
        fn get_bst(&self) -> Result<BstReturn, Self::Error> {
            Ok(BstReturn {
                battery_state: BatteryStateFlag::CHARGING,
                battery_present_rate: 1000,
                battery_remaining_capacity: 5000,
                battery_present_voltage: 12000,
            })
        }
        fn get_bix(&self) -> Result<BixFixedStrings, Self::Error> {
            Ok(BixFixedStrings {
                design_capacity: 10000,
                cycle_count: 42,
                power_unit: PowerUnit::MilliWatts,
                ..Default::default()
            })
        }
        fn set_btp(&self, _: u32) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    struct ErrSource;
    impl ErrorType for ErrSource {
        type Error = TestError;
    }
    impl BatterySource for ErrSource {
        fn get_bst(&self) -> Result<BstReturn, Self::Error> {
            Err(TestError)
        }
        fn get_bix(&self) -> Result<BixFixedStrings, Self::Error> {
            Err(TestError)
        }
        fn set_btp(&self, _: u32) -> Result<(), Self::Error> {
            Err(TestError)
        }
    }

    // ── poll_bst ─────────────────────────────────────────────────────────────

    #[test]
    fn poll_bst_sets_success_flag_on_ok() {
        let mut state = BatteryState::default();
        poll_bst(&mut state, &OkSource);
        assert!(state.bst_success);
        assert_eq!(state.bst.battery_remaining_capacity, 5000);
    }

    #[test]
    fn poll_bst_clears_success_flag_on_err() {
        let mut state = BatteryState::default();
        state.bst.battery_remaining_capacity = 99;
        poll_bst(&mut state, &ErrSource);
        assert!(!state.bst_success);
        assert_eq!(state.bst.battery_remaining_capacity, 99);
    }

    // ── poll_bix ─────────────────────────────────────────────────────────────

    #[test]
    fn poll_bix_sets_success_flag_on_ok() {
        let mut state = BatteryState::default();
        poll_bix(&mut state, &OkSource);
        assert!(state.bix_success);
        assert_eq!(state.bix.design_capacity, 10000);
        assert_eq!(state.bix.cycle_count, 42);
    }

    #[test]
    fn poll_bix_clears_success_flag_on_err() {
        let mut state = BatteryState::default();
        state.bix.design_capacity = 9999;
        poll_bix(&mut state, &ErrSource);
        assert!(!state.bix_success);
        assert_eq!(state.bix.design_capacity, 9999);
    }

    // ── format helpers ───────────────────────────────────────────────────────

    #[test]
    fn charge_state_discharging() {
        assert_eq!(charge_state_as_str(BatteryStateFlag::DISCHARGING), "Discharging");
    }

    #[test]
    fn charge_state_charging() {
        assert_eq!(charge_state_as_str(BatteryStateFlag::CHARGING), "Charging");
    }

    #[test]
    fn str_from_bytes_valid_nul_terminated() {
        let bytes = b"Li-ion\0\0";
        assert_eq!(str_from_bytes(bytes), "Li-ion");
    }

    #[test]
    fn str_from_bytes_no_nul_returns_invalid() {
        let bytes = b"no nul here";
        assert_eq!(str_from_bytes(bytes), "Invalid");
    }

    #[test]
    fn power_unit_capacity_strings() {
        assert_eq!(power_unit_as_capacity_str(PowerUnit::MilliWatts), "mWh");
        assert_eq!(power_unit_as_capacity_str(PowerUnit::MilliAmps), "mAh");
    }

    #[test]
    fn power_unit_rate_strings() {
        assert_eq!(power_unit_as_rate_str(PowerUnit::MilliWatts), "mW");
        assert_eq!(power_unit_as_rate_str(PowerUnit::MilliAmps), "mA");
    }

    #[test]
    fn bat_tech_strings() {
        assert_eq!(bat_tech_as_str(BatteryTechnology::Primary), "Primary");
        assert_eq!(bat_tech_as_str(BatteryTechnology::Secondary), "Secondary");
    }

    #[test]
    fn swap_cap_strings() {
        assert_eq!(swap_cap_as_str(BatterySwapCapability::NonSwappable), "Non swappable");
        assert_eq!(swap_cap_as_str(BatterySwapCapability::ColdSwappable), "Cold swappable");
        assert_eq!(swap_cap_as_str(BatterySwapCapability::HotSwappable), "Hot swappable");
    }
}

