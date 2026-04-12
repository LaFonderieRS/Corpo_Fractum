//! Application activation — builds the main window and wires up the UI.

#[cfg(all(feature = "console-bottom", feature = "console-tab"))]
compile_error!(
    "features `console-bottom` and `console-tab` are mutually exclusive — pick one"
);

use glib;
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, HeaderBar,
    Orientation, Paned,
};
#[cfg(feature = "console-tab")]
use gtk4::{Label, Notebook};

#[cfg(any(feature = "console-bottom", feature = "console-tab"))]
use crate::ui::console::ConsolePanel;
use tokio::runtime::Handle;

use crate::bridge::{AnalysisBridge, BridgeEvent};
use crate::log_layer::LogRecord;
use crate::splash::SplashScreen;
use crate::ui::{explorer::ExplorerPanel, graph::GraphPanel, code::CodePanel};

/// Called once by GTK when the application is ready.
pub fn activate(app: &Application, rt: Handle, log_rx: async_channel::Receiver<LogRecord>) {
    // ── CSS — dark theme overrides ────────────────────────────────────────────
    let css = gtk4::CssProvider::new();
    css.load_from_string(include_str!("style.css"));
    gtk4::style_context_add_provider_for_display(
        &gtk4::gdk::Display::default().expect("No display"),
        &css,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    // ── Splash screen ─────────────────────────────────────────────────────────
    let splash = SplashScreen::show();
    splash.set_status("Initialising…");

    // ── Async bridge ──────────────────────────────────────────────────────────
    let bridge = AnalysisBridge::new(rt);

    // ── Splash ↔ bridge wiring ─────────────────────────────────────────────────
    {
        let splash = splash.clone();
        bridge.subscribe(move |event| match event {
            BridgeEvent::AnalysisStarted(_) => {
                splash.set_status("Loading binary…");
                splash.set_progress(0.1);
            }
            BridgeEvent::AnalysisDone | BridgeEvent::AnalysisError(_) => {
                splash.dismiss();
            }
            _ => {}
        });
    }

    // ── Panels ────────────────────────────────────────────────────────────────
    let explorer = ExplorerPanel::new(bridge.clone());
    let code_view = CodePanel::new(bridge.clone());
    let graph_view = GraphPanel::new(bridge.clone());

    // ── Layout: horizontal Paned (explorer | vertical Paned (code | graph)) ──
    let right_pane = Paned::new(Orientation::Vertical);
    right_pane.set_start_child(Some(code_view.widget()));

    // Option B: graph slot becomes a GtkNotebook with Graph + Console tabs.
    #[cfg(feature = "console-tab")]
    {
        let console = ConsolePanel::new(bridge.clone(), log_rx.clone());
        let notebook = Notebook::new();
        notebook.append_page(graph_view.widget(), Some(&Label::new(Some("Graph"))));
        notebook.append_page(console.widget(), Some(&Label::new(Some("Console"))));
        right_pane.set_end_child(Some(&notebook));
    }
    #[cfg(not(feature = "console-tab"))]
    right_pane.set_end_child(Some(graph_view.widget()));

    right_pane.set_position(500);

    let main_pane = Paned::new(Orientation::Horizontal);
    main_pane.set_start_child(Some(explorer.widget()));

    // Option A: console goes below the code+graph pane in an outer vertical split.
    #[cfg(feature = "console-bottom")]
    {
        let console = ConsolePanel::new(bridge.clone(), log_rx);
        let outer_right = Paned::new(Orientation::Vertical);
        outer_right.set_start_child(Some(&right_pane));
        outer_right.set_end_child(Some(console.widget()));
        outer_right.set_position(600);
        main_pane.set_end_child(Some(&outer_right));
    }
    #[cfg(not(feature = "console-bottom"))]
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

    let about_btn = gtk4::Button::with_label("About");
    about_btn.connect_clicked(move |btn| {
        let parent = btn.root().and_then(|r| r.downcast::<ApplicationWindow>().ok());
        show_about_dialog(parent.as_ref());
    });
    header.pack_end(&about_btn);

    // ── Window ────────────────────────────────────────────────────────────────
    let window = ApplicationWindow::builder()
        .application(app)
        .title("Corpo Fractum")
        .default_width(1400)
        .default_height(900)
        .child(&main_pane)
        .build();
    window.set_titlebar(Some(&header));

    // Present the main window only after the splash closes.
    {
        let window = window.clone();
        splash.connect_dismissed(move || window.present());
    }

    // Dismiss the splash after a short delay so the user sees it at startup.
    glib::idle_add_local_once(move || {
        glib::timeout_add_local_once(
            std::time::Duration::from_millis(1_800),
            move || splash.dismiss(),
        );
    });
}

// ── About dialog ─────────────────────────────────────────────────────────────

fn show_about_dialog(parent: Option<&ApplicationWindow>) {
    let dialog = gtk4::AboutDialog::new();
    dialog.set_program_name(Some("Corpo Fractum"));
    dialog.set_version(Some(env!("CARGO_PKG_VERSION")));
    dialog.set_comments(Some(
        "From gods came man. From binary, came code.\n\n\
         Open-source binary decompiler targeting x86-64, ELF, PE and Mach-O. \
         Lifts machine code to a typed SSA intermediate representation and emits \
         readable C, C++ or Rust pseudo-code. The entire toolchain — loader, \
         disassembler, IR, analysis, codegen and UI — is written in pure Rust.",
    ));
    dialog.set_license_type(gtk4::License::Gpl30);
    dialog.set_authors(&[
        "Corpo Fractum Contributors - La Fonderie",
    ]);
    dialog.set_system_information(Some(
        "rustdec-loader   — ELF / PE / Mach-O parser        (goblin)\n\
         rustdec-disasm   — multi-arch disassembler          (capstone-rs)\n\
         rustdec-ir       — SSA intermediate representation\n\
         rustdec-lift     — x86-64 instruction lifter\n\
         rustdec-analysis — CFG · dominance · structuration\n\
         rustdec-codegen  — C / C++ / Rust code generators\n\
         rustdec-gui      — GTK4 application                 (gtk4-rs · Cairo · Tokio)",
    ));
    dialog.set_transient_for(parent);
    dialog.set_modal(true);
    dialog.present();
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
