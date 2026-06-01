use crate::common;
use crate::common::SYMBOLS;
use crate::state::{FanStateLevels, SensorThresholds, ThermalCommand, ThermalState};
use tracing::{debug, warn};

#[cfg(test)]
use crate::source::DynSource;
#[cfg(test)]
use crate::state::{FanData, FanRpmBounds, SensorData};

use ratatui::{
    buffer::Buffer,
    crossterm::event::{Event, KeyCode, KeyEventKind},
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Stylize, palette::tailwind},
    text::{Line, Span},
    widgets::{Block, Paragraph, Widget},
};
use std::sync::mpsc;
use tui_input::{Input, backend::crossterm::EventHandler};

// ── Threshold configuration ───────────────────────────────────────────────────

// TODO: Implement using source once GET/SET VAR and GET/SET THRS commands are supported.
/// Returns the hardcoded sensor thresholds.  Called by the background updater.
pub(crate) fn sensor_thresholds() -> SensorThresholds {
    SensorThresholds {
        warn_low: 13.0,
        warn_high: 35.0,
        prochot: 40.0,
        critical: 45.0,
    }
}

// ── State-type update methods (used by tests) ─────────────────────────────────

#[cfg(test)]
impl SensorData {
    pub(crate) fn update(&mut self, source: &dyn DynSource) {
        if let Ok(temp) = source.get_temperature() {
            self.skin_temp = temp;
            self.samples.insert(temp);
            self.temp_success = true;
        } else {
            self.temp_success = false;
        }
        self.thresholds = sensor_thresholds();
        self.thresholds_success = true;
    }
}

#[cfg(test)]
impl FanData {
    pub(crate) fn update(&mut self, source: &dyn DynSource) {
        if let Ok(rpm) = source.get_rpm() {
            self.rpm = rpm;
            self.samples.insert(rpm as u32);
            self.rpm_success = true;
        } else {
            self.rpm_success = false;
        }
        match (source.get_min_rpm(), source.get_max_rpm()) {
            (Ok(min), Ok(max)) => {
                self.rpm_bounds = FanRpmBounds { min, max };
                self.bounds_success = true;
            }
            _ => self.bounds_success = false,
        }
        use ec_test_lib::Threshold;
        match (
            source.get_threshold(Threshold::On),
            source.get_threshold(Threshold::Ramping),
            source.get_threshold(Threshold::Max),
        ) {
            (Ok(on), Ok(ramping), Ok(max)) => {
                self.state_levels = FanStateLevels { on, ramping, max };
                self.levels_success = true;
            }
            _ => self.levels_success = false,
        }
    }
}

// ── Color / zone helpers ──────────────────────────────────────────────────────

fn temp_level_color(temp: f64, thresholds: &SensorThresholds) -> Color {
    if temp >= thresholds.critical {
        tailwind::RED.c500
    } else if temp >= thresholds.prochot {
        tailwind::ORANGE.c500
    } else if temp >= thresholds.warn_high {
        tailwind::AMBER.c400
    } else {
        tailwind::GREEN.c400
    }
}

fn thermal_zone(temp: f64, thresholds: &SensorThresholds) -> &'static str {
    if temp >= thresholds.critical {
        "Critical"
    } else if temp >= thresholds.prochot {
        "Prochot"
    } else if temp >= thresholds.warn_high {
        "Warning"
    } else {
        "Normal"
    }
}

fn fan_zone(temp: f64, levels: &FanStateLevels) -> &'static str {
    if temp >= levels.max {
        "Max"
    } else if temp >= levels.ramping {
        "Ramping"
    } else if temp >= levels.on {
        "On"
    } else {
        "Off"
    }
}

fn fan_zone_color(rpm: f64, levels: &FanStateLevels) -> Color {
    if rpm >= levels.max {
        tailwind::AMBER.c400
    } else if rpm >= levels.ramping {
        tailwind::SKY.c400
    } else if rpm >= levels.on {
        tailwind::GREEN.c500
    } else {
        tailwind::SLATE.c500
    }
}

// ── UI module ─────────────────────────────────────────────────────────────────

