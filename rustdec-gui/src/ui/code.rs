//! Central code panel.
//!
//! Displays the pseudo-code output from the selected backend.
//! Uses a [`gtk4::TextView`] in read-only mode with monospaced font and
//! syntax-highlight placeholders via Pango tags.

use std::cell::Cell;
use std::rc::Rc;

use glib;
use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, Label, Orientation, PolicyType, ScrolledWindow,
    TextBuffer, TextView, Widget, WrapMode,
};

use crate::bridge::{AnalysisBridge, BridgeEvent};

// ── Welcome screen ────────────────────────────────────────────────────────────

const WELCOME: &str = "
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
";

// How many lines to highlight per idle tick.
// 250 lines ≈ 2–4 ms on a modern CPU, well under the 16 ms GTK frame budget.
const LINES_PER_TICK: i32 = 250;

const KEYWORDS: &[&str] = &[
    "void", "int", "uint64_t", "uint32_t", "uint8_t", "int64_t",
    "int32_t", "return", "if", "else", "while", "for", "goto",
    "struct", "float", "double", "break", "continue",
];

// ── Panel ─────────────────────────────────────────────────────────────────────

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

        // Disable the undo history — this is a read-only view that can receive
        // tens of thousands of lines. Storing every insertion wastes RAM and
        // slows down GTK's internal bookkeeping unnecessarily.
        buffer.begin_irreversible_action();
        buffer.set_text(WELCOME);
        buffer.end_irreversible_action();

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

        {
            let buf            = buffer.clone();
            let is_first       = Rc::new(Cell::new(true));
            // Guard against duplicate AnalysisDone events launching two
            // concurrent highlighting passes on the same buffer.
            let highlighting   = Rc::new(Cell::new(false));

            bridge.subscribe(move |event| match event {
                BridgeEvent::AnalysisStarted(_) => {
                    buf.begin_irreversible_action();
                    buf.set_text("// Analysing binary…");
                    buf.end_irreversible_action();
                    is_first.set(true);
                    highlighting.set(false);
                }

                BridgeEvent::AnalysisFunctionReady(_, code) => {
                    buf.begin_irreversible_action();
                    if is_first.get() {
                        buf.set_text(&code);
                        is_first.set(false);
                    } else {
                        // Two separate inserts avoid a temporary allocation
                        // that format!("\n\n{code}") would create.
                        let mut end = buf.end_iter();
                        buf.insert(&mut end, "\n\n");
                        let mut end = buf.end_iter();
                        buf.insert(&mut end, &code);
                    }
                    buf.end_irreversible_action();
                }

                BridgeEvent::AnalysisDone => {
                    // Only start one highlighting pass at a time.
                    if !highlighting.get() {
                        highlighting.set(true);
                        let highlighting = highlighting.clone();
                        schedule_highlighting(buf.clone(), move || {
                            highlighting.set(false);
                        });
                    }
                }

                BridgeEvent::AnalysisError(msg) => {
                    buf.begin_irreversible_action();
                    buf.set_text(&format!("// Error: {msg}"));
                    buf.end_irreversible_action();
                    // Reset so the next successful analysis starts cleanly.
                    is_first.set(true);
                    highlighting.set(false);
                }
            });
        }

        Self { root, buffer }
    }

    pub fn widget(&self) -> &impl IsA<Widget> {
        &self.root
    }
}

// ── Incremental syntax highlighting ──────────────────────────────────────────

/// Start an incremental highlighting pass on `buf`.
///
/// `on_done` is called once every line has been processed.
/// The pass is split into ticks of `LINES_PER_TICK` lines each, yielding to
/// the GTK main loop between ticks so the UI stays responsive.
///
/// Old tags are cleared before the first tick so that a second analysis never
/// accumulates stale highlights from the previous run.
fn schedule_highlighting(buf: TextBuffer, on_done: impl Fn() + 'static) {
    let on_done   = Rc::new(on_done);
    let next_line = Rc::new(Cell::new(0i32));

    // Clear all existing tags before starting — prevents stale highlights
    // from a previous analysis bleeding into the new one.
    let start = buf.start_iter();
    let end   = buf.end_iter();
    buf.remove_all_tags(&start, &end);

    fn tick(
        buf:       TextBuffer,
        next_line: Rc<Cell<i32>>,
        on_done:   Rc<dyn Fn()>,
    ) {
        let total = buf.line_count();
        let from  = next_line.get();
        if from >= total {
            on_done();
            return;
        }

        let to = (from + LINES_PER_TICK).min(total);
        highlight_range(&buf, from, to);
        next_line.set(to);

        if to < total {
            let buf2      = buf.clone();
            let next_line = next_line.clone();
            let on_done   = on_done.clone();
            glib::idle_add_local_once(move || tick(buf2, next_line, on_done));
        } else {
            on_done();
        }
    }

    glib::idle_add_local_once(move || tick(buf, next_line, on_done));
}

/// Highlight lines `[from, to)` using a single sequential scan per line.
///
/// Rather than running 18 separate `find` passes (one per keyword), we scan
/// the line once and check every byte position against the keyword list —
/// O(line_len × avg_kw_len) instead of O(line_len × kw_count).
fn highlight_range(buf: &TextBuffer, from: i32, to: i32) {
    for line_no in from..to {
        let line_start = buf.iter_at_line(line_no)
            .unwrap_or_else(|| buf.end_iter());
        let mut line_end = line_start.clone();
        line_end.forward_to_line_end();

        // buf.text returns a GString — avoid a second allocation by working
        // on the GString directly rather than calling .to_string().
        let line_gstr = buf.text(&line_start, &line_end, false);
        let line_text: &str = line_gstr.as_str();

        // Comment: colour from `//` to end of line, skip keyword scan.
        if let Some(col) = line_text.find("//") {
            if let Some(cs) = buf.iter_at_line_offset(line_no, col as i32) {
                buf.apply_tag_by_name("comment", &cs, &line_end);
            }
            continue;
        }

        // Single-pass keyword scan: try each position once.
        let bytes = line_text.as_bytes();
        let len   = bytes.len();
        let mut pos = 0usize;

        while pos < len {
            // Quick pre-filter: skip positions that can't start a word boundary.
            let prev_alpha = pos > 0 && bytes[pos - 1].is_ascii_alphanumeric();
            if prev_alpha {
                pos += 1;
                continue;
            }

            // Try every keyword at this position.
            let mut matched = false;
            for kw in KEYWORDS {
                let kw_len = kw.len();
                if pos + kw_len > len { continue; }
                if &line_text[pos..pos + kw_len] != *kw { continue; }
                // Check trailing boundary.
                let after_ok = pos + kw_len >= len
                    || !bytes[pos + kw_len].is_ascii_alphanumeric();
                if after_ok {
                    if let (Some(ks), Some(ke)) = (
                        buf.iter_at_line_offset(line_no, pos as i32),
                        buf.iter_at_line_offset(line_no, (pos + kw_len) as i32),
                    ) {
                        buf.apply_tag_by_name("keyword", &ks, &ke);
                    }
                    pos += kw_len;
                    matched = true;
                    break;
                }
            }

            if !matched { pos += 1; }
        }
    }
}
