use crate::common;
use crate::common::SYMBOLS;
use crate::state::{Fetched, UcsiState};
use ec_test_lib::ucsi::{UcsiCapability, UcsiConnectorCapability, UcsiConnectorStatus, UcsiVersion};
use ratatui::{
    buffer::Buffer,
    crossterm::event::Event,
    layout::{Constraint, Layout, Rect},
    prelude::*,
    style::{Color, Style, Stylize, palette::tailwind},
    text::{Line, Span},
    widgets::{Block, Paragraph},
};

const LABEL_COLOR: Color = tailwind::CYAN.c300;

/// USB-C / UCSI UI module — stateless; all data is read from [`UcsiState`].
pub struct Ucsi;

impl Ucsi {
    pub fn new() -> Self {
        Self
    }

    pub(crate) fn handle_event(&mut self, _evt: &Event) {}

    pub(crate) fn render(&self, state: &UcsiState, area: Rect, buf: &mut Buffer) {
        use Constraint::{Length, Min};

        let is_healthy = matches!(state.version, Some(Ok(_))) && matches!(state.connector_status, Some(Ok(_)));

        let [version_area, bottom_area] = Layout::vertical([Length(4), Min(0)]).areas(area);
        let [cap_area, conn_area] =
            Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(bottom_area);

        let block = Block::bordered()
            .title(common::status_title("USB-C (UCSI)", is_healthy))
            .border_style(tailwind::CYAN.c600);
        let inner = block.inner(version_area);
        block.render(version_area, buf);
        Paragraph::new(vec![
            Line::from(Span::styled(
                format!("UCSI {}", format_version(&state.version)),
                Style::default().fg(Color::White).bold(),
            )),
            Line::from(format_capability_line(&state.capability)),
        ])
        .render(inner, buf);

        self.render_connector_capability(state, cap_area, buf);
        self.render_connector_status(state, conn_area, buf);
    }

    pub(crate) fn render_card(&self, state: &UcsiState, area: Rect, buf: &mut Buffer) {
        use Constraint::{Length, Min};

        let is_healthy = matches!(state.connector_status, Some(Ok(_)));
        let block = Block::bordered()
            .title(common::status_title("USB-C", is_healthy))
            .border_style(tailwind::CYAN.c700);
        let inner = block.inner(area);
        block.render(area, buf);

        let [version_area, meta_area, status_area] = Layout::vertical([Length(1), Length(2), Min(0)]).areas(inner);

        Line::from(Span::styled(
            format!("UCSI {}", format_version(&state.version)),
            Style::default().fg(Color::White).bold(),
        ))
        .render(version_area, buf);

        Paragraph::new(vec![
            common::metric_row("Connectors", format_connectors(&state.capability), LABEL_COLOR),
            common::metric_row(
                "Conn 1",
                format_connector_capability(&state.connector_capability),
                LABEL_COLOR,
            ),
        ])
        .render(meta_area, buf);

        Paragraph::new(vec![common::metric_row(
            "Status",
            format_status(&state.connector_status),
            LABEL_COLOR,
        )])
        .render(status_area, buf);
    }

    fn render_connector_capability(&self, state: &UcsiState, area: Rect, buf: &mut Buffer) {
        let is_ok = matches!(state.connector_capability, Some(Ok(_)));
        let lines: Vec<Line<'_>> = match &state.connector_capability {
            None => vec![Line::raw("Pending...")],
            Some(Err(e)) => vec![Line::raw(format!("Error: {e}"))],
            Some(Ok(cap)) => vec![
                Line::raw(format!("Modes:    {}", format_operation_mode(cap))),
                Line::raw(format!("Provider: {}", yes_no(cap.provider))),
                Line::raw(format!("Consumer: {}", yes_no(cap.consumer))),
            ],
        };
        Paragraph::new(lines)
            .block(common::title_block(
                common::status_title("Connector 1 Capability", is_ok),
                0,
                LABEL_COLOR,
            ))
            .render(area, buf);
    }

    fn render_connector_status(&self, state: &UcsiState, area: Rect, buf: &mut Buffer) {
        let is_ok = matches!(state.connector_status, Some(Ok(_)));
        let lines: Vec<Line<'_>> = match &state.connector_status {
            None => vec![Line::raw("Pending...")],
            Some(Err(e)) => vec![Line::raw(format!("Error: {e}"))],
            Some(Ok(status)) => vec![
                Line::raw(format!("Attached:  {}", yes_no(status.connected))),
                Line::raw(format!("Direction: {}", status.power_direction)),
                Line::raw(format!("Partner:   {}", if status.partner_usb { "USB" } else { "-" })),
            ],
        };
        Paragraph::new(lines)
            .block(common::title_block(
                common::status_title("Connector 1 Status", is_ok),
                0,
                LABEL_COLOR,
            ))
            .render(area, buf);
    }
}

