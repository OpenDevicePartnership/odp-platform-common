use crate::common;
use crate::common::SYMBOLS;
use crate::state::{Fetched, UcsiState};
use ec_test_lib::ucsi::{UcsiConnectorCapability, UcsiConnectorStatus};
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

fn capability_summary(cap: &ec_test_lib::ucsi::UcsiCapability) -> String {
    format!(
        "{} conn{}  PD {:x}.{:02x}",
        cap.num_connectors,
        if cap.usb_pd_supported { " USB-PD" } else { "" },
        cap.bcd_pd_version >> 8,
        cap.bcd_pd_version & 0xff,
    )
}

fn connector_summary(cap: &UcsiConnectorCapability) -> String {
    let mut modes = Vec::new();
    if cap.drp {
        modes.push("DRP");
    }
    if cap.usb2 {
        modes.push("USB2");
    }
    if cap.usb3 {
        modes.push("USB3");
    }
    let roles = match (cap.provider, cap.consumer) {
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
    if !status.connected {
        return "Disconnected".to_string();
    }
    let partner = if status.partner_usb { "USB" } else { "partner" };
    format!(
        "Connected {} {} {} {partner}",
        SYMBOLS.mid_dot, status.power_direction, SYMBOLS.mid_dot
    )
}
