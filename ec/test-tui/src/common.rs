use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style, Stylize, palette::tailwind},
    symbols,
    text::{Line, Span},
    widgets::{Axis, Block, Chart, Clear, Dataset, GraphType, LineGauge, Padding, Paragraph, Widget},
};
use std::collections::VecDeque;
use std::sync::LazyLock;

// ── Unicode mode ──────────────────────────────────────────────────────────────

/// Whether unicode rendering is globally enabled.
///
/// Set `EC_TUI_DISABLE_UNICODE=1` (or any non-empty value) to force
/// pure-ASCII output across the entire UI.  When unset, unicode support is
/// auto-detected from the terminal.
static UNICODE_ENABLED: LazyLock<bool> = LazyLock::new(|| {
    if std::env::var("EC_TUI_DISABLE_UNICODE")
        .ok()
        .filter(|v| !v.is_empty())
        .is_some()
    {
        return false;
    }
    supports_unicode::on(supports_unicode::Stream::Stdout)
});

#[inline]
pub(crate) fn unicode_enabled() -> bool {
    *UNICODE_ENABLED
}

/// Symbol set used throughout the UI, chosen once at startup.
pub(crate) struct Symbols {
    /// Status dot, no trailing space (e.g. `"●"` or `"*"`).
    pub dot: &'static str,
    /// Charging indicator (e.g. `"▲"` or `"^"`).
    pub charging: &'static str,
    /// Discharging indicator (e.g. `"▼"` or `"v"`).
    pub discharging: &'static str,
    /// Upward arrow (e.g. `"↑"` or `"^"`).
    pub arrow_up: &'static str,
    /// Downward arrow (e.g. `"↓"` or `"v"`).
    pub arrow_down: &'static str,
    /// Left-pointing arrow (e.g. `"◄"` or `"<"`).
    pub arrow_left: &'static str,
    /// Right-pointing arrow (e.g. `"►"` or `">"`).
    pub arrow_right: &'static str,
    /// Degree sign for temperatures (e.g. `"°"` or `""`).
    pub degree: &'static str,
    /// En-dash for ranges (e.g. `"–"` or `"-"`).
    pub en_dash: &'static str,
    /// Warning sign (e.g. `"⚠"` or `"!"`).
    pub warning: &'static str,
    /// Horizontal line for dividers (e.g. `"─"` or `"-"`).
    pub h_line: &'static str,
    /// Middle dot separator (e.g. `"·"` or `"."`).
    pub mid_dot: &'static str,
}

/// Application-wide symbol set, selected once at startup via [`unicode_enabled()`].
pub(crate) static SYMBOLS: LazyLock<Symbols> = LazyLock::new(|| {
    if unicode_enabled() {
        Symbols {
            dot: "●",
            charging: "▲",
            discharging: "▼",
            arrow_up: "↑",
            arrow_down: "↓",
            arrow_left: "◄",
            arrow_right: "►",
            degree: "°",
            en_dash: "–",
            warning: "⚠",
            h_line: "─",
            mid_dot: "·",
        }
    } else {
        Symbols {
            dot: "*",
            charging: "^",
            discharging: "v",
            arrow_up: "^",
            arrow_down: "v",
            arrow_left: "<",
            arrow_right: ">",
            degree: "",
            en_dash: "-",
            warning: "!",
            h_line: "-",
            mid_dot: ".",
        }
    }
});

/// Chart marker selected once at startup.
///
/// Uses [`Marker::Braille`] when unicode is enabled, [`Marker::Dot`] otherwise.
/// Override globally with `EC_TUI_DISABLE_UNICODE=1`.
pub(crate) static CHART_MARKER: LazyLock<symbols::Marker> = LazyLock::new(|| {
    if unicode_enabled() {
        symbols::Marker::Braille
    } else {
        symbols::Marker::Dot
    }
});

// ── Global color constants ────────────────────────────────────────────────────

/// Consistent color palette for metric types across all tabs.
#[allow(dead_code)]
pub(crate) mod palette {
    use ratatui::style::{Color, palette::tailwind};

