use crate::battery::Battery;
use crate::logging::LogBuffer;
use crate::rtc::Rtc;
use crate::state::{AppState, BatteryCommand, ThermalCommand};
use crate::system::System;
use crate::thermal::Thermal;

use color_eyre::Result;
use tracing::{Level, debug, info};

use ratatui::{
    DefaultTerminal,
    buffer::Buffer,
    crossterm::event::{self, Event, KeyCode, KeyEventKind},
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Stylize, palette::tailwind},
    text::{Line, Span},
    widgets::{Block, Padding, Paragraph, Tabs, Widget},
};

use std::{
    sync::{Arc, RwLock, mpsc},
    time::{Duration, Instant},
};

use strum::{Display, EnumIter, FromRepr, IntoEnumIterator};

/// Enum wrapping all UI modules.  Methods dispatch to the concrete type
/// without dynamic dispatch or heap allocation.
pub(crate) enum TabModule {
    Battery(Battery),
    Thermal(Thermal),
    Rtc(Rtc),
    System(System),
}

impl TabModule {
    pub(crate) fn title(&self) -> &'static str {
        match self {
            Self::Battery(_) => "Battery Information",
            Self::Thermal(_) => "Thermal Information",
            Self::Rtc(_) => "RTC Information",
            Self::System(_) => "System Information",
        }
    }

    pub(crate) fn handle_event(&mut self, evt: &Event) {
        match self {
            Self::Battery(m) => m.handle_event(evt),
            Self::Thermal(m) => m.handle_event(evt),
            Self::Rtc(m) => m.handle_event(evt),
            Self::System(m) => m.handle_event(evt),
        }
    }

    pub(crate) fn render(&self, state: &AppState, area: Rect, buf: &mut Buffer) {
        match self {
            Self::Battery(m) => m.render(state, area, buf),
            Self::Thermal(m) => m.render(state, area, buf),
            Self::Rtc(m) => m.render(state, area, buf),
            Self::System(m) => m.render(state, area, buf),
        }
    }

    pub(crate) fn render_card(&self, state: &AppState, area: Rect, buf: &mut Buffer) {
        match self {
            Self::Battery(m) => m.render_card(state, area, buf),
            Self::Thermal(m) => m.render_card(state, area, buf),
            Self::Rtc(m) => m.render_card(state, area, buf),
            Self::System(m) => m.render_card(state, area, buf),
        }
    }
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum RunState {
    #[default]
    Running,
    Quitting,
}

#[derive(Default, Clone, Copy, Display, FromRepr, EnumIter, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum SelectedTab {
    #[default]
    #[strum(to_string = "Dashboard")]
    TabDashboard,
    #[strum(to_string = "Battery")]
    TabBattery,
    #[strum(to_string = "Thermal")]
    TabThermal,
    #[strum(to_string = "RTC")]
    TabRTC,
    #[strum(to_string = "System")]
    TabSystem,
}

/// The main application: holds UI state and a read handle on the shared data.
pub struct App {
    run_state: RunState,
    selected_tab: SelectedTab,
    modules: [TabModule; 4],
    shared_state: Arc<RwLock<AppState>>,
    log_buffer: LogBuffer,
    log_visible: bool,
    log_scroll: usize,
}

impl App {
    /// Construct the application.
    ///
    /// * `shared_state` — populated by the background updater thread.
    /// * `battery_tx` / `thermal_tx` — command channels for hardware write-backs.
    pub fn new(
        shared_state: Arc<RwLock<AppState>>,
        battery_tx: mpsc::Sender<BatteryCommand>,
        thermal_tx: mpsc::Sender<ThermalCommand>,
        log_buffer: LogBuffer,
    ) -> Self {
        let modules = [
            TabModule::Battery(Battery::new(battery_tx)),
            TabModule::Thermal(Thermal::new(thermal_tx)),
            TabModule::Rtc(Rtc::new()),
            TabModule::System(System::new()),
        ];

        let app = Self {
            run_state: Default::default(),
            selected_tab: Default::default(),
            modules,
            shared_state,
            log_buffer,
            log_visible: false,
            log_scroll: 0,
        };
        info!("application initialized");
        app
    }

    /// Run the application's main loop.
    pub fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        let tick_rate = Duration::from_millis(250);
        let mut last_tick = Instant::now();

        info!("entering main loop");

        while self.run_state == RunState::Running {
            terminal.draw(|frame| frame.render_widget(&self, frame.area()))?;

            let timeout = tick_rate.saturating_sub(last_tick.elapsed());
            if event::poll(timeout)? {
                self.handle_events()?;
            }

            if last_tick.elapsed() >= tick_rate {
                last_tick = Instant::now();
            }
        }

