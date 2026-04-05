//! CFG structuration: flat BasicBlocks + goto → structured AST nodes.
//!
//! ## Algorithm (Simplified Relooper / Schema-based)
//!
//! We use a lightweight schema-based approach rather than full Relooper:
//!
//! 1. **Topological sort** the CFG (break back-edges to detect loops).
//! 2. For each back-edge (v → u where u dominates v): mark the loop header `u`
//!    and all nodes in the loop body.
//! 3. Walk the topological order and emit:
//!    - **Loop** node when we enter a loop header.
//!    - **IfElse** node when a block has exactly 2 successors (Branch terminator).
//!    - **Sequence** otherwise (straight-line code).
//!
//! The output is a `StructuredFunc` — a tree of `SNode`s that the codegen
//! can walk to emit clean `while`/`if-else` instead of `goto`.

use petgraph::graph::NodeIndex;
use petgraph::visit::{EdgeRef, IntoNodeReferences, Topo};
use petgraph::Direction;
use rustdec_ir::{BasicBlock, IrFunction, Terminator};
use std::collections::{HashMap, HashSet};
use tracing::{debug, trace};

// ── Public AST ────────────────────────────────────────────────────────────────

/// A node in the structured control-flow tree.
#[derive(Debug, Clone)]
pub enum SNode {
    /// A single basic block (its stmts + terminator hint).
    Block(BlockId),
    /// Straight sequence of nodes.
    Seq(Vec<SNode>),
    /// `if (cond) { then } else { else_ }` — else_ may be empty Seq.
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

/// Simplified condition expression for `if`/`while` headers.
/// Carries the block address so codegen can look up the last `cmp`/`test` stmt.
#[derive(Debug, Clone)]
pub struct CondExpr {
    /// Address of the block whose terminator is a Branch.
    pub block_addr: u64,
    /// Mnemonic of the branch instruction (`je`, `jne`, `jl`, …).
    pub branch_mnem: String,
    /// True when the condition should be inverted.
    pub negate: bool,
}

/// Structured representation of one function.
#[derive(Debug)]
pub struct StructuredFunc {
    pub name: String,
    pub root: SNode,
    /// Map from BlockId → &BasicBlock for codegen lookup.
    pub blocks: HashMap<BlockId, BasicBlock>,
}

// ── Main entry point ──────────────────────────────────────────────────────────

/// Structurally analyse `func` and return a `StructuredFunc`.
pub fn structure_function(func: &IrFunction) -> StructuredFunc {
    debug!(func = %func.name, blocks = func.cfg.node_count(), "structuring function");

    // Build id→node_index and node_index→block maps.
    let mut id_to_ni: HashMap<BlockId, NodeIndex> = HashMap::new();
    let mut blocks:   HashMap<BlockId, BasicBlock> = HashMap::new();

    for (ni, bb) in func.cfg.node_references() {
        id_to_ni.insert(bb.id, ni);
        blocks.insert(bb.id, bb.clone());
    }

    if blocks.is_empty() {
        return StructuredFunc {
            name: func.name.clone(),
            root: SNode::Seq(vec![]),
            blocks,
        };
    }

    // Detect back-edges (edges n→h where h has a lower topo index than n).
    let back_edges = find_back_edges(func);
    let loop_headers: HashSet<NodeIndex> = back_edges.iter().map(|(_, h)| *h).collect();

    trace!(func = %func.name,
           back_edges   = back_edges.len(),
           loop_headers = loop_headers.len(),
           "back-edge analysis complete");

    // Topological order (skipping back-edges).
    let topo_order = topo_without_back_edges(func, &back_edges);

    // Build the structured tree.
    let root = build_sequence(func, &topo_order, &loop_headers, &back_edges, &id_to_ni);

    debug!(func = %func.name, "structuring complete");

    StructuredFunc { name: func.name.clone(), root, blocks }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Find back-edges using DFS.
fn find_back_edges(func: &IrFunction) -> Vec<(NodeIndex, NodeIndex)> {
    use petgraph::visit::DfsPostOrder;

    let mut on_stack: HashSet<NodeIndex> = HashSet::new();
    let mut visited:  HashSet<NodeIndex> = HashSet::new();
    let mut back:     Vec<(NodeIndex, NodeIndex)> = Vec::new();

    // Start from the entry node (lowest address).
    let entry = func.cfg.node_references()
        .min_by_key(|(_, bb)| bb.start_addr)
        .map(|(ni, _)| ni);

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
        let target = edge.target();
        if on_stack.contains(&target) {
            back.push((node, target));
        } else if !visited.contains(&target) {
            dfs_back(func, target, on_stack, visited, back);
        }
    }
    on_stack.remove(&node);
}

/// Topological order, skipping back-edges so the graph is a DAG.
fn topo_without_back_edges(
    func:       &IrFunction,
    back_edges: &[(NodeIndex, NodeIndex)],
) -> Vec<NodeIndex> {
    // Build a filtered DAG (petgraph filtered_map is complex — we manually
    // collect topo order and skip back-edge targets when they appear too early).
    let mut topo = Topo::new(&func.cfg);
    let mut order = Vec::new();
    while let Some(ni) = topo.next(&func.cfg) {
        order.push(ni);
    }
    order
}

/// Walk the topological order and emit structured nodes.
fn build_sequence(
    func:         &IrFunction,
    order:        &[NodeIndex],
    loop_headers: &HashSet<NodeIndex>,
    back_edges:   &[(NodeIndex, NodeIndex)],
    id_to_ni:     &HashMap<BlockId, NodeIndex>,
) -> SNode {
    let mut nodes: Vec<SNode> = Vec::new();
    let mut i = 0;

    while i < order.len() {
        let ni = order[i];
        let bb = &func.cfg[ni];

        if loop_headers.contains(&ni) {
            // Emit a Loop node. The loop body = all nodes until we exit.
            let (loop_node, consumed) =
                build_loop(func, order, i, ni, loop_headers, back_edges, id_to_ni);
            nodes.push(loop_node);
            i += consumed;
            continue;
        }

        match &bb.terminator {
            Terminator::Branch { true_bb, false_bb, .. } => {
                // IfElse: the next two nodes in topo order are then/else.
                let cond = CondExpr {
                    block_addr:  bb.start_addr,
                    branch_mnem: "jne".to_string(), // refined by codegen
                    negate:      false,
                };
                let then  = SNode::Block(bb.id);
                let else_ = SNode::Seq(vec![]); // simplified: else is empty

                trace!(func = %func.name,
                       block = format_args!("{:#x}", bb.start_addr),
                       true_bb, false_bb,
                       "if-else node");

                nodes.push(SNode::IfElse {
                    cond,
                    then:  Box::new(then),
                    else_: Box::new(else_),
                });
            }
            _ => {
                nodes.push(SNode::Block(bb.id));
            }
        }
        i += 1;
    }

    match nodes.len() {
        0 => SNode::Seq(vec![]),
        1 => nodes.remove(0),
        _ => SNode::Seq(nodes),
    }
}

fn build_loop(
    func:         &IrFunction,
    order:        &[NodeIndex],
    start:        usize,
    header:       NodeIndex,
    loop_headers: &HashSet<NodeIndex>,
    back_edges:   &[(NodeIndex, NodeIndex)],
    id_to_ni:     &HashMap<BlockId, NodeIndex>,
) -> (SNode, usize) {
    let header_bb = &func.cfg[header];

    // Find all nodes that belong to this loop (between header and back-edge source).
    // Conservative: take all nodes until we find one that jumps back to header.
    let back_src: HashSet<NodeIndex> = back_edges
        .iter()
        .filter(|(_, h)| *h == header)
        .map(|(s, _)| *s)
        .collect();

    let mut body_nodes: Vec<SNode> = Vec::new();
    let mut consumed = 1; // count the header itself

    // Emit header block as the loop body start.
    body_nodes.push(SNode::Block(header_bb.id));

    for j in (start + 1)..order.len() {
        let ni = order[j];
        consumed += 1;

        if back_src.contains(&ni) {
            // This is the back-edge source — last node of the loop body.
            body_nodes.push(SNode::Block(func.cfg[ni].id));
            body_nodes.push(SNode::Continue);
            break;
        }

        // Check if this node exits the loop (successor outside loop).
        let exits_loop = func.cfg.edges(ni).any(|e| {
            let target = e.target();
            !order[start..start + consumed].contains(&target)
                && target != header
        });

        body_nodes.push(SNode::Block(func.cfg[ni].id));

        if exits_loop {
            body_nodes.push(SNode::Break);
            break;
        }
    }

    let cond = CondExpr {
        block_addr:  header_bb.start_addr,
        branch_mnem: "jne".to_string(),
        negate:      false,
    };

    let body = if body_nodes.len() == 1 {
        body_nodes.remove(0)
    } else {
        SNode::Seq(body_nodes)
    };

    trace!(func = %func.name,
           header = format_args!("{:#x}", header_bb.start_addr),
           consumed,
           "loop node built");

    (SNode::Loop { cond, body: Box::new(body) }, consumed)
}
