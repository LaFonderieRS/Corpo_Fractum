//! Central code panel.
//!
//! Displays the pseudo-code output from the selected backend.
//! Uses a [`gtk4::TextView`] in read-only mode with monospaced font and
//! syntax-highlight placeholders via Pango tags.

use std::cell::Cell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, Label, Orientation, PolicyType, ScrolledWindow,
    TextBuffer, TextView, Widget, WrapMode,
};

use crate::bridge::{AnalysisBridge, BridgeEvent};

pub struct CodePanel {
    root:   GtkBox,
    buffer: TextBuffer,
}

impl CodePanel {
    pub fn new(bridge: AnalysisBridge) -> Self {
        let root = GtkBox::new(Orientation::Vertical, 0);

        let header = Label::new(Some("Decompiled code"));
        header.add_css_class("panel-header");
        root.append(&header);

        let buffer = TextBuffer::new(None);
        buffer.set_text("
 ██████╗ ██████╗ ██████╗ ██████╗  ██████╗     ███████╗██████╗  █████╗  ██████╗████████╗██╗   ██╗███╗   ███╗
██╔════╝██╔═══██╗██╔══██╗██╔══██╗██╔═══██╗    ██╔════╝██╔══██╗██╔══██╗██╔════╝╚══██╔══╝██║   ██║████╗ ████║
██║     ██║   ██║██████╔╝██████╔╝██║   ██║    █████╗  ██████╔╝███████║██║        ██║   ██║   ██║██╔████╔██║
██║     ██║   ██║██╔══██╗██╔═══╝ ██║   ██║    ██╔══╝  ██╔══██╗██╔══██║██║        ██║   ██║   ██║██║╚██╔╝██║
╚██████╗╚██████╔╝██║  ██║██║     ╚██████╔╝    ██║     ██║  ██║██║  ██║╚██████╗   ██║   ╚██████╔╝██║ ╚═╝ ██║
 ╚═════╝ ╚═════╝ ╚═╝  ╚═╝╚═╝      ╚═════╝     ╚═╝     ╚═╝  ╚═╝╚═╝  ╚═╝ ╚═════╝   ╚═╝    ╚═════╝ ╚═╝     ╚═╝

                             ░▒▓ CORPO FRACTUM ▓▒░

                  From gods came man. From binary, came code.

        [ dissecting structure ]  [ lifting instructions ]  [ rebuilding meaning ]

-----------------------------------------------------------------------------------------------------------

> ready.
        ");

        // create_tag returns Option<TextTag> — unwrap is safe: names are unique.
        if let Some(tag) = buffer.create_tag(Some("comment"), &[]) {
            tag.set_foreground(Some("#6a9955"));
        }
        if let Some(tag) = buffer.create_tag(Some("keyword"), &[]) {
            tag.set_foreground(Some("#569cd6"));
            tag.set_weight(700);
        }

        let view = TextView::with_buffer(&buffer);
        view.set_editable(false);
        view.set_cursor_visible(false);
        view.set_monospace(true);
        view.set_wrap_mode(WrapMode::None);
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

        // Subscribe: stream pseudo-code into the buffer one function at a time.
        // AnalysisFunctionReady events arrive sequentially on the GTK main thread,
        // so TextBuffer (non-Send) is fully safe to mutate here.
        {
            let buf = buffer.clone();
            let is_first = Rc::new(Cell::new(true));
            bridge.subscribe(move |event| match event {
                BridgeEvent::AnalysisStarted(_) => {
                    buf.set_text("// Analysing binary…");
                    is_first.set(true);
                }
                BridgeEvent::AnalysisFunctionReady(_, code) => {
                    if is_first.get() {
                        buf.set_text(&code);
                        is_first.set(false);
                    } else {
                        buf.insert(&mut buf.end_iter(), &format!("\n\n{code}"));
                    }
                }
                BridgeEvent::AnalysisDone => {
                    apply_basic_highlighting(&buf);
                }
                BridgeEvent::AnalysisError(msg) => {
                    buf.set_text(&format!("// Error: {msg}"));
                }
            });
        }

        Self { root, buffer }
    }

    pub fn widget(&self) -> &impl IsA<Widget> {
        &self.root
    }
}

/// Very simple keyword highlighting using TextTags.
/// A real implementation would use a proper lexer.
fn apply_basic_highlighting(buf: &TextBuffer) {
    let text = buf
        .text(&buf.start_iter(), &buf.end_iter(), false)
        .to_string();

    let c_keywords = [
        "void", "int", "uint64_t", "uint32_t", "uint8_t", "int64_t",
        "int32_t", "return", "if", "else", "goto", "struct", "float", "double",
    ];

    for kw in c_keywords {
        let mut start = 0;
        while let Some(pos) = text[start..].find(kw) {
            let abs = start + pos;
            let before_ok = abs == 0 || !text.as_bytes()[abs - 1].is_ascii_alphanumeric();
            let after_ok  = abs + kw.len() >= text.len()
                || !text.as_bytes()[abs + kw.len()].is_ascii_alphanumeric();
            if before_ok && after_ok {
                let mut s = buf.start_iter();
                let mut e = buf.start_iter();
                s.set_offset(abs as i32);
                e.set_offset((abs + kw.len()) as i32);
                buf.apply_tag_by_name("keyword", &s, &e);
            }
            start = abs + 1;
        }
    }

    // Highlight comments (// …).
    for (line_no, line) in text.lines().enumerate() {
        if let Some(col) = line.find("//") {
            let line_start: usize = text.lines().take(line_no).map(|l| l.len() + 1).sum();
            let abs = line_start + col;
            let abs_end = line_start + line.len();
            let mut s = buf.start_iter();
            let mut e = buf.start_iter();
            s.set_offset(abs as i32);
            e.set_offset(abs_end as i32);
            buf.apply_tag_by_name("comment", &s, &e);
        }
    }
}
