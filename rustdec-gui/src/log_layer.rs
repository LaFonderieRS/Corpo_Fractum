//! Custom tracing layer that forwards log records to the GTK console.
//!
//! `GtkLogLayer` implements `tracing_subscriber::Layer` and sends every event
//! through an `async_channel`. The receiver is meant to be drained on the GTK
//! main thread (see `ConsolePanel`).

use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

// ── Public types ──────────────────────────────────────────────────────────────

/// A single log record forwarded to the GTK console.
#[derive(Debug, Clone)]
pub struct LogRecord {
    pub level:   Level,
    pub target:  String,
    pub message: String,
}

// ── Layer ─────────────────────────────────────────────────────────────────────

/// Tracing layer that sends log records through a bounded async channel.
///
/// The `Sender` is `Send + Sync`, so this layer works with any multi-threaded
/// subscriber registry. The GTK side drains the corresponding `Receiver` on
/// the main thread via `glib::MainContext::spawn_local`.
pub struct GtkLogLayer {
    tx: async_channel::Sender<LogRecord>,
}

impl GtkLogLayer {
    /// Build the layer and the corresponding receiver.
    pub fn new() -> (Self, async_channel::Receiver<LogRecord>) {
        let (tx, rx) = async_channel::unbounded();
        (Self { tx }, rx)
    }
}

impl<S: Subscriber> Layer<S> for GtkLogLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);

        let record = LogRecord {
            level:   *event.metadata().level(),
            target:  event.metadata().target().to_owned(),
            message: visitor.message,
        };

        // Non-blocking: if the receiver is gone or the buffer is full we drop silently.
        let _ = self.tx.try_send(record);
    }
}

// ── Field visitor ─────────────────────────────────────────────────────────────

#[derive(Default)]
struct MessageVisitor {
    message: String,
}

impl tracing::field::Visit for MessageVisitor {
    /// Called for `&str` literal fields (rare for the message field).
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_owned();
        }
    }

    /// Called for all other fields, including `format_args!` messages.
    /// `fmt::Arguments` implements `Debug` via `Display`, so we get the
    /// formatted string without extra quotes.
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        }
    }
}
