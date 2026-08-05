use crate::common;
use crate::common::SYMBOLS;
use crate::state::{Fetched, UcsiState};
use ec_test_lib::ucsi::{PowerDirection, UcsiCapability, UcsiConnectorCapability, UcsiConnectorStatus};
use ratatui::{
    buffer::Buffer,
    crossterm::event::Event,
    layout::Rect,
    style::{Color, palette::tailwind},
    text::Line,
    widgets::{Block, Paragraph, Widget},
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
        self.render_titled("USB-C (UCSI)", tailwind::CYAN.c600, state, area, buf);
    }

    pub(crate) fn render_card(&self, state: &UcsiState, area: Rect, buf: &mut Buffer) {
        self.render_titled("USB-C", tailwind::CYAN.c700, state, area, buf);
    }

    /// Both the tab and the dashboard card render the same metric rows inside a
    /// bordered block; only the title and border colour differ.
    fn render_titled(&self, title: &str, border: Color, state: &UcsiState, area: Rect, buf: &mut Buffer) {
        let is_healthy = matches!(state.connector_status, Some(Ok(_)));
        let block = Block::bordered()
            .title(common::status_title(title, is_healthy))
            .border_style(border);
        let inner = block.inner(area);
        block.render(area, buf);
        Paragraph::new(rows(state)).render(inner, buf);
    }
}

// ── Shared row/summary builder ────────────────────────────────────────────────

fn rows(s: &UcsiState) -> Vec<Line<'static>> {
    vec![
        common::metric_row("Version", cell(&s.version, |v| v.to_string()), LABEL_COLOR),
        common::metric_row("Capability", cell(&s.capability, capability_summary), LABEL_COLOR),
        common::metric_row("Conn 1", cell(&s.connector_capability, connector_summary), LABEL_COLOR),
        common::metric_row("Status", cell(&s.connector_status, status_summary), LABEL_COLOR),
    ]
}

/// Render a fetched cell as honest pending / error / value text.
fn cell<T>(fetched: &Fetched<T>, f: impl FnOnce(&T) -> String) -> String {
    match fetched {
        None => "Pending...".to_string(),
        Some(Err(e)) => format!("Error: {e}"),
        Some(Ok(v)) => f(v),
    }
}

fn capability_summary(cap: &UcsiCapability) -> String {
    let connector = if cap.num_connectors == 1 {
        "connector"
    } else {
        "connectors"
    };
    let pd_support = if cap.attributes.usb_power_delivery() {
        "USB-PD"
    } else {
        "no USB-PD"
    };
    format!(
        "{} {connector} {} {pd_support} {} PD {:x}.{:02x}",
        cap.num_connectors,
        SYMBOLS.mid_dot,
        SYMBOLS.mid_dot,
        cap.bcd_usb_pd_spec >> 8,
        cap.bcd_usb_pd_spec & 0xff,
    )
}

fn connector_summary(cap: &UcsiConnectorCapability) -> String {
    let modes_flags = cap.operation_mode();
    let mut modes = Vec::new();
    if modes_flags.drp() {
        modes.push("DRP");
    }
    if modes_flags.usb2() {
        modes.push("USB2");
    }
    if modes_flags.usb3() {
        modes.push("USB3");
    }
    let roles = match (cap.provider(), cap.consumer()) {
        (true, true) => "provider/consumer",
        (true, false) => "provider",
        (false, true) => "consumer",
        (false, false) => "-",
    };
    let modes = if modes.is_empty() {
        "none".to_string()
    } else {
        modes.join("/")
    };
    format!("{modes} {} {roles}", SYMBOLS.mid_dot)
}

fn status_summary(status: &UcsiConnectorStatus) -> String {
    if !status.connect_status {
        return "Disconnected".to_string();
    }
    let (direction, partner) = match &status.status {
        Some(connected) => {
            let direction = match connected.power_direction {
                PowerDirection::Sink => "Sink",
                PowerDirection::Source => "Source",
            };
            let partner = if connected.partner_flags.usb() {
                "USB"
            } else {
                "partner"
            };
            (direction, partner)
        }
        None => ("?", "partner"),
    };
    format!(
        "Connected {} {direction} {} {partner}",
        SYMBOLS.mid_dot, SYMBOLS.mid_dot
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ec_test_lib::UcsiSource;
    use ec_test_lib::mock::Mock;

    #[test]
    fn capability_summary_separates_connector_and_pd_details() {
        let cap = Mock::default().get_capability().unwrap();
        assert_eq!(capability_summary(&cap), "1 connector · USB-PD · PD 3.00");
    }

    #[test]
    fn status_summary_renders_connected_sink() {
        let status = Mock::default().get_connector_status(1).unwrap();
        assert_eq!(status_summary(&status), "Connected · Sink · USB");
    }
}