        info!("exiting main loop");
        Ok(())
    }

    fn handle_events(&mut self) -> std::io::Result<()> {
        let evt = event::read()?;
        if let Event::Key(key) = evt
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Right => self.next_tab(),
                KeyCode::Left => self.previous_tab(),
                KeyCode::Up if self.log_visible => {
                    self.log_scroll = self.log_scroll.saturating_add(1);
                }
                KeyCode::Down if self.log_visible => {
                    self.log_scroll = self.log_scroll.saturating_sub(1);
                }
                KeyCode::Char('1') => self.selected_tab = SelectedTab::TabDashboard,
                KeyCode::Char('2') => self.selected_tab = SelectedTab::TabBattery,
                KeyCode::Char('3') => self.selected_tab = SelectedTab::TabThermal,
                KeyCode::Char('4') => self.selected_tab = SelectedTab::TabRTC,
                KeyCode::Char('5') => self.selected_tab = SelectedTab::TabSystem,
                KeyCode::Char('l') => {
                    self.log_visible = !self.log_visible;
                    if self.log_visible {
                        self.log_scroll = 0;
                    }
                }
                KeyCode::Char('q') | KeyCode::Esc => self.quit(),
                _ => self.handle_tab_event(&evt),
            }
        }
        Ok(())
    }

    fn handle_tab_event(&mut self, evt: &Event) {
        if let Some(i) = self.selected_tab.module_index() {
            self.modules[i].handle_event(evt);
        }
    }

    fn next_tab(&mut self) {
        self.selected_tab = self.selected_tab.next();
        debug!(tab = %self.selected_tab, "switched to next tab");
    }

    fn previous_tab(&mut self) {
        self.selected_tab = self.selected_tab.previous();
        debug!(tab = %self.selected_tab, "switched to previous tab");
    }

    fn quit(&mut self) {
        info!("quit requested");
        self.run_state = RunState::Quitting;
    }

    fn render_tabs(&self, area: Rect, buf: &mut Buffer) {
        let titles = SelectedTab::iter().map(SelectedTab::title);
        let highlight_style = (Color::default(), self.selected_tab.palette().c700);
        let selected_tab_index = self.selected_tab as usize;
        Tabs::new(titles)
            .highlight_style(highlight_style)
            .select(selected_tab_index)
            .padding("", "")
            .divider(" ")
            .render(area, buf);
    }

    fn render_selected_tab(&self, state: &AppState, area: Rect, buf: &mut Buffer) {
        if self.selected_tab == SelectedTab::TabDashboard {
            self.render_dashboard(state, area, buf);
            return;
        }

        if let Some(i) = self.selected_tab.module_index() {
            let module = &self.modules[i];
            let block = self
                .selected_tab
                .block()
                .title(Line::from(module.title()).bold().centered());
            let inner = block.inner(area);
            block.render(area, buf);
            module.render(state, inner, buf);
        }
    }

    fn render_dashboard(&self, state: &AppState, area: Rect, buf: &mut Buffer) {
        let block = SelectedTab::TabDashboard
            .block()
            .title(Line::from("System Overview").bold().centered());
        let inner = block.inner(area);
        block.render(area, buf);

        let n = self.modules.len() as u16;
        let constraints: Vec<Constraint> = (0..n).map(|_| Constraint::Ratio(1, n as u32)).collect();
        let card_areas = Layout::horizontal(constraints).split(inner);

        for (i, module) in self.modules.iter().enumerate() {
            module.render_card(state, card_areas[i], buf);
        }
    }
}

impl Widget for &App {
    fn render(self, area: Rect, buf: &mut Buffer) {
        use Constraint::{Length, Min};

        let state = self.shared_state.read().expect("state RwLock poisoned");

        let log_height = if self.log_visible { LOG_PANEL_HEIGHT } else { 0 };
        let vertical = Layout::vertical([Length(1), Min(0), Length(log_height), Length(1)]);
        let [header_area, inner_area, log_area, footer_area] = vertical.areas(area);

        let horizontal = Layout::horizontal([Min(0), Length(20)]);
        let [tabs_area, title_area] = horizontal.areas(header_area);

        render_title(title_area, buf);
        self.render_tabs(tabs_area, buf);
        self.render_selected_tab(&state, inner_area, buf);
        if self.log_visible {
            self.render_log_panel(log_area, buf);
        }
        self.render_footer(footer_area, buf);
    }
}

