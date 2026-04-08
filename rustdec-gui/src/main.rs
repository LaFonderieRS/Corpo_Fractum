//! # rustdec-gui
//!
//! Entry point for the RustDec desktop application.
//! Initialises the GTK4 application, sets up Tokio for async backend work,
//! and launches the main window.

mod app;
mod bridge;
mod log_layer;
mod splash;
mod ui;

use gtk4::prelude::*;
use gtk4::Application;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

const APP_ID: &str = "io.rustdec.RustDec";

fn main() -> anyhow::Result<()> {
    // Logging — RUSTDEC_LOG=debug rustdec
    //
    // Two layers:
    //   1. fmt  — human-readable output on stdout (existing behaviour)
    //   2. gtk  — forwards every record to the in-app Console panel
    let (gtk_layer, log_rx) = log_layer::GtkLogLayer::new();

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(EnvFilter::from_env("RUSTDEC_LOG")))
        .with(gtk_layer)
        .init();

    // Tokio runtime for async analysis tasks.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()?;

    // GTK application.
    let app = Application::builder()
        .application_id(APP_ID)
        .build();

    app.connect_activate(move |gtk_app| {
        // Pass the runtime handle and log receiver into the UI.
        let handle = rt.handle().clone();
        app::activate(gtk_app, handle, log_rx.clone());
    });

    let exit_code = app.run();
    std::process::exit(exit_code.into());
}
