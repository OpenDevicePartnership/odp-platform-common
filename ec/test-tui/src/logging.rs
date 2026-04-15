use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use tracing::Level;
use tracing_subscriber::{
    fmt::{
        format::Writer as FmtWriter,
        time::{FormatTime, UtcTime},
    },
    layer::Context,
};

const LOG_CAPACITY: usize = 500;

#[derive(Clone, Debug)]
pub struct LogEntry {
    pub level: Level,
    pub target: String,
    pub message: String,
    /// UTC wall-clock time in RFC 3339 format.
    pub timestamp: String,
}

/// A clonable shared circular buffer that accumulates [`LogEntry`] items.
///
/// All clones share the same underlying storage (backed by `Arc`).
#[derive(Clone, Default)]
pub struct LogBuffer {
    inner: Arc<Mutex<VecDeque<LogEntry>>>,
}

impl LogBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, entry: LogEntry) {
        let mut q = self.inner.lock().expect("log buffer lock poisoned");
        if q.len() >= LOG_CAPACITY {
            q.pop_front();
        }
        q.push_back(entry);
    }

    /// Returns all entries, oldest first.
    pub fn entries(&self) -> Vec<LogEntry> {
        self.inner
            .lock()
            .expect("log buffer lock poisoned")
            .iter()
            .cloned()
            .collect()
    }
}

// ── tracing layer ─────────────────────────────────────────────────────────────

struct MessageVisitor(String);

impl tracing::field::Visit for MessageVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.0 = value.to_owned();
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

/// A [`tracing_subscriber::Layer`] that feeds events into a [`LogBuffer`].
pub struct TuiLayer {
    buffer: LogBuffer,
}

impl TuiLayer {
    pub fn new(buffer: LogBuffer) -> Self {
        Self { buffer }
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for TuiLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);

        let mut timestamp = String::new();
        let mut w = FmtWriter::new(&mut timestamp);
        UtcTime::rfc_3339().format_time(&mut w).ok();

        self.buffer.push(LogEntry {
            level: *event.metadata().level(),
            target: event.metadata().target().to_owned(),
            message: visitor.0,
            timestamp,
        });
    }
}
