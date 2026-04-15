use crate::common;
use crate::state::{AppState, FanStateLevels, SensorThresholds, ThermalCommand};
use tracing::{debug, warn};

#[cfg(test)]
use crate::state::{FanData, FanRpmBounds, SensorData};

#[cfg(test)]
use ec_test_lib::ThermalSource;
use ratatui::{
    buffer::Buffer,
    crossterm::event::{Event, KeyCode, KeyEventKind},
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style, Stylize, palette::tailwind},
    text::{Line, Span},
    widgets::{Block, Paragraph, Widget},
};
use std::sync::mpsc;
use tui_input::{Input, backend::crossterm::EventHandler};

const LABEL_COLOR: Color = tailwind::ORANGE.c300;

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
    pub fn update<S: ThermalSource>(&mut self, source: &S) {
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
    pub fn update<S: ThermalSource>(&mut self, source: &S) {
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

fn fan_zone(rpm: f64, levels: &FanStateLevels) -> &'static str {
    if rpm >= levels.max {
        "Max"
    } else if rpm >= levels.ramping {
        "Ramping"
    } else if rpm >= levels.on {
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
    rpm_input: Input,
    cmd_tx: mpsc::Sender<ThermalCommand>,
}

impl Thermal {
    pub fn new(cmd_tx: mpsc::Sender<ThermalCommand>) -> Self {
        Self {
            rpm_input: Input::default(),
            cmd_tx,
        }
    }
}

impl Thermal {
    pub(crate) fn handle_event(&mut self, evt: &Event) {
        if let Event::Key(key) = evt
            && key.code == KeyCode::Enter
            && key.kind == KeyEventKind::Press
        {
            let raw = self.rpm_input.value_and_reset();
            if let Ok(rpm) = raw.parse::<f64>() {
                debug!(rpm, "user requested fan RPM change");
                let _ = self.cmd_tx.send(ThermalCommand::SetRpm(rpm));
            } else {
                warn!(input = raw, "invalid fan RPM value; expected a number");
            }
        } else {
            let _ = self.rpm_input.handle_event(evt);
        }
    }

    pub(crate) fn render(&self, state: &AppState, area: Rect, buf: &mut Buffer) {
        let [sensor_area, fan_area] = common::area_split(area, Direction::Horizontal, 50, 50);
        self.render_sensor(state, sensor_area, buf);
        self.render_fan(state, fan_area, buf);
    }

    pub(crate) fn render_card(&self, state: &AppState, area: Rect, buf: &mut Buffer) {
        use ratatui::layout::Constraint::{Length, Min};

        let th = &state.thermal;
        let block = Block::bordered()
            .title(common::status_title("Thermal", th.sensor.temp_success && th.fan.rpm_success))
            .border_style(tailwind::ORANGE.c700);
        let inner = block.inner(area);
        block.render(area, buf);

        let [temp_line, temp_gauge, fan_line, fan_gauge, info_area] =
            Layout::vertical([Length(1), Length(1), Length(1), Length(1), Min(0)]).areas(inner);

        // Temperature line + zone label
        let temp_color = temp_level_color(th.sensor.skin_temp, &th.sensor.thresholds);
        let zone = thermal_zone(th.sensor.skin_temp, &th.sensor.thresholds);
        Line::from(vec![
            Span::styled(
                format!("Skin  {:.1} \u{00b0}C", th.sensor.skin_temp),
                Style::default().fg(temp_color).bold(),
            ),
            Span::raw("  "),
            Span::styled(zone, Style::default().fg(temp_color)),
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
            label: Some(Span::raw(format!("{:.1}\u{00b0}C", th.sensor.skin_temp))),
            thresholds: &temp_thresholds,
            track_color: tailwind::SLATE.c800,
        }
        .render(temp_gauge, buf);

        // Fan line + zone label
        let fan_zone_str = fan_zone(th.fan.rpm, &th.fan.state_levels);
        let fan_color = fan_zone_color(th.fan.rpm, &th.fan.state_levels);
        Line::from(vec![
            Span::styled(
                format!("Fan   {:.0} RPM", th.fan.rpm),
                Style::default().fg(LABEL_COLOR).bold(),
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
                    "W:{:.0}\u{00b0} P:{:.0}\u{00b0} C:{:.0}\u{00b0}",
                    th.sensor.thresholds.warn_high,
                    th.sensor.thresholds.prochot,
                    th.sensor.thresholds.critical
                ),
                tailwind::SLATE.c500,
            ),
            common::metric_row(
                "Fan   ",
                format!(
                    "On:{:.0} Ramp:{:.0} Max:{:.0} RPM",
                    th.fan.state_levels.on,
                    th.fan.state_levels.ramping,
                    th.fan.state_levels.max
                ),
                tailwind::SLATE.c500,
            ),
        ])
        .render(info_area, buf);
    }
}

// ── Render helpers ────────────────────────────────────────────────────────────

impl Thermal {
    fn render_sensor(&self, state: &AppState, area: Rect, buf: &mut Buffer) {
        let [chart_area, stats_area] = common::area_split(area, Direction::Vertical, 65, 35);
        self.render_sensor_chart(state, chart_area, buf);
        self.render_sensor_stats(state, stats_area, buf);
    }

    fn render_sensor_chart(&self, state: &AppState, area: Rect, buf: &mut Buffer) {
        let s = &state.thermal.sensor;
        let y_labels = [
            "0.0".bold(),
            Span::styled(
                format!("{:.1}", (s.thresholds.critical + 5.0) / 2.0),
                Style::default().bold(),
            ),
            Span::styled(
                format!("{:.1}", s.thresholds.critical + 5.0),
                Style::default().bold(),
            ),
        ];
        let graph = common::Graph {
            title: "Temperature vs Time".to_string(),
            color: tailwind::ORANGE.c400,
            samples: s.samples.get(),
            x_axis: "Time (s)".to_string(),
            x_bounds: [0.0, 60.0],
            x_labels: common::time_labels(state.thermal.t, crate::state::THERMAL_MAX_SAMPLES),
            y_axis: "Temperature (\u{00b0}C)".to_string(),
            y_bounds: [0.0, s.thresholds.critical + 5.0],
            y_labels,
        };
        common::render_chart(area, buf, graph);
    }

    fn render_sensor_stats(&self, state: &AppState, area: Rect, buf: &mut Buffer) {
        let s = &state.thermal.sensor;
        let block = common::title_block(
            common::status_title("Live Temperature", s.temp_success),
            0,
            LABEL_COLOR,
        );
        let inner = block.inner(area);
        block.render(area, buf);

        use Constraint::{Length, Min};
        let [temp_line, gauge_area, thresholds_area] =
            Layout::vertical([Length(1), Length(1), Min(0)]).areas(inner);

        let color = temp_level_color(s.skin_temp, &s.thresholds);
        Paragraph::new(common::metric_row(
            "Skin ",
            format!("{:.2} \u{00b0}C", s.skin_temp),
            color,
        ))
        .render(temp_line, buf);

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
            label: Some(Span::raw(format!("{:.1}\u{00b0}C", s.skin_temp))),
            thresholds: &thresholds,
            track_color: tailwind::SLATE.c800,
        }
        .render(gauge_area, buf);

        Paragraph::new(vec![
            common::metric_row("Warn    ", format!("{:.0} \u{00b0}C", s.thresholds.warn_high), LABEL_COLOR),
            common::metric_row("Prochot ", format!("{:.0} \u{00b0}C", s.thresholds.prochot), LABEL_COLOR),
            common::metric_row("Critical", format!("{:.0} \u{00b0}C", s.thresholds.critical), LABEL_COLOR),
        ])
        .render(thresholds_area, buf);
    }

    fn render_fan(&self, state: &AppState, area: Rect, buf: &mut Buffer) {
        let [chart_area, widget_area] = common::area_split(area, Direction::Vertical, 65, 35);
        let [stats_area, levels_area] = common::area_split(widget_area, Direction::Horizontal, 50, 50);
        self.render_fan_chart(state, chart_area, buf);
        self.render_fan_stats(state, stats_area, buf);
        self.render_fan_levels(state, levels_area, buf);
    }

    fn render_fan_chart(&self, state: &AppState, area: Rect, buf: &mut Buffer) {
        let f = &state.thermal.fan;
        let y_labels = [
            "0.0".bold(),
            Span::styled((f.rpm_bounds.max / 2.0).to_string(), Style::default().bold()),
            Span::styled(f.rpm_bounds.max.to_string(), Style::default().bold()),
        ];
        let graph = common::Graph {
            title: "Fan RPM vs Time".to_string(),
            color: tailwind::SKY.c400,
            samples: f.samples.get(),
            x_axis: "Time (s)".to_string(),
            x_bounds: [0.0, 60.0],
            x_labels: common::time_labels(state.thermal.t, crate::state::THERMAL_MAX_SAMPLES),
            y_axis: "RPM".to_string(),
            y_bounds: [0.0, f.rpm_bounds.max],
            y_labels,
        };
        common::render_chart(area, buf, graph);
    }

    fn render_fan_stats(&self, state: &AppState, area: Rect, buf: &mut Buffer) {
        let f = &state.thermal.fan;
        let block = common::title_block(
            common::status_title("Live Fan RPM", f.rpm_success && f.bounds_success),
            0,
            LABEL_COLOR,
        );
        let inner = block.inner(area);
        block.render(area, buf);

        use Constraint::{Length, Min};
        let [rpm_line, gauge_area, input_area] =
            Layout::vertical([Length(1), Length(1), Min(0)]).areas(inner);

        Paragraph::new(common::metric_row(
            "RPM  ",
            format!("{:.0}  ({} \u{2013} {})", f.rpm, f.rpm_bounds.min, f.rpm_bounds.max),
            LABEL_COLOR,
        ))
        .render(rpm_line, buf);

        let max = f.rpm_bounds.max.max(1.0);
        let ratio = (f.rpm / max).clamp(0.0, 1.0);
        let thresholds = [
            (0.0, tailwind::GREEN.c500),
            (f.state_levels.on / max, tailwind::SKY.c400),
            (f.state_levels.ramping / max, tailwind::AMBER.c400),
            (f.state_levels.max / max, tailwind::ORANGE.c400),
        ];
        common::ThresholdGauge {
            ratio,
            label: Some(Span::raw(format!("{:.0} RPM", f.rpm))),
            thresholds: &thresholds,
            track_color: tailwind::SLATE.c800,
        }
        .render(gauge_area, buf);

        self.render_fan_rpm_input(input_area, buf);
    }

    fn render_fan_levels(&self, state: &AppState, area: Rect, buf: &mut Buffer) {
        let f = &state.thermal.fan;
        let block = common::title_block(
            common::status_title("Fan State Levels", f.levels_success),
            1,
            LABEL_COLOR,
        );
        Paragraph::new(vec![
            common::metric_row("On      ", format!("{:.0} \u{00b0}C", f.state_levels.on), LABEL_COLOR),
            common::metric_row("Ramping ", format!("{:.0} \u{00b0}C", f.state_levels.ramping), LABEL_COLOR),
            common::metric_row("Max     ", format!("{:.0} \u{00b0}C", f.state_levels.max), LABEL_COLOR),
        ])
        .block(block)
        .render(area, buf);
    }

    fn render_fan_rpm_input(&self, area: Rect, buf: &mut Buffer) {
        let width = area.width.max(3) - 3;
        let scroll = self.rpm_input.visual_scroll(width as usize);
        Paragraph::new(self.rpm_input.value())
            .style(Style::default())
            .scroll((0, scroll as u16))
            .block(
                Block::bordered()
                    .title("Set Fan RPM <ENTER>")
                    .border_style(Style::default().fg(tailwind::ORANGE.c600)),
            )
            .render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::test_support::TestError;
    use ec_test_lib::{ErrorType, Threshold};

    // ── test doubles ─────────────────────────────────────────────────────────

    struct OkThermal;
    impl ErrorType for OkThermal {
        type Error = TestError;
    }
    impl ThermalSource for OkThermal {
        fn get_temperature(&self) -> Result<f64, Self::Error> {
            Ok(25.5)
        }
        fn get_rpm(&self) -> Result<f64, Self::Error> {
            Ok(3000.0)
        }
        fn get_min_rpm(&self) -> Result<f64, Self::Error> {
            Ok(0.0)
        }
        fn get_max_rpm(&self) -> Result<f64, Self::Error> {
            Ok(6000.0)
        }
        fn get_threshold(&self, threshold: Threshold) -> Result<f64, Self::Error> {
            match threshold {
                Threshold::On => Ok(28.0),
                Threshold::Ramping => Ok(40.0),
                Threshold::Max => Ok(44.0),
            }
        }
        fn set_rpm(&self, _: f64) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    struct ErrThermal;
    impl ErrorType for ErrThermal {
        type Error = TestError;
    }
    impl ThermalSource for ErrThermal {
        fn get_temperature(&self) -> Result<f64, Self::Error> {
            Err(TestError)
        }
        fn get_rpm(&self) -> Result<f64, Self::Error> {
            Err(TestError)
        }
        fn get_min_rpm(&self) -> Result<f64, Self::Error> {
            Err(TestError)
        }
        fn get_max_rpm(&self) -> Result<f64, Self::Error> {
            Err(TestError)
        }
        fn get_threshold(&self, _: Threshold) -> Result<f64, Self::Error> {
            Err(TestError)
        }
        fn set_rpm(&self, _: f64) -> Result<(), Self::Error> {
            Err(TestError)
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
        let mut data = SensorData::default();
        data.skin_temp = 99.9;
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
        let mut data = FanData::default();
        data.rpm = 1234.0;
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