// ── Formatting helpers ────────────────────────────────────────────────────────

fn yes_no(v: bool) -> &'static str {
    if v { "Yes" } else { "No" }
}

fn format_version(version: &Fetched<UcsiVersion>) -> String {
    match version {
        None => "Pending...".to_string(),
        Some(Err(_)) => "Error".to_string(),
        Some(Ok(v)) => v.to_string(),
    }
}

fn format_connectors(capability: &Fetched<UcsiCapability>) -> String {
    match capability {
        None => "Pending...".to_string(),
        Some(Err(_)) => "Error".to_string(),
        Some(Ok(cap)) => cap.num_connectors.to_string(),
    }
}

fn format_capability_line(capability: &Fetched<UcsiCapability>) -> Span<'static> {
    let text = match capability {
        None => "Pending...".to_string(),
        Some(Err(e)) => format!("Error: {e}"),
        Some(Ok(cap)) => format!(
            "{} connector(s){}  PD {:x}.{:02x}",
            cap.num_connectors,
            if cap.usb_pd_supported { "  USB-PD" } else { "" },
            cap.bcd_pd_version >> 8,
            cap.bcd_pd_version & 0xff,
        ),
    };
    Span::styled(text, Style::default().fg(tailwind::SLATE.c400))
}

fn format_operation_mode(cap: &UcsiConnectorCapability) -> String {
    let mut modes = Vec::new();
    if cap.operation_mode.drp {
        modes.push("DRP");
    }
    if cap.operation_mode.usb2 {
        modes.push("USB2");
    }
    if cap.operation_mode.usb3 {
        modes.push("USB3");
    }
    if modes.is_empty() {
        "none".to_string()
    } else {
        modes.join(&format!(" {} ", SYMBOLS.mid_dot))
    }
}

fn format_connector_capability(cap: &Fetched<UcsiConnectorCapability>) -> String {
    match cap {
        None => "Pending...".to_string(),
        Some(Err(_)) => "Error".to_string(),
        Some(Ok(c)) => {
            let roles = match (c.provider, c.consumer) {
                (true, true) => "provider/consumer",
                (true, false) => "provider",
                (false, true) => "consumer",
                (false, false) => "-",
            };
            format!("{} {} {roles}", format_operation_mode(c), SYMBOLS.mid_dot)
        }
    }
}

fn format_status(status: &Fetched<UcsiConnectorStatus>) -> String {
    match status {
        None => "Pending...".to_string(),
        Some(Err(_)) => "Error".to_string(),
        Some(Ok(s)) => {
            if !s.connected {
                "Disconnected".to_string()
            } else {
                let partner = if s.partner_usb { "USB" } else { "partner" };
                format!(
                    "Connected {} {} {} {partner}",
                    SYMBOLS.mid_dot, s.power_direction, SYMBOLS.mid_dot
                )
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ec_test_lib::ucsi::{OperationMode, PowerDirection};

    #[test]
    fn status_connected_sink_summary() {
        let status = Some(Ok(UcsiConnectorStatus {
            connected: true,
            power_direction: PowerDirection::Sink,
            partner_usb: true,
        }));
        let s = format_status(&status);
        assert!(s.contains("Connected"), "{s}");
        assert!(s.contains("Sink"), "{s}");
        assert!(s.contains("USB"), "{s}");
    }

    #[test]
    fn status_disconnected_summary() {
        let status = Some(Ok(UcsiConnectorStatus {
            connected: false,
            power_direction: PowerDirection::Source,
            partner_usb: false,
        }));
        assert_eq!(format_status(&status), "Disconnected");
    }

    #[test]
    fn status_pending_and_error() {
        assert_eq!(format_status(&None), "Pending...");
        assert_eq!(format_status(&Some(Err(color_eyre::eyre::eyre!("x")))), "Error");
    }

    #[test]
    fn operation_mode_lists_enabled_flags() {
        let cap = UcsiConnectorCapability {
            operation_mode: OperationMode {
                drp: true,
                usb2: true,
                usb3: false,
            },
            provider: true,
            consumer: true,
        };
        let s = format_operation_mode(&cap);
        assert!(s.contains("DRP") && s.contains("USB2"), "{s}");
        assert!(!s.contains("USB3"), "{s}");
    }
}
