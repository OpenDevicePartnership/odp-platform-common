use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style, Stylize, palette::tailwind},
    symbols,
    text::{Line, Span},
    widgets::{Axis, Block, Chart, Dataset, GraphType, LineGauge, Padding, Widget},
};
use std::collections::VecDeque;
use std::sync::LazyLock;

/// Chart marker selected once at startup.
/// Override with `EC_TUI_MARKER=dot` or `EC_TUI_MARKER=braille`.
/// Default: Braille if the terminal advertises Unicode support, Dot otherwise.
pub(crate) static CHART_MARKER: LazyLock<symbols::Marker> =
    LazyLock::new(|| match std::env::var("EC_TUI_MARKER").as_deref() {
        Ok("dot") => symbols::Marker::Dot,
        Ok("braille") => symbols::Marker::Braille,
        _ => {
            if supports_unicode::on(supports_unicode::Stream::Stdout) {
                symbols::Marker::Braille
            } else {
                symbols::Marker::Dot
            }
        }
    });

#[derive(Default)]
pub struct SampleBuf<T, const N: usize> {
    samples: VecDeque<T>,
}

impl<T: Into<f64> + Copy, const N: usize> SampleBuf<T, N> {
    // Insert a sample into the buffer and evict the oldest if full
    pub fn insert(&mut self, sample: T) {
        self.samples.push_back(sample);
        if self.samples.len() > N {
            self.samples.pop_front();
        }
    }

    // Converts the buffer into a format that ratatui can use
    // Probably more efficent way than copying but buffer is small and only called once a second
    pub fn get(&self) -> Vec<(f64, f64)> {
        self.samples
            .iter()
            .enumerate()
            .map(|(i, &val)| (i as f64, val.into()))
            .collect()
    }
}

// Properties for rendering a graph
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

// Split an area in a direction with given percentages
pub fn area_split(area: Rect, direction: Direction, first: u16, second: u16) -> [Rect; 2] {
    // SAFETY: We always split into exactly 2 constraints, so the conversion is infallible.
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
///
/// The dot is green on success and red on failure, providing a compact
/// visual health indicator that renders reliably without terminal emoji.
pub fn status_title(title: impl Into<String>, success: bool) -> Line<'static> {
    let (dot, color) = if success {
        ("● ", tailwind::GREEN.c400)
    } else {
        ("● ", tailwind::RED.c500)
    };
    Line::from(vec![
        Span::styled(dot, Style::default().fg(color)),
        Span::raw(title.into()),
    ])
}

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

pub fn time_labels(t: usize, max_samples: usize) -> [Span<'static>; 3] {
    let (start, mid, end) = if t <= max_samples {
        (0, max_samples / 2, max_samples)
    } else {
        (t - max_samples, t - max_samples / 2, t)
    };
    [
        Span::styled(start.to_string(), Style::default().bold()),
        Span::styled(mid.to_string(), Style::default().bold()),
        Span::styled(end.to_string(), Style::default().bold()),
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
///
/// Bands are drawn left-to-right in ascending `value` order. The `ratio`
/// (0.0–1.0) is the current fill fraction; the gauge itself is filled up to
/// that point using whichever band color applies.
pub struct ThresholdGauge<'a> {
    pub ratio: f64,
    pub label: Option<Span<'a>>,
    /// (threshold_ratio, color_above) pairs sorted ascending.  The last
    /// entry's color is used for any ratio above that threshold.
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

/// Minimal test doubles shared across module test suites.
#[cfg(test)]
pub(crate) mod test_support {
    use ec_test_lib::{Error as EcError, ErrorKind};

    /// A zero-size error type that always maps to [`ErrorKind::Other`].
    #[derive(Debug)]
    pub(crate) struct TestError;

    impl std::fmt::Display for TestError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "test error")
        }
    }

    impl std::error::Error for TestError {}

    impl EcError for TestError {
        fn kind(&self) -> ErrorKind {
            ErrorKind::Other
        }
    }
}
