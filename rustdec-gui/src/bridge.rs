//! Async bridge between the GTK UI thread and the Tokio analysis backend.
//!
//! # Thread model
//!
//! glib 0.20 removed `MainContext::channel` in favour of `async_channel`.
//! The pattern here:
//!   1. `async_channel::unbounded()` gives a `Sender` (Send + Clone) and a
//!      `Receiver` (not Send — lives on the GTK main thread).
//!   2. Tokio background tasks call `tx.send_blocking(event)`.
//!   3. A `glib::MainContext::default().spawn_local` future drains the
//!      receiver on the GTK main thread and dispatches to subscribers.
//!
//! Subscribers hold `Box<dyn Fn(BridgeEvent) + 'static>` — **no `Send`
//! bound** — so closures may safely capture GTK widgets (`ListBox`,
//! `DrawingArea`, `TextBuffer`, `Rc<RefCell<…>>`, …).

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use glib::MainContext;
use tokio::runtime::Handle;

use rustdec_analysis::analyse;
use rustdec_codegen::{emit_module, Language};
use rustdec_loader::load_file;

// ── Events ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum BridgeEvent {
    AnalysisStarted(PathBuf),
    /// Emitted once per function as codegen completes; carries `(name, code)`.
    AnalysisFunctionReady(String, String),
    /// Emitted after all functions have been streamed — signals completion.
    AnalysisDone,
    AnalysisError(String),
}

// ── Callback — no Send bound ──────────────────────────────────────────────────

type Callback = Box<dyn Fn(BridgeEvent) + 'static>;

// ── Bridge ────────────────────────────────────────────────────────────────────

/// Cheap-to-clone handle. Must only be used from the GTK main thread.
#[derive(Clone)]
pub struct AnalysisBridge {
    listeners: Rc<RefCell<Vec<Callback>>>,
    /// `async_channel::Sender` is `Send + Clone` — safe to move into Tokio tasks.
    tx:        async_channel::Sender<BridgeEvent>,
    rt:        Handle,
    language:  Rc<RefCell<Language>>,
}

impl AnalysisBridge {
    pub fn new(rt: Handle) -> Self {
        let (tx, rx) = async_channel::unbounded::<BridgeEvent>();
        let listeners: Rc<RefCell<Vec<Callback>>> = Rc::new(RefCell::new(vec![]));

        // Drain the channel on the GTK main thread.
        {
            let listeners = listeners.clone();
            MainContext::default().spawn_local(async move {
                while let Ok(event) = rx.recv().await {
                    for cb in listeners.borrow().iter() {
                        cb(event.clone());
                    }
                }
            });
        }

        Self {
            listeners,
            tx,
            rt,
            language: Rc::new(RefCell::new(Language::C)),
        }
    }

    /// Register a callback executed on the GTK main thread.
    /// GTK widgets captured in the closure are fully safe here.
    pub fn subscribe(&self, cb: impl Fn(BridgeEvent) + 'static) {
        self.listeners.borrow_mut().push(Box::new(cb));
    }

    pub fn set_language(&self, lang: Language) {
        *self.language.borrow_mut() = lang;
    }

    /// Start a background analysis for `path`.
    pub fn load_file(&self, path: PathBuf) {
        let lang = *self.language.borrow();
        let tx   = self.tx.clone();

        // Notify immediately (already on GTK main thread).
        for cb in self.listeners.borrow().iter() {
            cb(BridgeEvent::AnalysisStarted(path.clone()));
        }

        // Heavy work on a Tokio blocking thread.
        self.rt.spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                let obj    = load_file(&path).map_err(|e| e.to_string())?;
                let module = analyse(&obj).map_err(|e| e.to_string())?;
                let code   = emit_module(&module, lang).map_err(|e| e.to_string())?;
                Ok::<_, String>(code)
            })
            .await;

            // Stream one event per function so the UI populates progressively.
            match result {
                Ok(Ok(code)) => {
                    for (name, src) in code {
                        let _ = tx.send(BridgeEvent::AnalysisFunctionReady(name, src)).await;
                    }
                    let _ = tx.send(BridgeEvent::AnalysisDone).await;
                }
                Ok(Err(msg)) => { let _ = tx.send(BridgeEvent::AnalysisError(msg)).await; }
                Err(e)       => { let _ = tx.send(BridgeEvent::AnalysisError(e.to_string())).await; }
            }
        });
    }
}