/// Thermal UI module.  Holds only UI-local state: the fan RPM text input and
/// the command channel for sending write-backs to the background updater.
pub struct Thermal {
    popup_open: bool,
    popup_input: Input,
    cmd_tx: mpsc::Sender<ThermalCommand>,
}

impl Thermal {
    pub fn new(cmd_tx: mpsc::Sender<ThermalCommand>) -> Self {
        Self {
            popup_open: false,
            popup_input: Input::default(),
            cmd_tx,
        }
    }

    pub(crate) fn is_popup_open(&self) -> bool {
        self.popup_open
    }
}

impl Thermal {
    pub(crate) fn handle_event(&mut self, evt: &Event) {
        if let Event::Key(key) = evt
            && key.kind == KeyEventKind::Press
        {
            if self.popup_open {
                match key.code {
                    KeyCode::Esc => {
                        self.popup_open = false;
                        self.popup_input = Input::default();
                        debug!("RPM popup dismissed");
                    }
                    KeyCode::Enter => {
                        let raw = self.popup_input.value_and_reset();
                        self.popup_open = false;
                        if let Ok(rpm) = raw.parse::<f64>() {
                            debug!(rpm, "user requested fan RPM change");
                            let _ = self.cmd_tx.send(ThermalCommand::SetRpm(rpm));
                        } else {
                            warn!(input = raw, "invalid fan RPM value; expected a number");
                        }
                    }
                    _ => {
                        let _ = self.popup_input.handle_event(evt);
                    }
                }
            } else if matches!(key.code, KeyCode::Char('s') | KeyCode::Char('/')) {
                self.popup_open = true;
                debug!("RPM set popup opened");
            }
        }
    }

    pub(crate) fn render(&self, state: &ThermalState, area: Rect, buf: &mut Buffer) {
        // Stacked layout: sensor block on top, fan block on bottom, sparklines fill remaining
        let [sensor_area, fan_area] = Layout::vertical([Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)]).areas(area);

        self.render_sensor_compact(state, sensor_area, buf);
        self.render_fan_compact(state, fan_area, buf);

