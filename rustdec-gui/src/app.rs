//! Application activation — builds the main window and wires up the UI.

use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, HeaderBar,
    Orientation, Paned,
};
use tokio::runtime::Handle;

use crate::bridge::AnalysisBridge;
use crate::ui::{explorer::ExplorerPanel, graph::GraphPanel, code::CodePanel};

/// Called once by GTK when the application is ready.
pub fn activate(app: &Application, rt: Handle) {
    // ── CSS — dark theme overrides ────────────────────────────────────────────
    let css = gtk4::CssProvider::new();
    css.load_from_string(include_str!("style.css"));
    gtk4::style_context_add_provider_for_display(
        &gtk4::gdk::Display::default().expect("No display"),
        &css,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    // ── Async bridge ──────────────────────────────────────────────────────────
    let bridge = AnalysisBridge::new(rt);

    // ── Panels ────────────────────────────────────────────────────────────────
    let explorer = ExplorerPanel::new(bridge.clone());
    let code_view = CodePanel::new(bridge.clone());
    let graph_view = GraphPanel::new(bridge.clone());

    // ── Layout: horizontal Paned (explorer | vertical Paned (code | graph)) ──
    let right_pane = Paned::new(Orientation::Vertical);
    right_pane.set_start_child(Some(code_view.widget()));
    right_pane.set_end_child(Some(graph_view.widget()));
    right_pane.set_position(500);

    let main_pane = Paned::new(Orientation::Horizontal);
    main_pane.set_start_child(Some(explorer.widget()));
    main_pane.set_end_child(Some(&right_pane));
    main_pane.set_position(240);

    // ── Header bar ────────────────────────────────────────────────────────────
    let header = HeaderBar::new();
    let open_btn = gtk4::Button::with_label("Open…");
    {
        let bridge = bridge.clone();
        open_btn.connect_clicked(move |btn| {
            let parent = btn.root().and_then(|r| r.downcast::<ApplicationWindow>().ok());
            open_file_dialog(parent.as_ref(), bridge.clone());
        });
    }
    header.pack_start(&open_btn);

    let lang_btn = gtk4::DropDown::from_strings(&["C", "C++", "Rust"]);
    {
        let bridge = bridge.clone();
        lang_btn.connect_selected_item_notify(move |dd| {
            let lang = match dd.selected() {
                0 => rustdec_codegen::Language::C,
                1 => rustdec_codegen::Language::Cpp,
                _ => rustdec_codegen::Language::Rust,
            };
            bridge.set_language(lang);
        });
    }
    header.pack_end(&lang_btn);

    // ── Window ────────────────────────────────────────────────────────────────
    let window = ApplicationWindow::builder()
        .application(app)
        .title("Corpo Fractum")
        .default_width(1400)
        .default_height(900)
        .child(&main_pane)
        .build();
    window.set_titlebar(Some(&header));
    window.present();
}

// ── File open dialog ──────────────────────────────────────────────────────────

fn open_file_dialog(parent: Option<&ApplicationWindow>, bridge: AnalysisBridge) {
    let dialog = gtk4::FileDialog::new();
    dialog.open(parent, gtk4::gio::Cancellable::NONE, move |result| {
        if let Ok(file) = result {
            if let Some(path) = file.path() {
                bridge.load_file(path);
            }
        }
    });
}
