//! Left panel: function / symbol explorer.
//!
//! Shows a searchable list of all functions found in the binary.
//! Clicking a row emits a `FunctionSelected` signal via the bridge.

use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, Label, ListBox, Orientation,
    PolicyType, ScrolledWindow, SearchEntry, Widget,
};

use crate::bridge::{AnalysisBridge, BridgeEvent};

pub struct ExplorerPanel {
    root:    GtkBox,
    list:    ListBox,
    search:  SearchEntry,
    bridge:  AnalysisBridge,
}

impl ExplorerPanel {
    pub fn new(bridge: AnalysisBridge) -> Self {
        let root = GtkBox::new(Orientation::Vertical, 0);
        root.set_width_request(240);

        // Header label.
        let header = Label::new(Some("Functions"));
        header.add_css_class("panel-header");
        root.append(&header);

        // Search box.
        let search = SearchEntry::new();
        search.set_placeholder_text(Some("Search…"));
        root.append(&search);

        // Scrollable list.
        let list = ListBox::new();
        list.set_selection_mode(gtk4::SelectionMode::Single);
        list.add_css_class("function-list");

        let scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .vexpand(true)
            .child(&list)
            .build();
        root.append(&scroll);

        // Wire search filter.
        {
            let list_ref = list.clone();
            search.connect_search_changed(move |entry| {
                let query = entry.text().to_lowercase();
                list_ref.invalidate_filter();
                // Re-filter: show rows whose label contains the query.
                list_ref.set_filter_func(move |row| {
                    row.child()
                        .and_then(|w| w.downcast::<Label>().ok())
                        .map(|l| l.text().to_lowercase().contains(&query))
                        .unwrap_or(true)
                });
            });
        }

        // Subscribe to bridge events to populate the list.
        {
            let list_ref = list.clone();
            bridge.subscribe(move |event| {
                if let BridgeEvent::AnalysisDone(funcs) = event {
                    // Clear existing rows.
                    while let Some(child) = list_ref.first_child() {
                        list_ref.remove(&child);
                    }
                    for (name, _code) in &funcs {
                        let label = Label::new(Some(name));
                        label.set_halign(gtk4::Align::Start);
                        label.set_margin_start(8);
                        label.set_margin_end(8);
                        label.set_margin_top(4);
                        label.set_margin_bottom(4);
                        list_ref.append(&label);
                    }
                }
            });
        }

        Self { root, list, search, bridge }
    }

    /// Return the GTK widget to embed in a parent container.
    pub fn widget(&self) -> &impl IsA<Widget> {
        &self.root
    }
}
