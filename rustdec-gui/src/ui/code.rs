//! Central code panel.
//!
//! Displays either:
//!  • Decompiled pseudo-code for a selected function (with syntax highlighting)
//!  • A hex-dump / string-extraction view for a selected binary section
//!
//! Content is driven by `FunctionSelected` and `SectionSelected` bridge events.

use std::cell::Cell;
use std::fmt::Write as FmtWrite;
use std::rc::Rc;

use glib;
use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, Label, Orientation, PolicyType, ScrolledWindow,
    TextBuffer, TextView, Widget, WrapMode,
};

use rustdec_loader::SectionKind;

use crate::bridge::{AnalysisBridge, BridgeEvent, SectionMeta};

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

const ANALYSIS_DONE_MSG: &str = "// Analysis complete.\n//\n// Select a function or section from the left panel.";

// How many lines to highlight per idle tick.
const LINES_PER_TICK: i32 = 250;

const KEYWORDS: &[&str] = &[
    "void", "int", "uint64_t", "uint32_t", "uint8_t", "int64_t",
    "int32_t", "return", "if", "else", "while", "for", "goto",
    "struct", "float", "double", "break", "continue",
];

// ── Panel ─────────────────────────────────────────────────────────────────────

pub struct CodePanel {
    root: GtkBox,
}

impl CodePanel {
    pub fn new(bridge: AnalysisBridge) -> Self {
        let root = GtkBox::new(Orientation::Vertical, 0);

        let header = Label::new(Some("Decompiled code"));
        header.add_css_class("panel-header");
        root.append(&header);

        let buffer = TextBuffer::new(None);
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
        if let Some(tag) = buffer.create_tag(Some("hex-addr"), &[]) {
            tag.set_foreground(Some("#9cdcfe"));
        }
        if let Some(tag) = buffer.create_tag(Some("hex-ascii"), &[]) {
            tag.set_foreground(Some("#ce9178"));
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
            let buf          = buffer.clone();
            let highlighting = Rc::new(Cell::new(false));

            bridge.subscribe(move |event| match event {
                // New binary loading — reset to placeholder.
                BridgeEvent::AnalysisStarted(_) => {
                    highlighting.set(false);
                    buf.begin_irreversible_action();
                    buf.set_text("// Analysing binary…");
                    buf.end_irreversible_action();
                }

                // Analysis done — prompt user to select something if we haven't
                // replaced the placeholder text with real content yet.
                BridgeEvent::AnalysisDone => {
                    let current = buf.text(&buf.start_iter(), &buf.end_iter(), false);
                    if current.starts_with("// Analysing") {
                        buf.begin_irreversible_action();
                        buf.set_text(ANALYSIS_DONE_MSG);
                        buf.end_irreversible_action();
                    }
                }

                // User selected a function — show its decompiled code.
                BridgeEvent::FunctionSelected(_, code) => {
                    highlighting.set(false);
                    buf.begin_irreversible_action();
                    buf.set_text(&code);
                    buf.end_irreversible_action();

                    let hl = highlighting.clone();
                    hl.set(true);
                    schedule_highlighting(buf.clone(), move || { hl.set(false); });
                }

                // User selected a binary section — show its content.
                BridgeEvent::SectionSelected(meta) => {
                    highlighting.set(false);
                    let text = format_section_content(&meta);
                    buf.begin_irreversible_action();
                    buf.set_text(&text);
                    buf.end_irreversible_action();
                    // Only highlight if it's a code section (decompiled view).
                    // Hex-dump views don't need keyword highlighting.
                    if meta.kind == SectionKind::Code {
                        let hl = highlighting.clone();
                        hl.set(true);
                        schedule_highlighting(buf.clone(), move || { hl.set(false); });
                    }
                }

                BridgeEvent::AnalysisError(msg) => {
                    highlighting.set(false);
                    buf.begin_irreversible_action();
                    buf.set_text(&format!("// Error: {msg}"));
                    buf.end_irreversible_action();
                }

                _ => {}
            });
        }

        Self { root }
    }

    pub fn widget(&self) -> &impl IsA<Widget> {
        &self.root
    }
}

// ── Section content formatting ────────────────────────────────────────────────

