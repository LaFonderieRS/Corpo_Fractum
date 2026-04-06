//! CFG structuration: flat BasicBlocks + goto → structured AST nodes.
//!
//! ## Algorithm
//!
//! 1. Detect back-edges via DFS to identify loop headers.
//! 2. Build a filtered topological order that skips back-edges (so the
//!    graph is a DAG for the topo pass).
//! 3. Walk the topo order:
//!    - Loop header → emit `Loop { cond, body }`.
//!    - Block with `Branch` terminator → emit `IfElse { cond, then, else_ }`.
//!      The `then` arm covers the true-target subtree, `else_` the fall-through.
//!    - Otherwise → emit a plain `Block`.
//!
//! ## Key fixes over the previous version
//!
//! - `mnemonic` is now read from `Terminator::Branch` instead of being
//!   hardcoded to `"jne"`.
//! - `topo_without_back_edges` actually filters back-edges (the previous
//!   version ignored the parameter entirely).
//! - `IfElse { then, else_ }` now carries the true/false *subtrees* rather
//!   than just the branching block itself.
//! - Loop body detection uses a `HashSet` instead of `Vec::contains`.

use petgraph::graph::NodeIndex;
use petgraph::visit::{EdgeRef, IntoNodeReferences};
use rustdec_ir::{BasicBlock, IrFunction, Terminator};
use std::collections::{HashMap, HashSet, VecDeque};
use tracing::{debug, trace};

// ── Public AST ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum SNode {
    /// A single basic block.
    Block(BlockId),
    /// Straight sequence of nodes.
    Seq(Vec<SNode>),
    /// `if (cond) { then } else { else_ }` — else_ may be an empty Seq.
    IfElse {
        cond:  CondExpr,
        then:  Box<SNode>,
        else_: Box<SNode>,
    },
    /// `while (cond) { body }`.
    Loop {
        cond: CondExpr,
        body: Box<SNode>,
    },
    /// `break` out of the innermost loop.
    Break,
    /// `continue` to the innermost loop header.
    Continue,
}

pub type BlockId = u32;

/// Condition expression for `if`/`while` headers.
#[derive(Debug, Clone)]
pub struct CondExpr {
    /// Address of the block whose terminator is the Branch.
    pub block_addr: u64,
    /// Original x86 branch mnemonic (`"je"`, `"jne"`, `"jl"`, …).
    /// Read directly from `Terminator::Branch::mnemonic`.
    pub branch_mnem: String,
    /// When true the codegen should invert the relational operator.
    pub negate: bool,
}

/// Structured representation of one function.
#[derive(Debug)]
pub struct StructuredFunc {
    pub name:   String,
    pub root:   SNode,
    /// BlockId → BasicBlock, for codegen lookup.
    pub blocks: HashMap<BlockId, BasicBlock>,
}

// ── Main entry point ──────────────────────────────────────────────────────────

pub fn structure_function(func: &IrFunction) -> StructuredFunc {
    debug!(func = %func.name, blocks = func.cfg.node_count(), "structuring function");

    let mut blocks: HashMap<BlockId, BasicBlock> = HashMap::new();
    for (_, bb) in func.cfg.node_references() {
        blocks.insert(bb.id, bb.clone());
    }

    if blocks.is_empty() {
        return StructuredFunc { name: func.name.clone(), root: SNode::Seq(vec![]), blocks };
    }

    let back_edges   = find_back_edges(func);
    let back_set: HashSet<(NodeIndex, NodeIndex)> = back_edges.iter().copied().collect();
    let loop_headers: HashSet<NodeIndex>           = back_edges.iter().map(|(_, h)| *h).collect();

    trace!(func = %func.name,
           back_edges   = back_edges.len(),
           loop_headers = loop_headers.len(),
           "back-edge analysis");

    let topo = topo_without_back_edges(func, &back_set);
    let root = build_sequence(func, &topo, &loop_headers, &back_set);

    debug!(func = %func.name, "structuring complete");
    StructuredFunc { name: func.name.clone(), root, blocks }
}

// ── Back-edge detection ───────────────────────────────────────────────────────

fn find_back_edges(func: &IrFunction) -> Vec<(NodeIndex, NodeIndex)> {
    let entry = func.cfg.node_references()
        .min_by_key(|(_, bb)| bb.start_addr)
        .map(|(ni, _)| ni);

    let mut on_stack: HashSet<NodeIndex> = HashSet::new();
    let mut visited:  HashSet<NodeIndex> = HashSet::new();
    let mut back:     Vec<(NodeIndex, NodeIndex)> = Vec::new();

    if let Some(start) = entry {
        dfs_back(func, start, &mut on_stack, &mut visited, &mut back);
    }
    back
}

