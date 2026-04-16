//! Left panel: binary section tree + function list.
//!
//! Layout (top → bottom):
//!   • "Binary" header
//!   • Search box (filters the function list)
//!   • Scrollable content:
//!       – SECTIONS group: one clickable row per binary section
//!       – FUNCTIONS group: searchable list of all decompiled functions
//!
//! Clicking a section row emits `SectionSelected` via the bridge.
//! Clicking a function row emits `FunctionSelected` via the bridge.

use std::cell::Cell;
use std::rc::Rc;

use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, Button, Label, ListBox, Orientation,
    PolicyType, ScrolledWindow, SearchEntry, Separator, Widget,
};

use rustdec_loader::SectionKind;

use crate::bridge::{AnalysisBridge, BridgeEvent, SectionMeta};

// ── Panel ─────────────────────────────────────────────────────────────────────

pub struct ExplorerPanel {
    root:   GtkBox,
}

impl ExplorerPanel {
    pub fn new(bridge: AnalysisBridge) -> Self {
        let root = GtkBox::new(Orientation::Vertical, 0);
        root.set_width_request(260);

        // ── Header ────────────────────────────────────────────────────────────
        let header = Label::new(Some("Binary"));
        header.add_css_class("panel-header");
        root.append(&header);

        // ── Search box ────────────────────────────────────────────────────────
        let search = SearchEntry::new();
        search.set_placeholder_text(Some("Search functions…"));
        root.append(&search);

        // ── Outer scroll ──────────────────────────────────────────────────────
        let scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .vexpand(true)
            .build();

        let content = GtkBox::new(Orientation::Vertical, 0);
        scroll.set_child(Some(&content));
        root.append(&scroll);

        // ── SECTIONS group ────────────────────────────────────────────────────
        let sec_header = Label::new(Some("SECTIONS"));
        sec_header.add_css_class("explorer-group-header");
        sec_header.set_halign(gtk4::Align::Start);
        content.append(&sec_header);

        let sections_box = GtkBox::new(Orientation::Vertical, 0);
        sections_box.add_css_class("sections-box");
        content.append(&sections_box);

        // ── Separator ─────────────────────────────────────────────────────────
        let sep = Separator::new(Orientation::Horizontal);
        sep.add_css_class("explorer-separator");
        content.append(&sep);

        // ── FUNCTIONS group ───────────────────────────────────────────────────
        let fn_count: Rc<Cell<usize>> = Rc::new(Cell::new(0));

        // Header row: [FUNCTIONS (N)] ............... [full script]
        let fn_header_row = GtkBox::new(Orientation::Horizontal, 0);

        let fn_header = Label::new(Some("FUNCTIONS"));
        fn_header.add_css_class("explorer-group-header");
        fn_header.set_halign(gtk4::Align::Start);
        fn_header.set_hexpand(true);
        fn_header_row.append(&fn_header);

        let full_script_btn = Button::with_label("full script");
        full_script_btn.add_css_class("full-script-btn");
        full_script_btn.set_sensitive(false); // enabled once analysis completes
        fn_header_row.append(&full_script_btn);

        content.append(&fn_header_row);

        let func_list = ListBox::new();
        func_list.set_selection_mode(gtk4::SelectionMode::Single);
        func_list.add_css_class("function-list");
        content.append(&func_list);

        // ── Search filter on function list ────────────────────────────────────
        {
            let func_list_ref = func_list.clone();
            search.connect_search_changed(move |entry| {
                let query = entry.text().to_lowercase();
                func_list_ref.set_filter_func(move |row| {
                    row.child()
                        .and_then(|w| w.downcast::<Label>().ok())
                        .map(|l| l.text().to_lowercase().contains(&query))
                        .unwrap_or(true)
                });
            });
        }

        // ── "full script" button handler ──────────────────────────────────────
        {
            let bridge_ref = bridge.clone();
            full_script_btn.connect_clicked(move |_| {
                bridge_ref.select_all_functions();
            });
        }

        // ── Function row activation ───────────────────────────────────────────
        {
            let bridge_ref = bridge.clone();
            func_list.connect_row_activated(move |_, row| {
                if let Some(label) = row.child().and_then(|w| w.downcast::<Label>().ok()) {
                    bridge_ref.select_function(label.text().as_str());
                }
            });
        }

        // ── Bridge subscriptions ──────────────────────────────────────────────
        {
            let sections_box_ref    = sections_box.clone();
            let func_list_ref       = func_list.clone();
            let fn_header_ref       = fn_header.clone();
            let fn_count            = fn_count.clone();
            let full_script_btn_ref = full_script_btn.clone();
            let bridge_sub          = bridge.clone();

            bridge.subscribe(move |event| match event {
                // Clear everything when a new file starts loading.
                BridgeEvent::AnalysisStarted(_) => {
                    while let Some(child) = sections_box_ref.first_child() {
                        sections_box_ref.remove(&child);
                    }
                    while let Some(child) = func_list_ref.first_child() {
                        func_list_ref.remove(&child);
                    }
                    fn_count.set(0);
                    fn_header_ref.set_text("FUNCTIONS");
                    full_script_btn_ref.set_sensitive(false);
                }

                // Populate the sections group.
                BridgeEvent::SectionsLoaded(sections) => {
                    while let Some(child) = sections_box_ref.first_child() {
                        sections_box_ref.remove(&child);
                    }
                    for meta in sections {
                        let btn = make_section_row(&meta, bridge_sub.clone());
                        sections_box_ref.append(&btn);
                    }
                }

                // Append one function row per function as they stream in.
                BridgeEvent::AnalysisFunctionReady(name, _) => {
                    let label = Label::new(Some(&name));
                    label.set_halign(gtk4::Align::Start);
                    label.set_margin_start(8);
                    label.set_margin_end(8);
                    label.set_margin_top(3);
                    label.set_margin_bottom(3);
                    func_list_ref.append(&label);

                    let count = fn_count.get() + 1;
                    fn_count.set(count);
                    fn_header_ref.set_text(&format!("FUNCTIONS ({count})"));
                }

                // Enable "full script" once all functions are ready.
                BridgeEvent::AnalysisDone => {
                    full_script_btn_ref.set_sensitive(true);
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

// ── Section row widget ────────────────────────────────────────────────────────

/// Build a flat button row for one section.
fn make_section_row(meta: &SectionMeta, bridge: AnalysisBridge) -> Button {
    let name = meta.name.clone();

    // Row layout: [kind badge]  [name]  [size]
    let row_box = GtkBox::new(Orientation::Horizontal, 6);
    row_box.set_margin_start(8);
    row_box.set_margin_end(8);
    row_box.set_margin_top(4);
    row_box.set_margin_bottom(4);

    // Kind badge.
    let (badge_text, badge_class) = section_badge(meta.kind);
    let badge = Label::new(Some(badge_text));
    badge.add_css_class("section-badge");
    badge.add_css_class(badge_class);
    row_box.append(&badge);

    // Section name.
    let name_lbl = Label::new(Some(&meta.name));
    name_lbl.add_css_class("section-name");
    name_lbl.set_hexpand(true);
    name_lbl.set_halign(gtk4::Align::Start);
    row_box.append(&name_lbl);

    // Size.
    let size_lbl = Label::new(Some(&format_size(meta.size)));
    size_lbl.add_css_class("section-size");
    size_lbl.set_halign(gtk4::Align::End);
    row_box.append(&size_lbl);

    let btn = Button::new();
    btn.set_child(Some(&row_box));
    btn.add_css_class("section-row");

    btn.connect_clicked(move |_| {
        bridge.select_section(&name);
    });

    btn
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn section_badge(kind: SectionKind) -> (&'static str, &'static str) {
    match kind {
        SectionKind::Code         => ("code",   "badge-code"),
        SectionKind::ReadOnlyData => ("rodata", "badge-rodata"),
        SectionKind::Data         => ("data",   "badge-data"),
        SectionKind::Bss          => ("bss",    "badge-bss"),
        SectionKind::Debug        => ("debug",  "badge-debug"),
        SectionKind::Other        => ("other",  "badge-other"),
    }
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}
