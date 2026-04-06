//! Dominator tree and related analyses for a function CFG.
//!
//! ## What is domination?
//!
//! Node A **dominates** node B if every path from the entry to B passes
//! through A.  The **immediate dominator** (idom) of B is the closest
//! dominator of B that is not B itself.  The set of idom relationships
//! forms a tree rooted at the entry node — the **dominator tree**.
//!
//! ## Why it matters for decompilation
//!
//! | Use-case | How dominance helps |
//! |---|---|
//! | Loop detection | A back-edge n→h exists iff h dominates n. |
//! | Natural loop body | All nodes dominated by h and that can reach n. |
//! | If-else convergence | The immediate post-dominator of the branch block. |
//! | Variable liveness scope | A definition at node D is visible only in nodes D dominates. |
//!
//! ## Implementation
//!
//! We delegate to `petgraph::algo::dominators::simple_fast`, which implements
//! the Cooper-Harvey-Kennedy algorithm (a fast practical alternative to
//! Lengauer-Tarjan with the same O(n²) worst-case but better cache behaviour
//! on typical CFGs).  We then expose a richer API on top.

use petgraph::algo::dominators::{self, Dominators};
use petgraph::graph::NodeIndex;
use petgraph::visit::IntoNodeReferences;
use rustdec_ir::IrFunction;
use std::collections::{HashMap, HashSet};
use tracing::{debug, trace};

// ── Public types ──────────────────────────────────────────────────────────────

/// Dominator information for one function.
pub struct DomTree {
    /// Raw petgraph dominator result.
    inner:  Dominators<NodeIndex>,
    /// Entry node of the CFG.
    entry:  NodeIndex,
    /// Pre-computed children map: idom → [dominated nodes].
    children: HashMap<NodeIndex, Vec<NodeIndex>>,
    /// All nodes, for iteration.
    nodes:  Vec<NodeIndex>,
}

impl DomTree {
    /// Compute the dominator tree for `func`.
    ///
    /// Returns `None` if the CFG is empty.
    pub fn compute(func: &IrFunction) -> Option<Self> {
        // Entry node = block with the lowest start address.
        let entry = func.cfg.node_references()
            .min_by_key(|(_, bb)| bb.start_addr)
            .map(|(ni, _)| ni)?;

        debug!(entry = ?entry, "computing dominator tree");

        let inner = dominators::simple_fast(&func.cfg, entry);

        let nodes: Vec<NodeIndex> = func.cfg.node_references()
            .map(|(ni, _)| ni)
            .collect();

        // Build children map.
        let mut children: HashMap<NodeIndex, Vec<NodeIndex>> = HashMap::new();
        for &ni in &nodes {
            if let Some(idom) = inner.immediate_dominator(ni) {
                if idom != ni {
                    children.entry(idom).or_default().push(ni);
                }
            }
        }

        debug!(nodes = nodes.len(), "dominator tree ready");
        Some(Self { inner, entry, children, nodes })
    }

    /// Return the immediate dominator of `node`, or `None` for the entry.
    pub fn idom(&self, node: NodeIndex) -> Option<NodeIndex> {
        let d = self.inner.immediate_dominator(node)?;
        if d == node { None } else { Some(d) }
    }

    /// Return true if `a` dominates `b` (a == b counts as domination).
    pub fn dominates(&self, a: NodeIndex, b: NodeIndex) -> bool {
        if a == b { return true; }
        let mut cur = b;
        loop {
            match self.idom(cur) {
                None => return false,
                Some(p) if p == a => return true,
                Some(p) => cur = p,
            }
        }
    }

    /// Return all nodes dominated by `node` (including itself).
    pub fn dominated_by(&self, node: NodeIndex) -> HashSet<NodeIndex> {
        let mut result = HashSet::new();
        self.collect_subtree(node, &mut result);
        result
    }

    fn collect_subtree(&self, node: NodeIndex, out: &mut HashSet<NodeIndex>) {
        out.insert(node);
        if let Some(children) = self.children.get(&node) {
            for &child in children {
                self.collect_subtree(child, out);
            }
        }
    }

    /// Return the entry node of the CFG.
    pub fn entry(&self) -> NodeIndex { self.entry }

    /// All nodes in the CFG.
    pub fn nodes(&self) -> &[NodeIndex] { &self.nodes }

    /// Direct children in the dominator tree (nodes immediately dominated by `node`).
    pub fn children(&self, node: NodeIndex) -> &[NodeIndex] {
        self.children.get(&node).map(Vec::as_slice).unwrap_or(&[])
    }
}

// ── Loop analysis ─────────────────────────────────────────────────────────────

/// A natural loop identified by its header and body.
#[derive(Debug, Clone)]
pub struct NaturalLoop {
    /// The loop header — the only entry point into the loop.
    pub header: NodeIndex,
    /// All nodes in the loop body, including the header.
    pub body:   HashSet<NodeIndex>,
    /// Back-edge sources (nodes that jump back to the header).
    pub latches: Vec<NodeIndex>,
}