impl Drop for App {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

// ── Log panel ─────────────────────────────────────────────────────────────────

/// Height of the log panel in terminal rows (includes the border).
const LOG_PANEL_HEIGHT: u16 = 8;

fn level_color(level: Level) -> Color {
    match level {
        Level::ERROR => tailwind::RED.c400,
        Level::WARN => tailwind::AMBER.c400,
        Level::INFO => tailwind::GREEN.c400,
        Level::DEBUG => tailwind::SKY.c400,
        Level::TRACE => tailwind::SLATE.c500,
    }
}

impl App {
    fn render_log_panel(&self, area: Rect, buf: &mut Buffer) {
        let entries = self.log_buffer.entries();
        let visible_rows = area.height.saturating_sub(2) as usize;

        let max_scroll = entries.len().saturating_sub(visible_rows);
        let scroll = self.log_scroll.min(max_scroll);
        let skip = max_scroll.saturating_sub(scroll);
        let lines: Vec<Line<'_>> = entries[skip..]
            .iter()
            .take(visible_rows)
            .map(|entry| {
                Line::from(vec![
                    Span::styled(entry.timestamp.clone(), Style::default().fg(tailwind::SLATE.c500)),
                    Span::raw(" "),
                    Span::styled(
                        format!("[{:<5}]", entry.level),
                        Style::default().fg(level_color(entry.level)),
                    ),
                    Span::raw(format!(" {}: {}", entry.target, entry.message)),
                ])
            })
            .collect();

        let scroll_label = if scroll > 0 {
            format!(" Logs [↑{scroll}] ")
        } else {
            " Logs ".to_owned()
        };

        Paragraph::new(lines)
            .block(
                Block::bordered()
                    .title(Line::from(scroll_label).bold())
                    .title(
                        Line::from(Span::styled(
                            " RUST_LOG=<level> ",
                            Style::default().fg(tailwind::SLATE.c600),
                        ))
                        .right_aligned(),
                    )
                    .border_style(Style::default().fg(tailwind::SLATE.c700)),
            )
            .render(area, buf);
    }

    fn render_footer(&self, area: Rect, buf: &mut Buffer) {
        let key = Style::default()
            .fg(tailwind::SLATE.c100)
            .bg(tailwind::SLATE.c700)
            .bold();
        let desc = Style::default().fg(tailwind::SLATE.c500);
        let log_hint = if self.log_visible { " hide logs" } else { " show logs" };
        let mut spans = vec![
            Span::styled(" ◄ ► ", key),
            Span::styled(" switch tab  ", desc),
            Span::styled(" 1-5 ", key),
            Span::styled(" jump to tab  ", desc),
            Span::styled(" l ", key),
            Span::styled(log_hint, desc),
        ];
        if self.log_visible {
            spans.extend([Span::styled("  ↑ ↓ ", key), Span::styled(" scroll logs  ", desc)]);
        }
        spans.extend([Span::styled("  q ", key), Span::styled(" quit", desc)]);
        Line::from(spans).centered().render(area, buf);
    }
}

impl SelectedTab {
    /// Returns the index into `App::modules` for this tab, or `None` for
    /// Dashboard which renders all module cards inline.
    fn module_index(self) -> Option<usize> {
        match self {
            Self::TabDashboard => None,
            Self::TabBattery => Some(0),
            Self::TabThermal => Some(1),
            Self::TabRTC => Some(2),
            Self::TabSystem => Some(3),
        }
    }

    /// Get the previous tab, if there is no previous tab return the current tab.
    fn previous(self) -> Self {
        let current_index: usize = self as usize;
        let previous_index = current_index.saturating_sub(1);
        Self::from_repr(previous_index).unwrap_or(self)
    }

    /// Get the next tab, if there is no next tab return the current tab.
    fn next(self) -> Self {
        let current_index = self as usize;
        let next_index = current_index.saturating_add(1);
        Self::from_repr(next_index).unwrap_or(self)
    }
}

fn render_title(area: Rect, buf: &mut Buffer) {
    Line::from(Span::styled(
        "ODP EC Monitor",
        Style::default().fg(tailwind::SLATE.c400).bold(),
    ))
    .right_aligned()
    .render(area, buf);
}

impl SelectedTab {
    /// Return tab's name as a styled `Line`
    fn title(self) -> Line<'static> {
        format!("  {self}  ")
            .fg(tailwind::SLATE.c200)
            .bg(self.palette().c900)
            .into()
    }

    /// A block surrounding the tab's content
    fn block(self) -> Block<'static> {
        Block::bordered()
            .padding(Padding::uniform(1))
            .border_style(self.palette().c500)
    }

    const fn palette(self) -> tailwind::Palette {
        match self {
            Self::TabDashboard => tailwind::SLATE,
            Self::TabBattery => tailwind::SKY,
            Self::TabThermal => tailwind::ORANGE,
            Self::TabRTC => tailwind::VIOLET,
            Self::TabSystem => tailwind::EMERALD,
        }
    }
}