fn dfs_back(
    func:     &IrFunction,
    node:     NodeIndex,
    on_stack: &mut HashSet<NodeIndex>,
    visited:  &mut HashSet<NodeIndex>,
    back:     &mut Vec<(NodeIndex, NodeIndex)>,
) {
    if visited.contains(&node) { return; }
    visited.insert(node);
    on_stack.insert(node);

    for edge in func.cfg.edges(node) {
        let tgt = edge.target();
        if on_stack.contains(&tgt) {
            back.push((node, tgt));
        } else if !visited.contains(&tgt) {
            dfs_back(func, tgt, on_stack, visited, back);
        }
    }
    on_stack.remove(&node);
}

// ── Topological order without back-edges ─────────────────────────────────────

/// Kahn's algorithm on the DAG obtained by removing back-edges.
///
/// Unlike the previous `Topo::new(&func.cfg)` call (which ran on the full
/// cyclic graph), this correctly handles loops.
fn topo_without_back_edges(
    func:     &IrFunction,
    back_set: &HashSet<(NodeIndex, NodeIndex)>,
) -> Vec<NodeIndex> {
    // Compute in-degrees ignoring back-edges.
    let mut in_degree: HashMap<NodeIndex, usize> = HashMap::new();
    for (ni, _) in func.cfg.node_references() {
        in_degree.entry(ni).or_insert(0);
        for edge in func.cfg.edges(ni) {
            let tgt = edge.target();
            if !back_set.contains(&(ni, tgt)) {
                *in_degree.entry(tgt).or_insert(0) += 1;
            }
        }
    }

    // Start with zero-in-degree nodes (entry first by address).
    let mut queue: VecDeque<NodeIndex> = in_degree
        .iter()
        .filter(|(_, &d)| d == 0)
        .map(|(&ni, _)| ni)
        .collect();
    queue.make_contiguous().sort_by_key(|&ni| func.cfg[ni].start_addr);

    let mut order = Vec::with_capacity(func.cfg.node_count());
    while let Some(ni) = queue.pop_front() {
        order.push(ni);
        // Decrement successors, enqueue newly zero-in-degree ones.
        let mut newly_free: Vec<NodeIndex> = Vec::new();
        for edge in func.cfg.edges(ni) {
            let tgt = edge.target();
            if back_set.contains(&(ni, tgt)) { continue; }
            let d = in_degree.entry(tgt).or_insert(1);
            *d = d.saturating_sub(1);
            if *d == 0 {
                newly_free.push(tgt);
            }
        }
        // Sort for determinism (lower address first).
        newly_free.sort_by_key(|&ni| func.cfg[ni].start_addr);
        for ni in newly_free { queue.push_back(ni); }
    }

    // Append any remaining nodes (unreachable in DAG sense — safety net).
    let visited: HashSet<NodeIndex> = order.iter().copied().collect();
    for (ni, _) in func.cfg.node_references() {
        if !visited.contains(&ni) {
            order.push(ni);
        }
    }

    order
}

// ── Structured tree builder ───────────────────────────────────────────────────

fn build_sequence(
    func:         &IrFunction,
    order:        &[NodeIndex],
    loop_headers: &HashSet<NodeIndex>,
    back_set:     &HashSet<(NodeIndex, NodeIndex)>,
) -> SNode {
    let mut nodes: Vec<SNode> = Vec::new();
    let mut i = 0;

    while i < order.len() {
        let ni = order[i];

        if loop_headers.contains(&ni) {
            let (node, consumed) = build_loop(func, order, i, ni, back_set);
            nodes.push(node);
            i += consumed;
            continue;
        }

        let bb = &func.cfg[ni];
        match &bb.terminator {
            Terminator::Branch { _true_bb, _false_bb, mnemonic, .. } => {
                // Find the true-target and false-target node indices in the
                // topo slice that follows, and partition the remaining nodes.
                let cond = CondExpr {
                    block_addr:  bb.start_addr,
                    branch_mnem: mnemonic.clone(),
                    negate:      false,
                };

                // Determine which successor NodeIndex corresponds to which arm.
                let (true_ni, false_ni) = successors_for_branch(func, ni, back_set);

                let then_nodes  = collect_arm(func, order, i + 1, true_ni,  loop_headers, back_set);
                let else_nodes  = collect_arm(func, order, i + 1, false_ni, loop_headers, back_set);
                let arm_len     = then_nodes.len().max(else_nodes.len());

                let then_node  = nodes_to_snode(then_nodes);
                let else_node  = nodes_to_snode(else_nodes);

                trace!(func   = %func.name,
                       block   = format_args!("{:#x}", bb.start_addr),
                       mnem    = %mnemonic,
                       "if-else node");

                nodes.push(SNode::IfElse {
                    cond,
                    then:  Box::new(SNode::Seq(vec![SNode::Block(bb.id), then_node])),
                    else_: Box::new(else_node),
                });

                // Skip the nodes consumed by both arms.
                i += 1 + arm_len;
                continue;
            }
            _ => {
                nodes.push(SNode::Block(bb.id));
            }
        }
        i += 1;
    }

    nodes_to_snode(nodes)
}

