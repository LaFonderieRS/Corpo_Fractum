//! Call-graph construction.
//!
//! [`build_call_graph`] walks every IR statement in every basic block of
//! every function in an [`IrModule`] and produces a [`CallGraph`] — a map
//! from each function name to the (deduplicated, sorted) list of function
//! names it may call.
//!
//! # What is captured
//!
//! | `CallTarget` variant | Behaviour |
//! |---|---|
//! | `Named(name)` | Edge added directly using `name`. |
//! | `Direct(addr)` | Resolved to the callee's name via the entry-address index built from `module.functions`; skipped if the address is not a known function entry. |
//! | `Indirect(_)` | Not recorded — the target is not statically determinable. |
//!
//! Self-recursive calls (`foo → foo`) **are** recorded; the caller decides
//! whether to treat them specially.
//!
//! # Completeness guarantee
//!
//! Every function in `module` appears as a key in the returned [`CallGraph`],
//! even if it never calls anything (its value will be an empty `Vec`).
//! This lets callers distinguish "makes no calls" from "not in the module".

use std::collections::{HashMap, HashSet};

use rustdec_ir::{CallTarget, Expr, IrModule, Stmt};

// ── Public types ──────────────────────────────────────────────────────────────

/// The call graph of an IR module.
///
/// Maps each **caller** function name to the deduplicated, lexicographically
/// sorted list of **callee** names.  Both internal functions and named
/// external symbols (imports, libc wrappers, …) appear as callees.
///
/// Every function present in the source [`IrModule`] is guaranteed to be a
/// key, even if its callee list is empty.
#[derive(Debug, Default, Clone)]
pub struct CallGraph {
    /// Internal adjacency map.  Use the accessor methods rather than this
    /// field; it is public only so callers can destructure in exhaustive
    /// match arms or serialise without a conversion step.
    pub inner: HashMap<String, Vec<String>>,
}

impl CallGraph {
    // ── Construction (used only inside this module) ───────────────────────────

    fn new(inner: HashMap<String, Vec<String>>) -> Self {
        Self { inner }
    }

    // ── Read accessors ────────────────────────────────────────────────────────

    /// All caller names (every function in the source module, in arbitrary order).
    pub fn callers(&self) -> impl Iterator<Item = &str> {
        self.inner.keys().map(String::as_str)
    }

    /// Callees of `caller`, or `None` if `caller` is not a known function.
    pub fn callees(&self, caller: &str) -> Option<&[String]> {
        self.inner.get(caller).map(Vec::as_slice)
    }

    /// Returns `true` if there is a call edge from `caller` to `callee`.
    pub fn has_edge(&self, caller: &str, callee: &str) -> bool {
        self.inner
            .get(caller)
            .map(|cs| cs.binary_search_by_key(&callee, String::as_str).is_ok())
            .unwrap_or(false)
    }

    /// Total number of unique directed edges in the graph.
    pub fn edge_count(&self) -> usize {
        self.inner.values().map(Vec::len).sum()
    }

    /// Total number of nodes (one per function in the source module, plus
    /// any named external callees not present as callers).
    pub fn node_count(&self) -> usize {
        let mut all: HashSet<&str> = HashSet::new();
        for (caller, callees) in &self.inner {
            all.insert(caller);
            for c in callees {
                all.insert(c);
            }
        }
        all.len()
    }

    /// All unique node names — callers *and* named external callees.
    pub fn all_nodes(&self) -> HashSet<&str> {
        let mut nodes = HashSet::new();
        for (caller, callees) in &self.inner {
            nodes.insert(caller.as_str());
            for c in callees {
                nodes.insert(c.as_str());
            }
        }
        nodes
    }
}

// Allow treating `&CallGraph` like `&HashMap<String, Vec<String>>` for
// iteration and indexing.
impl std::ops::Deref for CallGraph {
    type Target = HashMap<String, Vec<String>>;
    fn deref(&self) -> &Self::Target { &self.inner }
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Build the call graph for `module`.
///
/// # Example
///
/// ```ignore
/// let module  = analyse(&binary_object)?;
/// let cg      = build_call_graph(&module);
///
/// if let Some(callees) = cg.callees("main") {
///     for c in callees { println!("main → {c}"); }
/// }
/// ```
pub fn build_call_graph(module: &IrModule) -> CallGraph {
    // Build an entry-address → function-name index used to resolve
    // `CallTarget::Direct(addr)` to the matching callee name.
    let addr_to_name: HashMap<u64, &str> = module
        .functions
        .iter()
        .map(|f| (f.entry_addr, f.name.as_str()))
        .collect();

    let inner: HashMap<String, Vec<String>> = module
        .functions
        .iter()
        .map(|func| {
            // Use a HashSet to deduplicate callees before collecting into a Vec.
            let mut seen: HashSet<String> = HashSet::new();

            for block in func.blocks_sorted() {
                for stmt in &block.stmts {
                    // Only `Stmt::Assign` can contain a `Call` expression.
                    let Stmt::Assign { rhs: Expr::Call { target, .. }, .. } = stmt else {
                        continue;
                    };

                    let callee: Option<String> = match target {
                        CallTarget::Named(name) => {
                            Some(name.clone())
                        }
                        CallTarget::Direct(addr) => {
                            addr_to_name.get(addr).map(|s| (*s).to_string())
                        }
                        // Indirect calls (function pointers, vtable dispatch)
                        // cannot be resolved statically — skip them.
                        CallTarget::Indirect(_) => None,
                    };

                    if let Some(name) = callee {
                        seen.insert(name);
                    }
                }
            }

            // Sort for deterministic output across runs.
            let mut callees: Vec<String> = seen.into_iter().collect();
            callees.sort_unstable();

            (func.name.clone(), callees)
        })
        .collect();

    CallGraph::new(inner)
}