        if self.popup_open {
            common::render_input_popup(area, buf, " Set Fan RPM ", self.popup_input.value());
        }
    }

    pub(crate) fn render_card(&self, state: &ThermalState, area: Rect, buf: &mut Buffer) {
        use ratatui::layout::Constraint::{Length, Min};

        let th = state;
        let block = Block::bordered()
            .title(common::status_title(
                "Thermal",
                th.sensor.temp_success && th.fan.rpm_success,
            ))
            .border_style(tailwind::ORANGE.c700);
        let inner = block.inner(area);
        block.render(area, buf);

        let [temp_line, temp_gauge, fan_line, fan_gauge, info_area] =
            Layout::vertical([Length(1), Length(1), Length(1), Length(1), Min(0)]).areas(inner);

        // Temperature line + zone label + ΔT/Δt
        let temp_color = temp_level_color(th.sensor.skin_temp, &th.sensor.thresholds);
        let zone = thermal_zone(th.sensor.skin_temp, &th.sensor.thresholds);
        let delta = th.sensor.samples.stats().delta;
        let delta_arrow = if delta > 0.1 {
            SYMBOLS.arrow_up
        } else if delta < -0.1 {
            SYMBOLS.arrow_down
        } else {
            SYMBOLS.mid_dot
        };
        Line::from(vec![
            Span::styled(
                format!("Skin  {:.1} {}C", th.sensor.skin_temp, SYMBOLS.degree),
                Style::default().fg(temp_color).bold(),
            ),
            Span::raw("  "),
            Span::styled(zone, Style::default().fg(temp_color)),
            Span::raw("  "),
            Span::styled(
                format!("{delta_arrow}{delta:+.1}"),
                Style::default().fg(tailwind::SLATE.c500),
            ),
        ])
        .render(temp_line, buf);

        // Temperature threshold gauge
        let max = th.sensor.thresholds.critical + 5.0;
        let ratio = (th.sensor.skin_temp / max).clamp(0.0, 1.0);
        let temp_thresholds = [
            (0.0, tailwind::GREEN.c400),
            (th.sensor.thresholds.warn_high / max, tailwind::AMBER.c400),
            (th.sensor.thresholds.prochot / max, tailwind::ORANGE.c400),
            (th.sensor.thresholds.critical / max, tailwind::RED.c500),
        ];
        common::ThresholdGauge {
            ratio,
            label: Some(Span::raw(format!("{:.1}{}C", th.sensor.skin_temp, SYMBOLS.degree))),
            thresholds: &temp_thresholds,
            track_color: tailwind::SLATE.c800,
        }
        .render(temp_gauge, buf);

        // Fan line + zone label
        let fan_zone_str = fan_zone(th.fan.rpm, &th.fan.state_levels);
        let fan_color = fan_zone_color(th.fan.rpm, &th.fan.state_levels);
        Line::from(vec![
            Span::styled(
                format!("Fan   {:.1} RPM", th.fan.rpm),
                Style::default().fg(common::palette::LABEL).bold(),
            ),
            Span::raw("  "),
            Span::styled(fan_zone_str, Style::default().fg(fan_color)),
        ])
        .render(fan_line, buf);

        // Fan RPM gauge
        let max_rpm = th.fan.rpm_bounds.max.max(1.0);
        let rpm_ratio = (th.fan.rpm / max_rpm).clamp(0.0, 1.0);
        let rpm_thresholds = [
            (0.0, tailwind::SLATE.c500),
            (th.fan.state_levels.on / max_rpm, tailwind::GREEN.c500),
            (th.fan.state_levels.ramping / max_rpm, tailwind::SKY.c400),
            (th.fan.state_levels.max / max_rpm, tailwind::AMBER.c400),
        ];
        common::ThresholdGauge {
            ratio: rpm_ratio,
            label: Some(Span::raw(format!("{:.0} RPM", th.fan.rpm))),
            thresholds: &rpm_thresholds,
            track_color: tailwind::SLATE.c800,
        }
        .render(fan_gauge, buf);

        Paragraph::new(vec![
            common::metric_row(
                "Temp  ",
                format!(
                    "W:{:.0}{d} P:{:.0}{d} C:{:.0}{d}",
                    th.sensor.thresholds.warn_high,
                    th.sensor.thresholds.prochot,
                    th.sensor.thresholds.critical,
                    d = SYMBOLS.degree
                ),
                tailwind::SLATE.c500,
            ),
            common::metric_row(
                "Fan   ",
                format!(
                    "On:{:.0}{d} Ramp:{:.0}{d} Max:{:.0}{d}",
                    th.fan.state_levels.on,
                    th.fan.state_levels.ramping,
                    th.fan.state_levels.max,
                    d = SYMBOLS.degree
                ),
                tailwind::SLATE.c500,
            ),
        ])
        .render(info_area, buf);
    }
}

// ── Compact render helpers ────────────────────────────────────────────────────