/// Find all natural loops in `func` using the dominator tree.
///
/// A natural loop is induced by a back-edge n→h where h dominates n.
/// The loop body is the set of all nodes that can reach n without
/// going through h (plus h itself).
pub fn find_natural_loops(func: &IrFunction, dom: &DomTree) -> Vec<NaturalLoop> {
    use petgraph::visit::EdgeRef;

    let mut loops: Vec<NaturalLoop> = Vec::new();

    // Collect all back-edges.
    for &ni in dom.nodes() {
        for edge in func.cfg.edges(ni) {
            let target = edge.target();
            // Back-edge: target dominates source.
            if dom.dominates(target, ni) {
                trace!(latch  = ?ni,
                       header = ?target,
                       "back-edge → natural loop");
                let body = compute_loop_body(func, target, ni);
                // Merge with an existing loop if headers match.
                if let Some(existing) = loops.iter_mut().find(|l| l.header == target) {
                    existing.body.extend(body);
                    existing.latches.push(ni);
                } else {
                    loops.push(NaturalLoop {
                        header:  target,
                        body,
                        latches: vec![ni],
                    });
                }
            }
        }
    }

    debug!(loops = loops.len(), "natural loop detection complete");
    loops
}

/// Compute the body of the natural loop with header `h` and latch `n`.
///
/// The body = {h} ∪ { all nodes that can reach n going backward
/// without passing through h }.
fn compute_loop_body(
    func:   &IrFunction,
    header: NodeIndex,
    latch:  NodeIndex,
) -> HashSet<NodeIndex> {
    use petgraph::Direction;

    let mut body = HashSet::new();
    body.insert(header);
    body.insert(latch);

    // BFS/DFS backwards from latch, stopping at header.
    let mut worklist = vec![latch];
    while let Some(node) = worklist.pop() {
        for pred in func.cfg.neighbors_directed(node, Direction::Incoming) {
            if !body.contains(&pred) {
                body.insert(pred);
                if pred != header {
                    worklist.push(pred);
                }
            }
        }
    }
    body
}

// ── Post-dominator helpers ────────────────────────────────────────────────────

/// Find the convergence point (join node) of an if-else rooted at `branch`.
///
/// The convergence point is the immediate post-dominator of `branch`:
/// the first node reached by *all* paths out of the branch.
///
/// We approximate it by computing the common dominator of all nodes
/// reachable from `branch` that are not dominated by it — i.e. the
/// first node outside the if-else scope.
///
/// Returns `None` if no convergence point exists (e.g. both arms exit
/// the function).
pub fn find_convergence(
    func:   &IrFunction,
    branch: NodeIndex,
    dom:    &DomTree,
) -> Option<NodeIndex> {
    use petgraph::visit::EdgeRef;

    // Collect direct successors of branch.
    let succs: Vec<NodeIndex> = func.cfg.edges(branch)
        .map(|e| e.target())
        .collect();

    if succs.len() < 2 { return None; }

    // Walk forward from each successor in topo order and find the first
    // node reachable from all successors that is NOT dominated by branch.
    // Simple approximation: find the LCA (lowest common ancestor) in the
    // dominator tree of the successors' subtrees that is not dominated by branch.

    // For each successor, collect all nodes reachable forward (BFS) that
    // are dominated by branch (= inside the if-else).
    let branch_dominated = dom.dominated_by(branch);

    // The convergence is the first node reachable from all succs that is
    // NOT in branch_dominated — we find it by intersection of reachable sets.
    let mut reachable_sets: Vec<HashSet<NodeIndex>> = succs
        .iter()
        .map(|&s| forward_reachable(func, s, &branch_dominated))
        .collect();

    if reachable_sets.is_empty() { return None; }

    let mut common = reachable_sets.remove(0);
    for set in reachable_sets {
        common = common.intersection(&set).copied().collect();
    }

    // Among common nodes, pick the one with the smallest topological depth
    // (proxy: smallest start_addr since we sorted by address during CFG build).
    common.iter()
        .min_by_key(|&&ni| func.cfg[ni].start_addr)
        .copied()
}

/// BFS forward from `start`, not entering nodes in `blocked`.
fn forward_reachable(
    func:    &IrFunction,
    start:   NodeIndex,
    blocked: &HashSet<NodeIndex>,
) -> HashSet<NodeIndex> {
    use petgraph::visit::EdgeRef;

    let mut visited = HashSet::new();
    let mut queue   = std::collections::VecDeque::new();

    if !blocked.contains(&start) {
        queue.push_back(start);
        visited.insert(start);
    }

    while let Some(ni) = queue.pop_front() {
        for edge in func.cfg.edges(ni) {
            let tgt = edge.target();
            if !blocked.contains(&tgt) && visited.insert(tgt) {
                queue.push_back(tgt);
            }
        }
    }
    visited
}
