use ratatui::{
    buffer::Buffer,
    crossterm::event::Event,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style, Stylize, palette::tailwind},
    text::{Line, Span},
    widgets::{Axis, Block, Chart, Dataset, GraphType, Paragraph, Widget},
};

use crate::common::{self, CHART_MARKER, Graph};
use crate::state::{AppState, SYSTEM_MAX_SAMPLES};

const LABEL_COLOR: Color = tailwind::SLATE.c400;
const CPU_COLOR: Color = tailwind::EMERALD.c400;
const MEM_COLOR: Color = tailwind::SKY.c400;
const NET_RX_COLOR: Color = tailwind::VIOLET.c400;
const NET_TX_COLOR: Color = tailwind::AMBER.c400;

// ── Public struct ─────────────────────────────────────────────────────────────

/// Stateless UI module for system-level metrics (CPU / Memory / Network).
pub struct System;

impl System {
    pub fn new() -> Self {
        Self
    }

    pub fn handle_event(&mut self, _evt: &Event) {}

    pub fn render(&self, state: &AppState, area: Rect, buf: &mut Buffer) {
        let [top_area, bottom_area] =
            Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(area);
        let [cpu_area, mem_area] = common::area_split(top_area, Direction::Horizontal, 50, 50);

        self.render_cpu(state, cpu_area, buf);
        self.render_memory(state, mem_area, buf);
        self.render_network(state, bottom_area, buf);
    }

    pub fn render_card(&self, state: &AppState, area: Rect, buf: &mut Buffer) {
        use Constraint::{Length, Min};
        let s = &state.system;

        let block = Block::bordered()
            .title(common::status_title(
                "System",
                s.cpu.success && s.memory.success && s.network.success,
            ))
            .border_style(tailwind::EMERALD.c700);
        let inner = block.inner(area);
        block.render(area, buf);

        let [cpu_label, cpu_gauge, mem_label, mem_gauge, net_row, _rest] =
            Layout::vertical([Length(1), Length(1), Length(1), Length(1), Length(1), Min(0)]).areas(inner);

        // CPU
        common::metric_row("CPU", format!("{:.1}%", s.cpu.usage), LABEL_COLOR).render(cpu_label, buf);
        common::ThresholdGauge {
            ratio: s.cpu.usage / 100.0,
            label: Some(Span::raw("")),
            thresholds: &[
                (0.0, tailwind::EMERALD.c500),
                (0.7, tailwind::AMBER.c400),
                (0.9, tailwind::RED.c400),
            ],
            track_color: tailwind::SLATE.c800,
        }
        .render(cpu_gauge, buf);

        // Memory
        let mem_ratio = mem_ratio(s.memory.used_bytes, s.memory.total_bytes);
        common::metric_row(
            "RAM",
            format!(
                "{} / {}",
                format_bytes(s.memory.used_bytes),
                format_bytes(s.memory.total_bytes)
            ),
            LABEL_COLOR,
        )
        .render(mem_label, buf);
        common::ThresholdGauge {
            ratio: mem_ratio,
            label: Some(Span::raw("")),
            thresholds: &[
                (0.0, tailwind::SKY.c500),
                (0.8, tailwind::AMBER.c400),
                (0.95, tailwind::RED.c400),
            ],
            track_color: tailwind::SLATE.c800,
        }
        .render(mem_gauge, buf);

        // Network
        Line::from(vec![
            Span::styled("↓ ", Style::default().fg(NET_RX_COLOR)),
            Span::raw(format_bps(s.network.rx_bps)),
            Span::raw("  "),
            Span::styled("↑ ", Style::default().fg(NET_TX_COLOR)),
            Span::raw(format_bps(s.network.tx_bps)),
        ])
        .render(net_row, buf);
    }
}

impl Default for System {
    fn default() -> Self {
        Self::new()
    }
}

// ── Private render helpers ────────────────────────────────────────────────────

