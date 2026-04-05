//! # rustdec-gui
//!
//! Entry point for the RustDec desktop application.
//! Initialises the GTK4 application, sets up Tokio for async backend work,
//! and launches the main window.

mod app;
mod bridge;
mod ui;

use gtk4::prelude::*;
use gtk4::Application;
use tracing_subscriber::EnvFilter;

const APP_ID: &str = "io.rustdec.RustDec";

fn main() -> anyhow::Result<()> {
    // Logging — RUSTDEC_LOG=debug rustdec
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_env("RUSTDEC_LOG"))
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
        // Pass the runtime handle into the UI so it can spawn analysis tasks.
        let handle = rt.handle().clone();
        app::activate(gtk_app, handle);
    });

    let exit_code = app.run();
    std::process::exit(exit_code.into());
}