    pub const TEMP: Color = tailwind::ORANGE.c400;
    pub const FAN: Color = tailwind::SKY.c400;
    pub const BATTERY: Color = tailwind::SKY.c400;
    pub const CPU: Color = tailwind::EMERALD.c400;
    pub const MEM: Color = tailwind::SKY.c400;
    pub const NET_RX: Color = tailwind::VIOLET.c400;
    pub const NET_TX: Color = tailwind::AMBER.c400;
    pub const LABEL: Color = tailwind::SLATE.c400;
    pub const SUCCESS: Color = tailwind::GREEN.c400;
    pub const FAILURE: Color = tailwind::RED.c500;
}

#[derive(Default)]
pub struct SampleBuf<T, const N: usize> {
    samples: VecDeque<T>,
    /// Running session statistics (all-time, not windowed).
    stats: SessionStats,
}

/// Lightweight running statistics — no history needed, just accumulators.
#[derive(Default, Clone, Copy)]
pub struct SessionStats {
    pub min: f64,
    pub max: f64,
    sum: f64,
    count: u64,
    /// Most recent value (used for rate-of-change).
    prev: f64,
    /// Difference between the two most recent samples.
    pub delta: f64,
}

impl SessionStats {
    pub fn avg(&self) -> f64 {
        if self.count == 0 {
            0.0
        } else {
            self.sum / self.count as f64
        }
    }

    /// Returns true once at least one sample has been recorded.
    pub fn has_data(&self) -> bool {
        self.count > 0
    }

    fn record(&mut self, value: f64) {
        if self.count == 0 {
            self.min = value;
            self.max = value;
        } else {
            if value < self.min {
                self.min = value;
            }
            if value > self.max {
                self.max = value;
            }
            self.delta = value - self.prev;
        }
        self.sum += value;
        self.count += 1;
        self.prev = value;
    }
}

impl<T: Into<f64> + Copy, const N: usize> SampleBuf<T, N> {
    pub fn insert(&mut self, sample: T) {
        let val: f64 = sample.into();
        self.stats.record(val);
        self.samples.push_back(sample);
        if self.samples.len() > N {
            self.samples.pop_front();
        }
    }

    /// Converts the buffer into `(x, y)` pairs for ratatui charts.
    pub fn get(&self) -> Vec<(f64, f64)> {
        self.samples
            .iter()
            .enumerate()
            .map(|(i, &val)| (i as f64, val.into()))
            .collect()
    }

    /// Returns the running session statistics.
    pub fn stats(&self) -> &SessionStats {
        &self.stats
    }
}

/// Properties for rendering a full-size line chart.
pub struct Graph {
    pub title: String,
    pub color: Color,
    pub samples: Vec<(f64, f64)>,