// ── Loop builder ─────────────────────────────────────────────────────────────

fn build_loop(
    func:     &IrFunction,
    order:    &[NodeIndex],
    start:    usize,
    header:   NodeIndex,
    back_set: &HashSet<(NodeIndex, NodeIndex)>,
) -> (SNode, usize) {
    let header_bb = &func.cfg[header];

    // Collect the set of back-edge sources targeting this header.
    let back_srcs: HashSet<NodeIndex> = back_set
        .iter()
        .filter(|(_, h)| *h == header)
        .map(|(s, _)| *s)
        .collect();

    // The loop body spans from after the header until we hit a back-edge
    // source or a node that exits the loop.
    let mut body_nodes: Vec<SNode> = vec![SNode::Block(header_bb.id)];
    let mut consumed = 1;

    // Build a set of all node indices seen so far for O(1) exit detection.
    let mut loop_set: HashSet<NodeIndex> = HashSet::new();
    loop_set.insert(header);

    for j in (start + 1)..order.len() {
        let ni = order[j];
        consumed += 1;
        loop_set.insert(ni);

        if back_srcs.contains(&ni) {
            body_nodes.push(SNode::Block(func.cfg[ni].id));
            body_nodes.push(SNode::Continue);
            break;
        }

        // Check if any successor is outside the loop (exit node).
        let exits = func.cfg.edges(ni).any(|e| {
            let tgt = e.target();
            !back_set.contains(&(ni, tgt))  // not a back-edge
                && tgt != header             // not the loop header
                && !loop_set.contains(&tgt)  // not already in the loop
        });

        body_nodes.push(SNode::Block(func.cfg[ni].id));
        if exits {
            body_nodes.push(SNode::Break);
            break;
        }
    }

    // Read the mnemonic from the header's terminator if it is a Branch.
    let mnemonic = match &header_bb.terminator {
        Terminator::Branch { mnemonic, .. } => mnemonic.clone(),
        _                                   => "jne".to_string(), // defensive default
    };

    let cond = CondExpr {
        block_addr:  header_bb.start_addr,
        branch_mnem: mnemonic,
        negate:      false,
    };

    trace!(func     = %func.name,
           header   = format_args!("{:#x}", header_bb.start_addr),
           consumed,
           "loop node built");

    (SNode::Loop { cond, body: Box::new(nodes_to_snode(body_nodes)) }, consumed)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Return the (true_ni, false_ni) NodeIndex pair for a Branch block.
/// Falls back to (header, header) if successors can't be found.
fn successors_for_branch(
    func:     &IrFunction,
    ni:       NodeIndex,
    _back_set: &HashSet<(NodeIndex, NodeIndex)>,
) -> (Option<NodeIndex>, Option<NodeIndex>) {
    let bb = &func.cfg[ni];
    match &bb.terminator {
        Terminator::Branch { _true_bb, _false_bb, .. } => {
            // Map BlockId → NodeIndex.
            let find_ni = |bid: u32| -> Option<NodeIndex> {
                func.cfg.node_references()
                    .find(|(_, b)| b.id == bid)
                    .map(|(ni, _)| ni)
            };
            (find_ni(*_true_bb), find_ni(*_false_bb))
        }
        _ => (None, None),
    }
}

/// Collect the nodes in a single arm of an if/else, stopping at the first
/// node that belongs to the other arm or is a loop header.
fn collect_arm(
    func:         &IrFunction,
    order:        &[NodeIndex],
    from:         usize,
    target:       Option<NodeIndex>,
    loop_headers: &HashSet<NodeIndex>,
    back_set:     &HashSet<(NodeIndex, NodeIndex)>,
) -> Vec<SNode> {
    let target = match target {
        Some(t) => t,
        None    => return vec![],
    };

    // Collect nodes reachable from `target` in topo order, stopping when we
    // reach a node that has predecessors coming from outside the arm
    // (= convergence point) or a loop header.
    let mut arm: Vec<SNode> = Vec::new();
    let mut arm_set: HashSet<NodeIndex> = HashSet::new();
    arm_set.insert(target);

    for &ni in &order[from..] {
        if ni == target || arm_set.contains(&ni) {
            if loop_headers.contains(&ni) && ni != target { break; }
            arm.push(SNode::Block(func.cfg[ni].id));
            arm_set.insert(ni);

            // If this node has a successor outside the arm → stop (convergence).
            let exits = func.cfg.edges(ni).any(|e| {
                let tgt = e.target();
                !back_set.contains(&(ni, tgt)) && !arm_set.contains(&tgt)
            });
            if exits { break; }
        }
    }

    arm
}

/// Collapse a `Vec<SNode>` into a single `SNode`.
fn nodes_to_snode(mut nodes: Vec<SNode>) -> SNode {
    match nodes.len() {
        0 => SNode::Seq(vec![]),
        1 => nodes.remove(0),
        _ => SNode::Seq(nodes),
    }
}
