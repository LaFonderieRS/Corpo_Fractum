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
use rustdec_ir::{CallTarget, Expr, IrModule, Stmt};
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

// ── Call-graph types ──────────────────────────────────────────────────────────

/// One function node in the call graph, extracted from the IR.
///
/// All fields are `Send + Clone` so the struct travels through
/// `async_channel` without issues.
#[derive(Debug, Clone)]
pub struct CgFunction {
    /// Demangled name (may be synthetic like `sub_401000`).
    pub name:        String,
    /// Entry virtual address.
    pub entry_addr:  u64,
    /// Number of basic blocks in the CFG.
    pub block_count: usize,
    /// Total number of IR statements across all blocks.
    pub stmt_count:  usize,
}

/// One directed call edge in the call graph.
#[derive(Debug, Clone)]
pub struct CgEdge {
    /// Index into `CallGraphData::functions` for the caller.
    pub caller_idx: usize,
    /// Index into `CallGraphData::functions` for the callee.
    ///
    /// `None` for calls that could not be resolved to a known internal
    /// function (e.g. indirect calls, or calls into external libraries
    /// that are only present as imports).
    pub callee_idx: Option<usize>,
    /// Name of the callee, even when `callee_idx` is `None`.
    pub callee_name: String,
    /// Number of distinct call-sites from `caller` to `callee_name`.
    /// Multiple `call` instructions in different basic blocks count separately.
    pub sites: usize,
}

/// Complete call-graph snapshot, extracted from the IR before code generation.
///
/// This is what the graph panel draws.  It is intentionally a plain-data
/// description of the graph: no petgraph types, no `IrFunction` references.
#[derive(Debug, Clone, Default)]
pub struct CallGraphData {
    /// All internal functions, in the order they were discovered.
    pub functions: Vec<CgFunction>,
    /// All call edges, resolved against `functions`.
    pub edges:     Vec<CgEdge>,
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
    /// Full call-graph snapshot, emitted once after `AnalysisDone`.
    CallGraphReady(Arc<CallGraphData>),
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
        let listeners:    Rc<RefCell<Vec<Callback>>>                = Rc::new(RefCell::new(vec![]));
        let function_map: Rc<RefCell<HashMap<String, String>>>      = Rc::new(RefCell::new(HashMap::new()));
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

                // Collect section metadata + raw bytes.
                let sections: Vec<SectionMeta> = obj.sections.iter().map(|s| SectionMeta {
                    name:         s.name.clone(),
                    kind:         s.kind,
                    virtual_addr: s.virtual_addr,
                    size:         s.size,
                    data:         Arc::new(s.data.clone()),
                }).collect();

                let module   = analyse(&obj).map_err(|e| e.to_string())?;

                // Extract call graph *before* codegen — we need the IR structure,
                // not the generated text, to build the graph.
                let cg = extract_call_graph(&module);

                let code = emit_module(&module, lang).map_err(|e| e.to_string())?;

                Ok::<_, String>((sections, cg, code))
            })
            .await;

            // Stream events back to the GTK thread.
            match result {
                Ok(Ok((sections, cg, code))) => {
                    let _ = tx.send(BridgeEvent::SectionsLoaded(sections)).await;
                    for (name, src) in code {
                        let _ = tx.send(BridgeEvent::AnalysisFunctionReady(name, src)).await;
                    }
                    let _ = tx.send(BridgeEvent::AnalysisDone).await;
                    // Send after AnalysisDone so the graph panel appears once
                    // the code view is already populated.
                    let _ = tx.send(BridgeEvent::CallGraphReady(Arc::new(cg))).await;
                }
                Ok(Err(msg)) => { let _ = tx.send(BridgeEvent::AnalysisError(msg)).await; }
                Err(e)       => { let _ = tx.send(BridgeEvent::AnalysisError(e.to_string())).await; }
            }
        });
    }
}

// ── Call-graph extraction ─────────────────────────────────────────────────────

/// Walk the `IrModule` and build a complete `CallGraphData`.
///
/// This runs on the Tokio blocking thread, so it may take a little time on
/// large binaries, but it only runs once per file load.
fn extract_call_graph(module: &IrModule) -> CallGraphData {
    // ── 1. Index all internal functions ──────────────────────────────────────
    //
    // We build two lookup maps:
    //   * name  → function index  (for `CallTarget::Named`)
    //   * addr  → function index  (for `CallTarget::Direct`)
    let mut functions: Vec<CgFunction> = Vec::with_capacity(module.functions.len());
    let mut name_to_idx: HashMap<String, usize> = HashMap::new();
    let mut addr_to_idx: HashMap<u64, usize>    = HashMap::new();

    for func in &module.functions {
        let idx = functions.len();
        let blocks = func.blocks_sorted();
        let stmt_count = blocks.iter().map(|b| b.stmts.len()).sum();

        functions.push(CgFunction {
            name:        func.name.clone(),
            entry_addr:  func.entry_addr,
            block_count: blocks.len(),
            stmt_count,
        });

        name_to_idx.insert(func.name.clone(), idx);
        addr_to_idx.insert(func.entry_addr, idx);
    }

    // ── 2. Extract call edges ─────────────────────────────────────────────────
    //
    // For each (caller_idx, callee_name) pair, count the number of call sites
    // (i.e. how many `call` instructions reference this callee in this caller).
    //
    // Key: (caller_idx, callee_name_string)
    // Value: site count
    let mut edge_map: HashMap<(usize, String), usize> = HashMap::new();

    for (caller_idx, func) in module.functions.iter().enumerate() {
        for block in func.blocks_sorted() {
            for stmt in &block.stmts {
                let callee_name = match stmt {
                    Stmt::Assign {
                        rhs: Expr::Call { target, .. },
                        ..
                    } => match target {
                        CallTarget::Direct(addr) => {
                            addr_to_idx.get(addr)
                                .map(|&i| functions[i].name.clone())
                        }
                        CallTarget::Named(name) => Some(name.clone()),
                        // Indirect calls (function pointers) — we can't
                        // resolve the target statically.
                        CallTarget::Indirect(_) => None,
                    },
                    _ => None,
                };

                if let Some(name) = callee_name {
                    // Self-recursion: keep it — it is a real edge.
                    *edge_map.entry((caller_idx, name)).or_insert(0) += 1;
                }
            }
        }
    }

    // ── 3. Flatten edge map into CgEdge list ──────────────────────────────────
    let mut edges: Vec<CgEdge> = edge_map
        .into_iter()
        .map(|((caller_idx, callee_name), sites)| {
            let callee_idx = name_to_idx.get(&callee_name).copied();
            CgEdge { caller_idx, callee_idx, callee_name, sites }
        })
        .collect();

    // Sort for deterministic output (stable layout across runs).
    edges.sort_by(|a, b| {
        a.caller_idx
            .cmp(&b.caller_idx)
            .then(a.callee_name.cmp(&b.callee_name))
    });

    CallGraphData { functions, edges }
}