impl Thermal {
    fn render_sensor_compact(&self, state: &ThermalState, area: Rect, buf: &mut Buffer) {
        use Constraint::{Length, Min};
        let s = &state.sensor;

        let block = Block::bordered()
            .title(common::status_title("Temperature", s.temp_success))
            .border_style(common::palette::TEMP);
        let inner = block.inner(area);
        block.render(area, buf);

        let [temp_line, gauge_area, thresh_area, stats_area, spark_area] =
            Layout::vertical([Length(1), Length(1), Length(3), Length(1), Min(0)]).areas(inner);

        // Current temp + zone + ΔT/Δt
        let color = temp_level_color(s.skin_temp, &s.thresholds);
        let zone = thermal_zone(s.skin_temp, &s.thresholds);
        let st = s.samples.stats();
        let delta = st.delta;
        let delta_arrow = if delta > 0.1 {
            SYMBOLS.arrow_up
        } else if delta < -0.1 {
            SYMBOLS.arrow_down
        } else {
            SYMBOLS.mid_dot
        };
        let delta_color = if delta > 0.5 {
            tailwind::RED.c500
        } else if delta > 0.1 {
            tailwind::AMBER.c400
        } else if delta < -0.1 {
            tailwind::SKY.c400
        } else {
            tailwind::SLATE.c500
        };
        Line::from(vec![
            Span::styled(
                format!("Skin  {:.1} {}C", s.skin_temp, SYMBOLS.degree),
                Style::default().fg(color).bold(),
            ),
            Span::raw("  "),
            Span::styled(zone, Style::default().fg(color)),
            Span::raw("  "),
            Span::styled(
                format!("{delta_arrow}{delta:+.1}{}C/s", SYMBOLS.degree),
                Style::default().fg(delta_color),
            ),
        ])
        .render(temp_line, buf);

        // Threshold gauge
        let max = s.thresholds.critical + 5.0;
        let ratio = (s.skin_temp / max).clamp(0.0, 1.0);
        let thresholds = [
            (0.0, tailwind::GREEN.c400),
            (s.thresholds.warn_high / max, tailwind::AMBER.c400),
            (s.thresholds.prochot / max, tailwind::ORANGE.c400),
            (s.thresholds.critical / max, tailwind::RED.c500),
        ];
        common::ThresholdGauge {
            ratio,
            label: Some(Span::raw(format!("{:.1}{}C", s.skin_temp, SYMBOLS.degree))),
            thresholds: &thresholds,
            track_color: tailwind::SLATE.c800,
        }
        .render(gauge_area, buf);

        // Inline thresholds
        Paragraph::new(vec![
            common::metric_row(
                "Warn    ",
                format!("{:.0} {}C", s.thresholds.warn_high, SYMBOLS.degree),
                common::palette::LABEL,
            ),
            common::metric_row(
                "Prochot ",
                format!("{:.0} {}C", s.thresholds.prochot, SYMBOLS.degree),
                common::palette::LABEL,
            ),
            common::metric_row(
                "Critical",
                format!("{:.0} {}C", s.thresholds.critical, SYMBOLS.degree),
                common::palette::LABEL,
            ),
        ])
        .render(thresh_area, buf);

        // Session stats
        if st.has_data() {
            let d = SYMBOLS.degree;
            common::metric_row(
                "Session ",
                format!("Min:{:.1}{d}C  Max:{:.1}{d}C  Avg:{:.1}{d}C", st.min, st.max, st.avg()),
                tailwind::SLATE.c500,
            )
            .render(stats_area, buf);
        }

        // Sparkline
        let samples = s.samples.get();
        common::render_sparkline(spark_area, buf, &samples, common::palette::TEMP, [0.0, max]);
    }