impl System {
    fn render_cpu(&self, state: &AppState, area: Rect, buf: &mut Buffer) {
        use Constraint::{Length, Min};
        let cpu = &state.system.cpu;

        let block = Block::bordered()
            .title(common::status_title("CPU", cpu.success))
            .border_style(tailwind::EMERALD.c700);
        let inner = block.inner(area);
        block.render(area, buf);

        let num_cores = cpu.per_core.len().min(64);
        let core_rows = num_cores.min(16) as u16;
        let [usage_line, usage_gauge, cores_area, chart_area] =
            Layout::vertical([Length(1), Length(1), Length(core_rows), Min(0)]).areas(inner);

        // Overall
        common::metric_row(
            "Overall",
            format!("{:.1}%  ({} cores)", cpu.usage, cpu.per_core.len()),
            LABEL_COLOR,
        )
        .render(usage_line, buf);

        common::ThresholdGauge {
            ratio: cpu.usage / 100.0,
            label: None,
            thresholds: &[
                (0.0, tailwind::EMERALD.c500),
                (0.7, tailwind::AMBER.c400),
                (0.9, tailwind::RED.c400),
            ],
            track_color: tailwind::SLATE.c800,
        }
        .render(usage_gauge, buf);

        // Per-core grid: up to 4 columns × 16 rows = 64 cores
        let cols_used = num_cores.div_ceil(16).clamp(1, 4);
        let col_areas =
            Layout::horizontal((0..cols_used).map(|_| Constraint::Ratio(1, cols_used as u32))).split(cores_area);

        for col in 0..cols_used {
            let start = col * 16;
            let slice = &cpu.per_core[start..cpu.per_core.len().min(start + 16)];
            let lines: Vec<Line<'_>> = slice
                .iter()
                .enumerate()
                .map(|(row, &pct)| {
                    let i = start + row;
                    let p = pct as f64;
                    let color = usage_color(p);
                    Line::from(vec![
                        Span::styled(format!("{i:>2} "), Style::default().fg(LABEL_COLOR)),
                        Span::styled(format!("{p:>5.1}%"), Style::default().fg(color).bold()),
                        Span::raw(" "),
                        Span::styled(mini_bar(p, 8), Style::default().fg(color)),
                    ])
                })
                .collect();
            Paragraph::new(lines).render(col_areas[col], buf);
        }

        // History chart
        if chart_area.height > 3 {
            common::render_chart(
                chart_area,
                buf,
                Graph {
                    title: "CPU Usage %".to_string(),
                    color: CPU_COLOR,
                    samples: cpu.samples.get(),
                    x_axis: String::new(),
                    x_bounds: [0.0, SYSTEM_MAX_SAMPLES as f64],
                    x_labels: common::time_labels(SYSTEM_MAX_SAMPLES),
                    y_axis: "%".to_string(),
                    y_bounds: [0.0, 100.0],
                    y_labels: [
                        Span::styled("0", Style::default().bold()),
                        Span::styled("50", Style::default().bold()),
                        Span::styled("100", Style::default().bold()),
                    ],
                },
            );
        }
    }

    fn render_memory(&self, state: &AppState, area: Rect, buf: &mut Buffer) {
        use Constraint::{Length, Min};
        let mem = &state.system.memory;

        let block = Block::bordered()
            .title(common::status_title("Memory", mem.success))
            .border_style(tailwind::SKY.c700);
        let inner = block.inner(area);
        block.render(area, buf);

        let ram_ratio = mem_ratio(mem.used_bytes, mem.total_bytes);
        let swap_ratio = mem_ratio(mem.swap_used_bytes, mem.swap_total_bytes);

        let [ram_line, ram_gauge, avail_line, swap_line, swap_gauge, chart_area] =
            Layout::vertical([Length(1), Length(1), Length(1), Length(1), Length(1), Min(0)]).areas(inner);

        common::metric_row(
            "RAM   ",
            format!(
                "{} / {}  ({:.1}%)",
                format_bytes(mem.used_bytes),
                format_bytes(mem.total_bytes),
                ram_ratio * 100.0
            ),
            LABEL_COLOR,
        )
        .render(ram_line, buf);

        common::ThresholdGauge {
            ratio: ram_ratio,
            label: None,
            thresholds: &[
                (0.0, tailwind::SKY.c500),
                (0.8, tailwind::AMBER.c400),
                (0.95, tailwind::RED.c400),
            ],
            track_color: tailwind::SLATE.c800,
        }
        .render(ram_gauge, buf);

        common::metric_row(
            "Avail ",
            format_bytes(mem.total_bytes.saturating_sub(mem.used_bytes)),
            LABEL_COLOR,
        )
        .render(avail_line, buf);

        if mem.swap_total_bytes > 0 {
            common::metric_row(
                "Swap  ",
                format!(
                    "{} / {}  ({:.1}%)",
                    format_bytes(mem.swap_used_bytes),
                    format_bytes(mem.swap_total_bytes),
                    swap_ratio * 100.0
                ),
                LABEL_COLOR,
            )
            .render(swap_line, buf);

            common::ThresholdGauge {
                ratio: swap_ratio,
                label: None,
                thresholds: &[
                    (0.0, tailwind::SLATE.c500),
                    (0.5, tailwind::AMBER.c400),
                    (0.8, tailwind::RED.c400),
                ],
                track_color: tailwind::SLATE.c800,
            }
            .render(swap_gauge, buf);
        }

        if chart_area.height > 3 {
            common::render_chart(
                chart_area,
                buf,
                Graph {
                    title: "RAM Usage %".to_string(),
                    color: MEM_COLOR,
                    samples: mem.samples.get(),
                    x_axis: String::new(),
                    x_bounds: [0.0, SYSTEM_MAX_SAMPLES as f64],
                    x_labels: common::time_labels(SYSTEM_MAX_SAMPLES),
                    y_axis: "%".to_string(),
                    y_bounds: [0.0, 100.0],
                    y_labels: [
                        Span::styled("0", Style::default().bold()),
                        Span::styled("50", Style::default().bold()),
                        Span::styled("100", Style::default().bold()),
                    ],
                },
            );
        }
    }

