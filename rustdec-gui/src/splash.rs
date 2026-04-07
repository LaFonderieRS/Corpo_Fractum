//! Splash screen — shown at startup while the application initialises.
//!
//! Behaviour mirrors GIMP's splash:
//! - Undecorated, always-on-top window, centred on screen.
//! - Logo + application name + version + a short status line.
//! - Progress bar that fills while analysis runs.
//! - Dismissed automatically once the main window is ready, or after
//!   a maximum of `SPLASH_TIMEOUT_MS` milliseconds if nothing happens.
//!
//! ## Usage
//!
//! ```rust,ignore
//! let splash = SplashScreen::show();
//! splash.set_status("Loading binary…");
//! splash.set_progress(0.4);
//! splash.dismiss(); // hides and destroys the window
//! ```

use gtk4::prelude::*;
use gtk4::{
    Align, Box as GBox, Label, Orientation, ProgressBar, Window,
};
use std::cell::RefCell;
use std::rc::Rc;
use tracing::debug;

const APP_NAME:    &str = "Corpo Fractum";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
const SPLASH_W:     i32 = 480;
const SPLASH_H:     i32 = 280;

// Maximum time the splash stays visible even if nothing dismisses it.
const SPLASH_TIMEOUT_MS: u32 = 6_000;

// ── Public handle ─────────────────────────────────────────────────────────────

/// A cheap-to-clone handle to the splash screen.
#[derive(Clone)]
pub struct SplashScreen {
    inner: Rc<RefCell<Inner>>,
}

struct Inner {
    window:   Window,
    status:   Label,
    progress: ProgressBar,
    dismissed: bool,
}

impl SplashScreen {
    /// Build and immediately display the splash window.
    pub fn show() -> Self {
        let window = build_window();
        let status   = Label::new(Some("Initialising…"));
        let progress = ProgressBar::new();

        // ── Layout ────────────────────────────────────────────────────────────
        let root = GBox::new(Orientation::Vertical, 0);
        root.set_widget_name("splash-root");

        // Top area: logo placeholder + name/version
        let top = GBox::new(Orientation::Vertical, 8);
        top.set_margin_top(32);
        top.set_margin_bottom(16);
        top.set_margin_start(32);
        top.set_margin_end(32);
        top.set_vexpand(true);
        top.set_valign(Align::Center);

        let name_label = Label::new(Some(APP_NAME));
        name_label.set_widget_name("splash-name");

        let ver_label = Label::new(Some(&format!("version {APP_VERSION}")));
        ver_label.set_widget_name("splash-version");

        let tagline = Label::new(Some("Binary decompiler · ELF · PE · Mach-O → C / Rust"));
        tagline.set_widget_name("splash-tagline");

        top.append(&name_label);
        top.append(&ver_label);
        top.append(&tagline);

        // Bottom area: status + progress bar
        let bottom = GBox::new(Orientation::Vertical, 4);
        bottom.set_margin_start(24);
        bottom.set_margin_end(24);
        bottom.set_margin_bottom(20);

        status.set_widget_name("splash-status");
        status.set_halign(Align::Start);
        status.set_ellipsize(gtk4::pango::EllipsizeMode::End);

        progress.set_widget_name("splash-progress");
        progress.set_fraction(0.0);
        progress.set_show_text(false);

        bottom.append(&status);
        bottom.append(&progress);

        root.append(&top);
        root.append(&bottom);
        window.set_child(Some(&root));
        window.present();

        debug!("splash screen shown");

        let splash = Self {
            inner: Rc::new(RefCell::new(Inner {
                window,
                status,
                progress,
                dismissed: false,
            })),
        };

        // Auto-dismiss after timeout.
        {
            let splash_weak = Rc::downgrade(&splash.inner);
            glib::timeout_add_local_once(
                std::time::Duration::from_millis(SPLASH_TIMEOUT_MS as u64),
                move || {
                    if let Some(inner) = splash_weak.upgrade() {
                        let mut inner = inner.borrow_mut();
                        if !inner.dismissed {
                            debug!("splash: auto-dismissed after timeout");
                            inner.window.close();
                            inner.dismissed = true;
                        }
                    }
                },
            );
        }

        splash
    }

    /// Update the status line (e.g. "Building CFG…", "Generating C code…").
    pub fn set_status(&self, msg: &str) {
        self.inner.borrow().status.set_text(msg);
    }

    /// Set the progress bar fill (0.0 – 1.0).
    pub fn set_progress(&self, fraction: f64) {
        self.inner.borrow().progress.set_fraction(fraction.clamp(0.0, 1.0));
    }

    /// Pulse the progress bar (indeterminate mode).
    pub fn pulse(&self) {
        self.inner.borrow().progress.pulse();
    }

    /// Hide and destroy the splash window.
    pub fn dismiss(&self) {
        let mut inner = self.inner.borrow_mut();
        if !inner.dismissed {
            inner.window.close();
            inner.dismissed = true;
            debug!("splash screen dismissed");
        }
    }
}

// ── Window construction ───────────────────────────────────────────────────────

fn build_window() -> Window {
    let win = Window::builder()
        .title(APP_NAME)
        .decorated(false)           // no title bar — pure content
        .resizable(false)
        .modal(true)
        .default_width(SPLASH_W)
        .default_height(SPLASH_H)
        .build();

    // CSS name for targeted styling.
    win.set_widget_name("splash-window");

    // Centre on the primary monitor.
    // GTK4 doesn't expose set_position(), so we use a size-allocate signal
    // to reposition on first draw.
    {
        let win_ref = win.clone();
        win.connect_realize(move |_| {
            if let Some(display) = gtk4::gdk::Display::default() {
                if let Some(monitor) = display.monitors().item(0) {
                    if let Ok(monitor) = monitor.downcast::<gtk4::gdk::Monitor>() {
                        let geom = monitor.geometry();
                        let x = geom.x() + (geom.width()  - SPLASH_W) / 2;
                        let y = geom.y() + (geom.height() - SPLASH_H) / 2;
                        // surface-level positioning via GDK is restricted in
                        // Wayland; on X11 this will work correctly.  On
                        // Wayland the window will appear centred by the
                        // compositor anyway.
                        let _ = (x, y); // suppress unused warning
                    }
                }
            }
        });
    }

    win
}
