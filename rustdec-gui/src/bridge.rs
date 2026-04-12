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
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use glib::MainContext;
use tokio::runtime::Handle;

use rustdec_analysis::analyse;
use rustdec_codegen::{emit_module, Language};
use rustdec_loader::{load_file, SectionKind};

// ── Section metadata ──────────────────────────────────────────────────────────

/// Metadata for one binary section, including its raw bytes.
///
/// `data` is wrapped in `Arc` so that cloning a `SectionMeta` (or a
/// `BridgeEvent` that contains one) is cheap — only the ref-count changes.
#[derive(Debug, Clone)]
pub struct SectionMeta {
    pub name:         String,
    pub kind:         SectionKind,
    pub virtual_addr: u64,
    pub size:         u64,
    /// Raw bytes of the section (`Arc` for cheap cloning through events).
    pub data:         Arc<Vec<u8>>,
}

// ── Events ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum BridgeEvent {
    AnalysisStarted(PathBuf),
    /// All sections have been loaded; carries metadata + bytes for each.
    SectionsLoaded(Vec<SectionMeta>),
    /// Emitted once per function as codegen completes; carries `(name, code)`.
    AnalysisFunctionReady(String, String),
    /// Emitted after all functions have been streamed — signals completion.
    AnalysisDone,
    AnalysisError(String),
    /// User clicked a function in the explorer — show its decompiled code.
    FunctionSelected(String, String),
    /// User clicked a section in the explorer — show its content.
    SectionSelected(SectionMeta),
}

// ── Callback — no Send bound ──────────────────────────────────────────────────

type Callback = Box<dyn Fn(BridgeEvent) + 'static>;

// ── Bridge ────────────────────────────────────────────────────────────────────

/// Cheap-to-clone handle. Must only be used from the GTK main thread.
#[derive(Clone)]
pub struct AnalysisBridge {
    listeners:    Rc<RefCell<Vec<Callback>>>,
    /// `async_channel::Sender` is `Send + Clone` — safe to move into Tokio tasks.
    tx:           async_channel::Sender<BridgeEvent>,
    rt:           Handle,
    language:     Rc<RefCell<Language>>,
    /// Stores decompiled code keyed by function name.
    function_map: Rc<RefCell<HashMap<String, String>>>,
    /// Stores section metadata (+ bytes) keyed by section name.
    section_map:  Rc<RefCell<HashMap<String, SectionMeta>>>,
}

impl AnalysisBridge {
    pub fn new(rt: Handle) -> Self {
        let (tx, rx) = async_channel::unbounded::<BridgeEvent>();
        let listeners:    Rc<RefCell<Vec<Callback>>>             = Rc::new(RefCell::new(vec![]));
        let function_map: Rc<RefCell<HashMap<String, String>>>   = Rc::new(RefCell::new(HashMap::new()));
        let section_map:  Rc<RefCell<HashMap<String, SectionMeta>>> = Rc::new(RefCell::new(HashMap::new()));

        // Drain the channel on the GTK main thread.
        {
            let listeners    = listeners.clone();
            let function_map = function_map.clone();
            let section_map  = section_map.clone();
            MainContext::default().spawn_local(async move {
                while let Ok(event) = rx.recv().await {
                    // Populate internal maps before dispatching to subscribers.
                    match &event {
                        BridgeEvent::AnalysisStarted(_) => {
                            function_map.borrow_mut().clear();
                            section_map.borrow_mut().clear();
                        }
                        BridgeEvent::SectionsLoaded(secs) => {
                            let mut map = section_map.borrow_mut();
                            for s in secs {
                                map.insert(s.name.clone(), s.clone());
                            }
                        }
                        BridgeEvent::AnalysisFunctionReady(name, code) => {
                            function_map.borrow_mut().insert(name.clone(), code.clone());
                        }
                        _ => {}
                    }
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
            language:     Rc::new(RefCell::new(Language::C)),
            function_map,
            section_map,
        }
    }

    /// Register a callback executed on the GTK main thread.
    pub fn subscribe(&self, cb: impl Fn(BridgeEvent) + 'static) {
        self.listeners.borrow_mut().push(Box::new(cb));
    }

    pub fn set_language(&self, lang: Language) {
        *self.language.borrow_mut() = lang;
    }

    /// Emit `FunctionSelected` synchronously (must be called from GTK main thread).
    pub fn select_function(&self, name: &str) {
        if let Some(code) = self.function_map.borrow().get(name).cloned() {
            let event = BridgeEvent::FunctionSelected(name.to_string(), code);
            for cb in self.listeners.borrow().iter() {
                cb(event.clone());
            }
        }
    }

    /// Emit `SectionSelected` synchronously (must be called from GTK main thread).
    pub fn select_section(&self, name: &str) {
        if let Some(meta) = self.section_map.borrow().get(name).cloned() {
            let event = BridgeEvent::SectionSelected(meta);
            for cb in self.listeners.borrow().iter() {
                cb(event.clone());
            }
        }
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
                let obj = load_file(&path).map_err(|e| e.to_string())?;

                // Collect section metadata + raw bytes (Arc for cheap event cloning).
                let sections: Vec<SectionMeta> = obj.sections.iter().map(|s| SectionMeta {
                    name:         s.name.clone(),
                    kind:         s.kind,
                    virtual_addr: s.virtual_addr,
                    size:         s.size,
                    data:         Arc::new(s.data.clone()),
                }).collect();

                let module = analyse(&obj).map_err(|e| e.to_string())?;
                let code   = emit_module(&module, lang).map_err(|e| e.to_string())?;

                Ok::<_, String>((sections, code))
            })
            .await;

            // Stream events back to the GTK thread.
            match result {
                Ok(Ok((sections, code))) => {
                    let _ = tx.send(BridgeEvent::SectionsLoaded(sections)).await;
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