    fn render_network(&self, state: &AppState, area: Rect, buf: &mut Buffer) {
        use Constraint::{Length, Min};
        let net = &state.system.network;

        let block = Block::bordered()
            .title(common::status_title("Network", net.success))
            .border_style(tailwind::VIOLET.c700);
        let inner = block.inner(area);
        block.render(area, buf);

        // Peak rate across both directions for gauge normalisation.
        let rx_vals = net.rx_samples.get();
        let tx_vals = net.tx_samples.get();
        let peak_bps = rx_vals
            .iter()
            .chain(tx_vals.iter())
            .map(|&(_, v)| v)
            .fold(0.0_f64, f64::max)
            .max(1.0);

        let [metrics_area, chart_area] = common::area_split(inner, Direction::Horizontal, 35, 65);

        let [rx_line, rx_gauge, tx_line, tx_gauge, totals_area] =
            Layout::vertical([Length(1), Length(1), Length(1), Length(1), Min(0)]).areas(metrics_area);

        common::metric_row("↓ RX  ", format_bps(net.rx_bps), NET_RX_COLOR).render(rx_line, buf);
        common::ThresholdGauge {
            ratio: (net.rx_bps / peak_bps).clamp(0.0, 1.0),
            label: Some(Span::raw("")),
            thresholds: &[(0.0, NET_RX_COLOR)],
            track_color: tailwind::SLATE.c800,
        }
        .render(rx_gauge, buf);

        common::metric_row("↑ TX  ", format_bps(net.tx_bps), NET_TX_COLOR).render(tx_line, buf);
        common::ThresholdGauge {
            ratio: (net.tx_bps / peak_bps).clamp(0.0, 1.0),
            label: Some(Span::raw("")),
            thresholds: &[(0.0, NET_TX_COLOR)],
            track_color: tailwind::SLATE.c800,
        }
        .render(tx_gauge, buf);

        Paragraph::new(vec![
            common::metric_row("Total ↓", format_bytes(net.total_rx), LABEL_COLOR),
            common::metric_row("Total ↑", format_bytes(net.total_tx), LABEL_COLOR),
        ])
        .render(totals_area, buf);

        if chart_area.height > 3 {
            render_dual_chart(chart_area, buf, &rx_vals, &tx_vals, peak_bps);
        }
    }
}

// ── Dual-dataset network chart ────────────────────────────────────────────────

fn render_dual_chart(area: Rect, buf: &mut Buffer, rx: &[(f64, f64)], tx: &[(f64, f64)], peak: f64) {
    let marker = *CHART_MARKER;
    let datasets = vec![
        Dataset::default()
            .name("↓ RX")
            .marker(marker)
            .style(Style::default().fg(NET_RX_COLOR))
            .graph_type(GraphType::Line)
            .data(rx),
        Dataset::default()
            .name("↑ TX")
            .marker(marker)
            .style(Style::default().fg(NET_TX_COLOR))
            .graph_type(GraphType::Line)
            .data(tx),
    ];

    // Fixed-width labels (10 chars) so ratatui's y-axis column never
    // grows as peak_bps increases, which would shrink the plot area.
    let bps_label = |v: f64| Span::raw(format!("{:>10}", format_bps(v)));

    Chart::new(datasets)
        .block(Block::bordered().title(Line::from("Network Throughput").cyan().bold().centered()))
        .x_axis(
            Axis::default()
                .style(Style::default().gray())
                .bounds([0.0, SYSTEM_MAX_SAMPLES as f64])
                .labels(common::time_labels(SYSTEM_MAX_SAMPLES)),
        )
        .y_axis(
            Axis::default()
                .style(Style::default().gray())
                .bounds([0.0, peak])
                .labels([bps_label(0.0), bps_label(peak / 2.0), bps_label(peak)]),
        )
        .render(area, buf);
}

// ── Pure helper functions ─────────────────────────────────────────────────────

fn mem_ratio(used: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        (used as f64 / total as f64).clamp(0.0, 1.0)
    }
}

fn usage_color(pct: f64) -> Color {
    if pct >= 90.0 {
        tailwind::RED.c400
    } else if pct >= 70.0 {
        tailwind::AMBER.c400
    } else {
        tailwind::EMERALD.c400
    }
}

/// Render a compact ASCII progress bar of the given `width` in characters.
fn mini_bar(pct: f64, width: usize) -> String {
    let filled = ((pct / 100.0) * width as f64).round() as usize;
    let filled = filled.min(width);
    let empty = width - filled;
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}

pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1_024;
    const MB: u64 = KB * 1_024;
    const GB: u64 = MB * 1_024;
    const TB: u64 = GB * 1_024;
    if bytes >= TB {
        format!("{:.1} TiB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.1} GiB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MiB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0} KiB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

pub fn format_bps(bps: f64) -> String {
    const KB: f64 = 1_024.0;
    const MB: f64 = KB * 1_024.0;
    const GB: f64 = MB * 1_024.0;
    if bps >= GB {
        format!("{:.1} GB/s", bps / GB)
    } else if bps >= MB {
        format!("{:.1} MB/s", bps / MB)
    } else if bps >= KB {
        format!("{:.0} KB/s", bps / KB)
    } else {
        format!("{bps:.0} B/s")
    }
}