    fn render_fan_compact(&self, state: &ThermalState, area: Rect, buf: &mut Buffer) {
        use Constraint::{Length, Min};
        let s = &state.sensor;
        let f = &state.fan;

        let block = Block::bordered()
            .title(common::status_title("Fan", f.rpm_success && f.bounds_success))
            .border_style(common::palette::FAN);
        let inner = block.inner(area);
        block.render(area, buf);

        let [rpm_line, gauge_area, info_area, stats_area, spark_area] =
            Layout::vertical([Length(1), Length(1), Length(4), Length(1), Min(0)]).areas(inner);

        // Current RPM + zone
        let zone_str = fan_zone(s.skin_temp, &f.state_levels);
        let zone_color = fan_zone_color(f.rpm, &f.state_levels);
        Line::from(vec![
            Span::styled(
                format!("Fan   {:.0} RPM", f.rpm),
                Style::default().fg(common::palette::LABEL).bold(),
            ),
            Span::raw("  "),
            Span::styled(zone_str, Style::default().fg(zone_color)),
        ])
        .render(rpm_line, buf);

        // RPM gauge
        let max_rpm = f.rpm_bounds.max.max(1.0);
        let rpm_ratio = (f.rpm / max_rpm).clamp(0.0, 1.0);
        let thresholds = [
            (0.0, tailwind::SLATE.c500),
            (f.state_levels.on / max_rpm, tailwind::GREEN.c500),
            (f.state_levels.ramping / max_rpm, tailwind::SKY.c400),
            (f.state_levels.max / max_rpm, tailwind::AMBER.c400),
        ];
        common::ThresholdGauge {
            ratio: rpm_ratio,
            label: Some(Span::raw(format!("{:.0} RPM", f.rpm))),
            thresholds: &thresholds,
            track_color: tailwind::SLATE.c800,
        }
        .render(gauge_area, buf);

        // Inline info: bounds + state levels + limit
        let (limit_str, limit_color) = if state.rpm_limit > 0.0 {
            let color = if state.rpm_set_success {
                tailwind::GREEN.c400
            } else {
                tailwind::RED.c500
            };
            (format!("{:.0} RPM", state.rpm_limit), color)
        } else {
            ("-- (press s to set)".to_string(), tailwind::SLATE.c500)
        };
        Paragraph::new(vec![
            common::metric_row(
                "Range   ",
                format!(
                    "{:.0} {} {:.0} RPM",
                    f.rpm_bounds.min, SYMBOLS.en_dash, f.rpm_bounds.max
                ),
                common::palette::LABEL,
            ),
            common::metric_row(
                "On      ",
                format!("{:.0} {}C", f.state_levels.on, SYMBOLS.degree),
                common::palette::LABEL,
            ),
            common::metric_row(
                "Ramping ",
                format!("{:.0} {}C", f.state_levels.ramping, SYMBOLS.degree),
                common::palette::LABEL,
            ),
            common::metric_row("Override", limit_str, limit_color),
        ])
        .render(info_area, buf);

        // Session stats
        let fan_st = f.samples.stats();
        if fan_st.has_data() {
            common::metric_row(
                "Session ",
                format!(
                    "Min:{:.0}  Max:{:.0}  Avg:{:.0} RPM",
                    fan_st.min,
                    fan_st.max,
                    fan_st.avg()
                ),
                tailwind::SLATE.c500,
            )
            .render(stats_area, buf);
        }

        // Sparkline
        let samples = f.samples.get();
        common::render_sparkline(spark_area, buf, &samples, common::palette::FAN, [0.0, max_rpm]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use color_eyre::eyre::eyre;
    use ec_test_lib::Threshold;

    // ── test doubles ─────────────────────────────────────────────────────────

    struct OkThermal;
    impl DynSource for OkThermal {
        fn get_temperature(&self) -> color_eyre::Result<f64> {
            Ok(25.5)
        }
        fn get_rpm(&self) -> color_eyre::Result<f64> {
            Ok(3000.0)
        }
        fn get_min_rpm(&self) -> color_eyre::Result<f64> {
            Ok(0.0)
        }
        fn get_max_rpm(&self) -> color_eyre::Result<f64> {
            Ok(6000.0)
        }
        fn get_threshold(&self, threshold: Threshold) -> color_eyre::Result<f64> {
            match threshold {
                Threshold::On => Ok(28.0),
                Threshold::Ramping => Ok(40.0),
                Threshold::Max => Ok(44.0),
            }
        }
        fn set_rpm(&self, _: f64) -> color_eyre::Result<()> {
            Ok(())
        }
        fn get_bst(&self) -> color_eyre::Result<battery_service_messages::BstReturn> {
            Err(eyre!("unused"))
        }
        fn get_bix(&self) -> color_eyre::Result<battery_service_messages::BixFixedStrings> {
            Err(eyre!("unused"))
        }
        fn set_btp(&self, _: u32) -> color_eyre::Result<()> {
            Err(eyre!("unused"))
        }
        fn get_capabilities(&self) -> color_eyre::Result<time_alarm_service_messages::TimeAlarmDeviceCapabilities> {
            Err(eyre!("unused"))
        }
        fn get_real_time(&self) -> color_eyre::Result<time_alarm_service_messages::AcpiTimestamp> {
            Err(eyre!("unused"))
        }
        fn get_wake_status(
            &self,
            _: time_alarm_service_messages::AcpiTimerId,
        ) -> color_eyre::Result<time_alarm_service_messages::TimerStatus> {
            Err(eyre!("unused"))
        }
        fn get_expired_timer_wake_policy(
            &self,
            _: time_alarm_service_messages::AcpiTimerId,
        ) -> color_eyre::Result<time_alarm_service_messages::AlarmExpiredWakePolicy> {
            Err(eyre!("unused"))
        }
        fn get_timer_value(
            &self,
            _: time_alarm_service_messages::AcpiTimerId,
        ) -> color_eyre::Result<time_alarm_service_messages::AlarmTimerSeconds> {
            Err(eyre!("unused"))
        }
    }

    struct ErrThermal;
    impl DynSource for ErrThermal {
        fn get_temperature(&self) -> color_eyre::Result<f64> {
            Err(eyre!("test error"))
        }
        fn get_rpm(&self) -> color_eyre::Result<f64> {
            Err(eyre!("test error"))
        }
        fn get_min_rpm(&self) -> color_eyre::Result<f64> {
            Err(eyre!("test error"))
        }
        fn get_max_rpm(&self) -> color_eyre::Result<f64> {
            Err(eyre!("test error"))
        }
        fn get_threshold(&self, _: Threshold) -> color_eyre::Result<f64> {
            Err(eyre!("test error"))
        }
        fn set_rpm(&self, _: f64) -> color_eyre::Result<()> {
            Err(eyre!("test error"))
        }
        fn get_bst(&self) -> color_eyre::Result<battery_service_messages::BstReturn> {
            Err(eyre!("unused"))
        }
        fn get_bix(&self) -> color_eyre::Result<battery_service_messages::BixFixedStrings> {
            Err(eyre!("unused"))
        }
        fn set_btp(&self, _: u32) -> color_eyre::Result<()> {
            Err(eyre!("unused"))
        }
        fn get_capabilities(&self) -> color_eyre::Result<time_alarm_service_messages::TimeAlarmDeviceCapabilities> {
            Err(eyre!("unused"))
        }
        fn get_real_time(&self) -> color_eyre::Result<time_alarm_service_messages::AcpiTimestamp> {
            Err(eyre!("unused"))
        }
        fn get_wake_status(
            &self,
            _: time_alarm_service_messages::AcpiTimerId,
        ) -> color_eyre::Result<time_alarm_service_messages::TimerStatus> {
            Err(eyre!("unused"))
        }
        fn get_expired_timer_wake_policy(
            &self,
            _: time_alarm_service_messages::AcpiTimerId,
        ) -> color_eyre::Result<time_alarm_service_messages::AlarmExpiredWakePolicy> {
            Err(eyre!("unused"))
        }
        fn get_timer_value(
            &self,
            _: time_alarm_service_messages::AcpiTimerId,
        ) -> color_eyre::Result<time_alarm_service_messages::AlarmTimerSeconds> {
            Err(eyre!("unused"))
        }
    }

    // ── SensorData ───────────────────────────────────────────────────────────

    #[test]
    fn sensor_update_sets_success_and_temp_on_ok() {
        let mut data = SensorData::default();
        data.update(&OkThermal);
        assert!(data.temp_success);
        assert_eq!(data.skin_temp, 25.5);
        assert!(data.thresholds_success);
        assert_eq!(data.thresholds.warn_high, 35.0);
    }

    #[test]
    fn sensor_update_clears_temp_success_on_err() {
        let mut data = SensorData {
            skin_temp: 99.9,
            ..SensorData::default()
        };
        data.update(&ErrThermal);
        assert!(!data.temp_success);
        assert_eq!(data.skin_temp, 99.9);
        assert!(data.thresholds_success);
    }

    #[test]
    fn sensor_update_records_sample_on_ok() {
        let mut data = SensorData::default();
        data.update(&OkThermal);
        assert!(!data.samples.get().is_empty());
    }

    // ── FanData ──────────────────────────────────────────────────────────────

    #[test]
    fn fan_update_sets_success_on_ok() {
        let mut data = FanData::default();
        data.update(&OkThermal);
        assert!(data.rpm_success);
        assert_eq!(data.rpm, 3000.0);
        assert!(data.bounds_success);
        assert_eq!(data.rpm_bounds.max, 6000.0);
        assert!(data.levels_success);
        assert_eq!(data.state_levels.ramping, 40.0);
    }

    #[test]
    fn fan_update_clears_success_on_err() {
        let mut data = FanData {
            rpm: 1234.0,
            ..FanData::default()
        };
        data.update(&ErrThermal);
        assert!(!data.rpm_success);
        assert!(!data.bounds_success);
        assert!(!data.levels_success);
        assert_eq!(data.rpm, 1234.0);
    }

    #[test]
    fn fan_update_records_sample_on_ok() {
        let mut data = FanData::default();
        data.update(&OkThermal);
        assert!(!data.samples.get().is_empty());
    }
}