    pub x_axis: String,
    pub x_bounds: [f64; 2],
    pub x_labels: [Span<'static>; 3],

    pub y_axis: String,
    pub y_bounds: [f64; 2],
    pub y_labels: [Span<'static>; 3],
}

/// Split an area in a direction with given percentages.
pub fn area_split(area: Rect, direction: Direction, first: u16, second: u16) -> [Rect; 2] {
    Layout::default()
        .direction(direction)
        .constraints([Constraint::Percentage(first), Constraint::Percentage(second)])
        .split(area)
        .as_ref()
        .try_into()
        .expect("layout always produces exactly 2 areas")
}

/// Wraps content in a titled bordered block.
pub fn title_block(title: impl Into<Line<'static>>, padding: u16, label_color: Color) -> Block<'static> {
    Block::bordered()
        .padding(Padding::vertical(padding))
        .title(title.into())
        .fg(label_color)
}

/// Returns a [`Line`] with a colored status dot followed by `title`.
pub fn status_title(title: impl Into<String>, success: bool) -> Line<'static> {
    let color = if success { palette::SUCCESS } else { palette::FAILURE };
    Line::from(vec![
        Span::styled(format!(" {} ", SYMBOLS.dot), Style::default().fg(color)),
        Span::raw(format!("{} ", title.into())),
    ])
}

/// Render a full-size chart with axes and labels.
pub fn render_chart(area: Rect, buf: &mut Buffer, graph: Graph) {
    let samples = &graph.samples[..];
    let datasets = vec![
        Dataset::default()
            .marker(*CHART_MARKER)
            .style(Style::default().fg(graph.color))
            .graph_type(GraphType::Line)
            .data(samples),
    ];

    let chart = Chart::new(datasets)
        .block(Block::bordered().title(Line::from(graph.title).cyan().bold().centered()))
        .x_axis(
            Axis::default()
                .title(graph.x_axis)
                .style(Style::default().gray())
                .bounds(graph.x_bounds)
                .labels(graph.x_labels),
        )
        .y_axis(
            Axis::default()
                .title(graph.y_axis)
                .style(Style::default().gray())
                .bounds(graph.y_bounds)
                .labels(graph.y_labels),
        );

    chart.render(area, buf);
}

/// Render a compact sparkline chart — no axes, no labels, just the line.
///
/// The chart fills the given `area` entirely.  Intended for areas of 3–5 rows
/// height.  Falls back to a blank area if `area.height < 2`.
pub fn render_sparkline(area: Rect, buf: &mut Buffer, samples: &[(f64, f64)], color: Color, y_bounds: [f64; 2]) {
    if area.height < 2 || samples.is_empty() {
        return;
    }

    let datasets = vec![
        Dataset::default()
            .marker(*CHART_MARKER)
            .style(Style::default().fg(color))
            .graph_type(GraphType::Line)
            .data(samples),
    ];

    let x_max = samples.last().map_or(60.0, |&(x, _)| x).max(1.0);

    Chart::new(datasets)
        .block(Block::bordered().border_style(Style::default().fg(tailwind::SLATE.c800)))
        .x_axis(Axis::default().bounds([0.0, x_max]))
        .y_axis(Axis::default().bounds(y_bounds))
        .render(area, buf);
}

pub fn time_labels(max_samples: usize) -> [Span<'static>; 3] {
    [
        Span::styled("0", Style::default().bold()),
        Span::styled((max_samples / 2).to_string(), Style::default().bold()),
        Span::styled(max_samples.to_string(), Style::default().bold()),
    ]
}

/// A single label/value row, styled for compact data tables.
///
/// `label` is rendered bold in `label_color`; `value` is plain white.
pub fn metric_row<'a>(label: &'a str, value: impl Into<String>, label_color: Color) -> Line<'a> {
    Line::from(vec![
        Span::styled(label, Style::default().fg(label_color).bold()),
        Span::raw("  "),
        Span::styled(value.into(), Style::default().fg(Color::White)),
    ])
}

/// A horizontal gauge with up to four colored threshold bands.
pub struct ThresholdGauge<'a> {
    pub ratio: f64,
    pub label: Option<Span<'a>>,
    /// (threshold_ratio, color_above) pairs sorted ascending.
    pub thresholds: &'a [(f64, Color)],
    pub track_color: Color,
}

impl Widget for ThresholdGauge<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let color = self
            .thresholds
            .iter()
            .rev()
            .find(|(t, _)| self.ratio >= *t)
            .map_or(self.thresholds.first().map_or(Color::Green, |&(_, c)| c), |&(_, c)| c);

        let label = self
            .label
            .unwrap_or_else(|| Span::raw(format!("{:.0}%", self.ratio * 100.0)));

        LineGauge::default()
            .filled_style(Style::default().fg(color))
            .unfilled_style(Style::default().fg(self.track_color))
            .label(label)
            .ratio(self.ratio.clamp(0.0, 1.0))
            .render(area, buf);
    }
}

/// Render a centered input popup overlay.
pub fn render_input_popup(area: Rect, buf: &mut Buffer, title: &str, value: &str) {
    let popup_w = 52u16.min(area.width);
    let popup_h = 5u16;
    let popup = Rect {
        x: area.x + area.width.saturating_sub(popup_w) / 2,
        y: area.y + area.height.saturating_sub(popup_h) / 2,
        width: popup_w,
        height: popup_h,
    };
    Clear.render(popup, buf);
    let block = Block::bordered()
        .title(Line::from(title).bold().centered())
        .title_bottom(
            Line::from(Span::styled(
                " Enter  confirm    Esc  cancel ",
                Style::default().fg(tailwind::SLATE.c500),
            ))
            .centered(),
        )
        .border_style(tailwind::YELLOW.c500);
    let inner = block.inner(popup);
    block.render(popup, buf);
    Paragraph::new(format!("> {value}")).render(inner, buf);
}