fn format_section_content(meta: &SectionMeta) -> String {
    let mut out = String::new();

    let kind_str = match meta.kind {
        SectionKind::Code         => "executable code",
        SectionKind::ReadOnlyData => "read-only data",
        SectionKind::Data         => "initialized data",
        SectionKind::Bss          => "uninitialized data (BSS)",
        SectionKind::Debug        => "debug information",
        SectionKind::Other        => "other",
    };

    let _ = writeln!(out, "// Section:  {}", meta.name);
    let _ = writeln!(out, "// Type:     {}", kind_str);
    let _ = writeln!(out, "// Address:  {:#010x}", meta.virtual_addr);
    let _ = writeln!(out, "// Size:     {} bytes", meta.size);

    match meta.kind {
        SectionKind::Code => {
            let _ = writeln!(out);
            let _ = writeln!(out, "// This is a code section.");
            let _ = writeln!(out, "// Select a function from the left panel to view decompiled code.");
        }

        SectionKind::Bss => {
            let _ = writeln!(out);
            let _ = writeln!(out, "// BSS section — zero-initialised at runtime, no raw bytes.");
        }

        _ => {
            // Extract printable strings.
            let strings = extract_printable_strings(&meta.data);
            if !strings.is_empty() {
                let _ = writeln!(out);
                let _ = writeln!(out, "// ── Strings ─────────────────────────────────────────────────────────────");
                for (offset, s) in strings.iter().take(200) {
                    let addr = meta.virtual_addr.saturating_add(*offset as u64);
                    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
                    let _ = writeln!(out, "//   {:#010x}  \"{escaped}\"", addr);
                }
                if strings.len() > 200 {
                    let _ = writeln!(out, "//   … ({} more)", strings.len() - 200);
                }
            }

            // Hex dump (capped at 4 096 bytes to stay responsive).
            let _ = writeln!(out);
            let _ = writeln!(out, "// ── Hex dump ────────────────────────────────────────────────────────────");
            let cap = meta.data.len().min(4096);
            for (chunk_idx, chunk) in meta.data[..cap].chunks(16).enumerate() {
                let addr = meta.virtual_addr.saturating_add((chunk_idx * 16) as u64);
                let hex: String = chunk
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ");
                let ascii: String = chunk
                    .iter()
                    .map(|&b| if b >= 0x20 && b < 0x7f { b as char } else { '.' })
                    .collect();
                let _ = writeln!(out, "{addr:#010x}  {hex:<48}  |{ascii}|");
            }
            if meta.data.len() > 4096 {
                let _ = writeln!(out, "// … ({} more bytes not shown)", meta.data.len() - 4096);
            }
        }
    }

    out
}

/// Extract printable ASCII strings of length ≥ 4 from a byte slice.
fn extract_printable_strings(data: &[u8]) -> Vec<(usize, String)> {
    let mut result  = Vec::new();
    let mut current = String::new();
    let mut start   = 0usize;

    for (i, &b) in data.iter().enumerate() {
        if b >= 0x20 && b < 0x7f {
            if current.is_empty() { start = i; }
            current.push(b as char);
        } else {
            if current.len() >= 4 {
                result.push((start, std::mem::take(&mut current)));
            } else {
                current.clear();
            }
        }
    }
    if current.len() >= 4 {
        result.push((start, current));
    }
    result
}

// ── Incremental syntax highlighting ──────────────────────────────────────────

fn schedule_highlighting(buf: TextBuffer, on_done: impl Fn() + 'static) {
    let on_done   = Rc::new(on_done);
    let next_line = Rc::new(Cell::new(0i32));

    let start = buf.start_iter();
    let end   = buf.end_iter();
    buf.remove_all_tags(&start, &end);

    fn tick(buf: TextBuffer, next_line: Rc<Cell<i32>>, on_done: Rc<dyn Fn()>) {
        let total = buf.line_count();
        let from  = next_line.get();
        if from >= total { on_done(); return; }

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

fn highlight_range(buf: &TextBuffer, from: i32, to: i32) {
    for line_no in from..to {
        let line_start = buf.iter_at_line(line_no)
            .unwrap_or_else(|| buf.end_iter());
        let mut line_end = line_start.clone();
        line_end.forward_to_line_end();

        let line_gstr = buf.text(&line_start, &line_end, false);
        let line_text: &str = line_gstr.as_str();

        if let Some(col) = line_text.find("//") {
            if let Some(cs) = buf.iter_at_line_offset(line_no, col as i32) {
                buf.apply_tag_by_name("comment", &cs, &line_end);
            }
            continue;
        }

        let bytes = line_text.as_bytes();
        let len   = bytes.len();
        let mut pos = 0usize;

        while pos < len {
            let prev_alpha = pos > 0 && bytes[pos - 1].is_ascii_alphanumeric();
            if prev_alpha { pos += 1; continue; }

            let mut matched = false;
            for kw in KEYWORDS {
                let kw_len = kw.len();
                if pos + kw_len > len { continue; }
                if &line_text[pos..pos + kw_len] != *kw { continue; }
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
