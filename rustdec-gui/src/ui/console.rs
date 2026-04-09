//! Console panel — displays analysis lifecycle events and tracing log records.

use std::cell::Cell;
use std::rc::Rc;

use glib::MainContext;
use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, Label, Orientation, PolicyType,
    ScrolledWindow, TextBuffer, TextView, Widget, WrapMode,
};
use tracing::Level;

use crate::bridge::{AnalysisBridge, BridgeEvent};
use crate::log_layer::LogRecord;

pub struct ConsolePanel {
    root: GtkBox,
}

impl ConsolePanel {
    pub fn new(bridge: AnalysisBridge, log_rx: async_channel::Receiver<LogRecord>) -> Self {
        let root = GtkBox::new(Orientation::Vertical, 0);

        let header = Label::new(Some("Console"));
        header.add_css_class("panel-header");
        root.append(&header);

        let buffer = TextBuffer::new(None);

        if let Some(tag) = buffer.create_tag(Some("info"), &[]) {
            tag.set_foreground(Some("#8a8a8a"));
        }
        if let Some(tag) = buffer.create_tag(Some("debug"), &[]) {
            tag.set_foreground(Some("#5a7a8a"));
        }
        if let Some(tag) = buffer.create_tag(Some("warn"), &[]) {
            tag.set_foreground(Some("#d7ba7d"));
        }
        if let Some(tag) = buffer.create_tag(Some("error"), &[]) {
            tag.set_foreground(Some("#f48771"));
        }

        let view = TextView::with_buffer(&buffer);
        view.set_editable(false);
        view.set_cursor_visible(false);
        view.set_monospace(true);
        view.set_wrap_mode(WrapMode::WordChar);
        view.set_top_margin(8);
        view.set_left_margin(12);
        view.add_css_class("code-view");

        let scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Automatic)
            .vscrollbar_policy(PolicyType::Automatic)
            .vexpand(true)
            .hexpand(true)
            .child(&view)
            .build();
        root.append(&scroll);

        // ── Bridge events (analysis lifecycle) ────────────────────────────────
        {
            let buf = buffer.clone();
            let view = view.clone();
            let fn_count = Rc::new(Cell::new(0usize));
            bridge.subscribe(move |event| {
                let (severity, msg) = match event {
                    BridgeEvent::AnalysisStarted(path) => {
                        fn_count.set(0);
                        ("info", format!("[info]  analysis started: {}", path.display()))
                    }
                    BridgeEvent::AnalysisFunctionReady(_, _) => {
                        fn_count.set(fn_count.get() + 1);
                        return;
                    }
                    BridgeEvent::AnalysisDone => (
                        "info",
                        format!("[info]  analysis done — {} function(s) lifted", fn_count.get()),
                    ),
                    BridgeEvent::AnalysisError(msg) => (
                        "error",
                        format!("[error] {msg}"),
                    ),
                };
                append_line(&buf, &view, severity, &msg);
            });
        }

        // ── Tracing log records ───────────────────────────────────────────────
        {
            let buf = buffer.clone();
            let view = view.clone();
            MainContext::default().spawn_local(async move {
                while let Ok(record) = log_rx.recv().await {
                    let (severity, prefix) = match record.level {
                        Level::ERROR => ("error", "ERROR"),
                        Level::WARN  => ("warn",  "WARN "),
                        Level::INFO  => ("info",  "INFO "),
                        Level::DEBUG => ("debug", "DEBUG"),
                        Level::TRACE => ("debug", "TRACE"),
                    };
                    let msg = format!(
                        "[{prefix}] {}: {}",
                        record.target, record.message
                    );
                    append_line(&buf, &view, severity, &msg);
                }
            });
        }

        Self { root }
    }

    pub fn widget(&self) -> &impl IsA<Widget> {
        &self.root
    }
}

fn append_line(buf: &TextBuffer, view: &TextView, severity: &str, text: &str) {
    let mut end = buf.end_iter();
    let start_offset = end.offset();
    buf.insert(&mut end, &format!("{text}\n"));
    let start = buf.iter_at_offset(start_offset);
    let end = buf.end_iter();
    buf.apply_tag_by_name(severity, &start, &end);
    view.scroll_to_iter(&mut buf.end_iter(), 0.0, false, 0.0, 0.0);
}
