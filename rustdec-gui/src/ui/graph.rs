//! Call-graph panel — hierarchical layout rendered with Cairo.
//!
//! # Data source
//!
//! The panel subscribes to [`BridgeEvent::CallGraphReady`] which carries a
//! [`CallGraphData`] snapshot built directly from the IR (not from the
//! generated C/C++/Rust text).  This gives us:
//!
//! * Every internal function as a node, even those that are never called
//!   (orphan functions, dead code, entry points).
//! * Named external callees (imports like `printf`, `malloc`) as distinct
//!   external nodes.
//! * Edge weights (`sites`) — the number of `call` instructions from one
//!   function to another, used to modulate edge opacity.
//!
//! # Layout algorithm
//!
//! 1. **Collect nodes** — internal functions first, then external callees not
//!    already in the function list.
//! 2. **Kahn's topological sort** to obtain a processing order; nodes that
//!    are part of SCCs (mutual recursion) are appended at the end.
//! 3. **Longest-path layering**: `level[v] = 1 + max(level[u] for u → v)`,
//!    so callers appear above callees.  Completely isolated functions share
//!    a dedicated bottom layer.
//! 4. **Barycenter heuristic** (one forward pass) to reduce crossings within
//!    each layer.
//! 5. Pixel positions from layer index and rank.
//!
//! # Rendering
//!
//! * Internal functions — dark-blue nodes; height scales with `stmt_count`
//!   so complex functions are visually taller.
//! * External / imported symbols — dark-teal nodes, fixed height.
//! * Edges — cubic Bézier, opacity proportional to `sites`.
//! * Arrowheads — small filled triangle at the callee end.
//!
//! # Interactivity
//!
//! * **Click** a node to select it and emit `FunctionSelected` via the bridge.
//! * **Drag** (left-button) to pan the canvas.
//! * **Scroll wheel** to zoom in/out, pivoting around the pointer.
//! * **Hover** highlights the node and colours its incoming/outgoing edges.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::sync::Arc;

use glib::Propagation;
use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, DrawingArea, EventControllerMotion, EventControllerScroll,
    EventControllerScrollFlags, GestureClick, GestureDrag, Label, Orientation,
    ScrolledWindow, Widget,
};

use crate::bridge::{AnalysisBridge, BridgeEvent, CallGraphData};

// ── Layout constants ──────────────────────────────────────────────────────────

const NODE_W:       f64 = 172.0;
/// Minimum node height (external nodes + simple internal nodes).
const NODE_H_MIN:   f64 = 34.0;
/// Extra height added per N statements (capped at `NODE_H_MAX`).
const STMT_PER_PX:  f64 = 8.0;   // 1 px per 8 stmts
const NODE_H_MAX:   f64 = 64.0;
const H_GAP:        f64 = 20.0;
const V_GAP:        f64 = 70.0;
const PAD:          f64 = 32.0;
const ARROW_SIZE:   f64 = 8.0;
const MAX_LABEL:    usize = 23;

// ── Edge highlight style ──────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum EdgeHighlight {
    None,
    /// Edge leaves the hovered node (caller → callee).
    Outgoing,
    /// Edge enters the hovered node (caller → callee).
    Incoming,
}

// ── Internal graph representation ────────────────────────────────────────────

#[derive(Clone)]
struct LayoutNode {
    /// Display name (truncated if needed).
    name:        String,
    /// Full name for tooltip / exact matching.
    full_name:   String,
    x:           f64,
    y:           f64,
    h:           f64,  // node height (variable for internal nodes)
    is_external: bool,
}

#[derive(Clone)]
struct LayoutEdge {
    from:       usize,
    to:         usize,
    /// Raw call-site count — used to derive edge opacity.
    sites:      usize,
}

#[derive(Clone)]
struct GraphState {
    nodes:    Vec<LayoutNode>,
    edges:    Vec<LayoutEdge>,
    canvas_w: f64,
    canvas_h: f64,
    // ── View transform ────────────────────────────────────────────────────────
    offset_x: f64,
    offset_y: f64,
    scale:    f64,
    // ── Drag-pan tracking ────────────────────────────────────────────────────
    /// Offset snapshot captured at drag_begin, used to compute absolute pan.
    pan_base_x: f64,
    pan_base_y: f64,
    // ── Interaction ──────────────────────────────────────────────────────────
    selected: Option<usize>,
    hovered:  Option<usize>,
    /// Most recent pointer position in widget coordinates (used for zoom pivot).
    mouse_x:  f64,
    mouse_y:  f64,
}

impl Default for GraphState {
    fn default() -> Self {
        Self {
            nodes:      Vec::new(),
            edges:      Vec::new(),
            canvas_w:   0.0,
            canvas_h:   0.0,
            offset_x:   0.0,
            offset_y:   0.0,
            scale:      1.0,
            pan_base_x: 0.0,
            pan_base_y: 0.0,
            selected:   None,
            hovered:    None,
            mouse_x:    0.0,
            mouse_y:    0.0,
        }
    }
}

// ── Panel ─────────────────────────────────────────────────────────────────────

pub struct GraphPanel {
    root: GtkBox,
}

impl GraphPanel {
    pub fn new(bridge: AnalysisBridge) -> Self {
        let root = GtkBox::new(Orientation::Vertical, 0);

        let header = Label::new(Some("Call Graph"));
        header.add_css_class("panel-header");
        root.append(&header);

        let state: Rc<RefCell<GraphState>> = Rc::new(RefCell::new(GraphState::default()));

        let canvas = DrawingArea::new();
        canvas.set_vexpand(true);
        canvas.set_hexpand(true);
        canvas.set_content_width(800);
        canvas.set_content_height(600);

        {
            let state = state.clone();
            canvas.set_draw_func(move |_area, cr, _w, _h| {
                let s = state.borrow();
                render(&s, cr);
            });
        }

        // ── Click: select node and emit bridge event ──────────────────────────
        {
            let state      = state.clone();
            let canvas_cb  = canvas.clone();
            let bridge     = bridge.clone();
            let click      = GestureClick::new();
            click.connect_pressed(move |_gesture, _n_press, x, y| {
                let idx = hit_test(&state.borrow(), x, y);
                if let Some(i) = idx {
                    let full_name = state.borrow().nodes[i].full_name.clone();
                    state.borrow_mut().selected = Some(i);
                    canvas_cb.queue_draw();
                    bridge.select_function(&full_name);
                } else {
                    state.borrow_mut().selected = None;
                    canvas_cb.queue_draw();
                }
            });
            canvas.add_controller(click);
        }

        // ── Drag: pan the canvas ──────────────────────────────────────────────
        {
            let state  = state.clone();
            let canvas = canvas.clone();
            let drag   = GestureDrag::new();
            {
                let state = state.clone();
                drag.connect_drag_begin(move |_gesture, _x, _y| {
                    let mut s  = state.borrow_mut();
                    s.pan_base_x = s.offset_x;
                    s.pan_base_y = s.offset_y;
                });
            }
            {
                let state  = state.clone();
                let canvas = canvas.clone();
                drag.connect_drag_update(move |_gesture, dx, dy| {
                    {
                        let mut s  = state.borrow_mut();
                        s.offset_x = s.pan_base_x + dx;
                        s.offset_y = s.pan_base_y + dy;
                    }
                    canvas.queue_draw();
                });
            }
            canvas.add_controller(drag);
        }

        // ── Scroll: zoom around the pointer ──────────────────────────────────
        {
            let state     = state.clone();
            let canvas_cb = canvas.clone();
            let scroll    = EventControllerScroll::new(EventControllerScrollFlags::VERTICAL);
            scroll.connect_scroll(move |_ctrl, _dx, dy| {
                let (mx, my, old_scale, old_ox, old_oy) = {
                    let s = state.borrow();
                    (s.mouse_x, s.mouse_y, s.scale, s.offset_x, s.offset_y)
                };
                // dy < 0 means scroll up (zoom in).
                let factor    = if dy < 0.0 { 1.1 } else { 1.0 / 1.1 };
                let new_scale = (old_scale * factor).clamp(0.1, 8.0);
                // Keep the canvas point under the pointer fixed after zoom.
                let cx = (mx - old_ox) / old_scale;
                let cy = (my - old_oy) / old_scale;
                {
                    let mut s  = state.borrow_mut();
                    s.scale    = new_scale;
                    s.offset_x = mx - cx * new_scale;
                    s.offset_y = my - cy * new_scale;
                }
                canvas_cb.queue_draw();
                Propagation::Proceed
            });
            canvas.add_controller(scroll);
        }

        // ── Motion: hover detection ───────────────────────────────────────────
        {
            let state     = state.clone();
            let canvas_cb = canvas.clone();
            let motion    = EventControllerMotion::new();
            motion.connect_motion(move |_ctrl, x, y| {
                let new_hovered = hit_test(&state.borrow(), x, y);
                let old_hovered = state.borrow().hovered;
                {
                    let mut s = state.borrow_mut();
                    s.mouse_x = x;
                    s.mouse_y = y;
                    s.hovered = new_hovered;
                }
                // Only redraw when the highlighted node changes.
                if new_hovered != old_hovered {
                    canvas_cb.queue_draw();
                }
            });
            canvas.add_controller(motion);
        }

        let scroll = ScrolledWindow::builder()
            .vexpand(true)
            .hexpand(true)
            .child(&canvas)
            .build();
        root.append(&scroll);

        {
            let state  = state.clone();
            let canvas = canvas.clone();

            bridge.subscribe(move |event| match event {
                BridgeEvent::AnalysisStarted(_) => {
                    *state.borrow_mut() = GraphState::default();
                    canvas.set_content_width(800);
                    canvas.set_content_height(600);
                    canvas.queue_draw();
                }

                BridgeEvent::CallGraphReady(cg) => {
                    let gs = layout(&cg);
                    let cw = gs.canvas_w as i32;
                    let ch = gs.canvas_h as i32;
                    *state.borrow_mut() = gs;
                    canvas.set_content_width(cw.max(800));
                    canvas.set_content_height(ch.max(600));
                    canvas.queue_draw();
                }

                _ => {}
            });
        }

        Self { root }
    }

    pub fn widget(&self) -> &impl IsA<Widget> {
        &self.root
    }
}

// ── Layout ────────────────────────────────────────────────────────────────────

fn layout(cg: &Arc<CallGraphData>) -> GraphState {
    // ── 1. Build node list ────────────────────────────────────────────────────
    //
    // Internal functions come first (indices 0..n_internal).
    // External callees that are not already in the function list are appended.

    let n_internal = cg.functions.len();
    let mut all_names: Vec<String>      = cg.functions.iter().map(|f| f.name.clone()).collect();
    let mut is_external: Vec<bool>      = vec![false; n_internal];
    let mut stmt_counts: Vec<usize>     = cg.functions.iter().map(|f| f.stmt_count).collect();

    // Index: name → slot
    let mut name_to_slot: HashMap<String, usize> = all_names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.clone(), i))
        .collect();

    for edge in &cg.edges {
        if !name_to_slot.contains_key(&edge.callee_name) {
            let slot = all_names.len();
            all_names.push(edge.callee_name.clone());
            is_external.push(true);
            stmt_counts.push(0);
            name_to_slot.insert(edge.callee_name.clone(), slot);
        }
    }

    let n = all_names.len();

    // ── 2. Adjacency ──────────────────────────────────────────────────────────

    let mut succs: Vec<Vec<usize>> = vec![vec![]; n];
    let mut preds: Vec<Vec<usize>> = vec![vec![]; n];
    let mut layout_edges: Vec<LayoutEdge> = Vec::new();
    let mut seen_pairs: HashSet<(usize, usize)> = HashSet::new();

    for edge in &cg.edges {
        let from = edge.caller_idx;
        let to   = match edge.callee_idx {
            Some(i) => i,
            None    => *name_to_slot.get(&edge.callee_name).unwrap(),
        };
        // Deduplicate by (from, to) pair; accumulate sites.
        if seen_pairs.insert((from, to)) {
            succs[from].push(to);
            preds[to].push(from);
            layout_edges.push(LayoutEdge { from, to, sites: edge.sites });
        } else {
            // Already have this pair — add sites to existing edge.
            if let Some(e) = layout_edges.iter_mut().find(|e| e.from == from && e.to == to) {
                e.sites += edge.sites;
            }
        }
    }

    // ── 3. Kahn topological sort ──────────────────────────────────────────────

    let mut in_deg: Vec<usize> = (0..n).map(|i| preds[i].len()).collect();
    let mut queue: VecDeque<usize> = (0..n).filter(|&i| in_deg[i] == 0).collect();
    let mut topo: Vec<usize> = Vec::with_capacity(n);

    while let Some(u) = queue.pop_front() {
        topo.push(u);
        for &v in &succs[u] {
            in_deg[v] -= 1;
            if in_deg[v] == 0 { queue.push_back(v); }
        }
    }
    // Append SCC members not reached by Kahn's (mutual recursion cycles).
    let in_topo: HashSet<usize> = topo.iter().cloned().collect();
    for i in 0..n { if !in_topo.contains(&i) { topo.push(i); } }

    // ── 4. Longest-path level assignment ─────────────────────────────────────

    let mut level: Vec<usize> = vec![0; n];
    for &u in &topo {
        let pred_max = preds[u].iter().map(|&p| level[p]).max();
        level[u] = pred_max.map(|l| l + 1).unwrap_or(0);
    }

    // Completely isolated nodes (no edges at all) go to their own bottom layer.
    let max_connected_level = topo
        .iter()
        .filter(|&&i| !succs[i].is_empty() || !preds[i].is_empty())
        .map(|&i| level[i])
        .max()
        .unwrap_or(0);

    for i in 0..n {
        if succs[i].is_empty() && preds[i].is_empty() {
            level[i] = max_connected_level + 1;
        }
    }

    let max_level = *level.iter().max().unwrap_or(&0);

    // ── 5. Group by level ─────────────────────────────────────────────────────

    let mut by_level: Vec<Vec<usize>> = vec![vec![]; max_level + 1];
    for (i, &lv) in level.iter().enumerate() {
        by_level[lv].push(i);
    }

    // ── 6. Barycenter heuristic — one forward pass ────────────────────────────

    // `x_rank[i]` tracks the fractional rank of node i within its layer,
    // used as the sorting key for the next layer.
    let mut x_rank: Vec<f64> = (0..n).map(|i| i as f64).collect();

    for lv in 1..=max_level {
        for &u in &by_level[lv] {
            let parent_xs: Vec<f64> = preds[u]
                .iter()
                .filter(|&&p| level[p] < lv)   // only look at layers above
                .map(|&p| x_rank[p])
                .collect();
            if !parent_xs.is_empty() {
                x_rank[u] = parent_xs.iter().sum::<f64>() / parent_xs.len() as f64;
            }
        }
        by_level[lv].sort_by(|&a, &b| {
            x_rank[a].partial_cmp(&x_rank[b]).unwrap_or(std::cmp::Ordering::Equal)
        });
        // Reassign integer ranks after sorting.
        for (rank, &u) in by_level[lv].iter().enumerate() {
            x_rank[u] = rank as f64;
        }
    }

    // ── 7. Pixel positions ────────────────────────────────────────────────────

    // Node heights depend on statement count for internal nodes.
    let node_h: Vec<f64> = (0..n).map(|i| {
        if is_external[i] {
            NODE_H_MIN
        } else {
            let extra = (stmt_counts[i] as f64 / STMT_PER_PX).min(NODE_H_MAX - NODE_H_MIN);
            NODE_H_MIN + extra
        }
    }).collect();

    // y-offset of each layer accounts for the tallest node in the previous layer.
    let mut layer_y: Vec<f64> = vec![PAD; max_level + 1];
    for lv in 1..=max_level {
        let prev_max_h = by_level[lv - 1]
            .iter()
            .map(|&i| node_h[i])
            .fold(NODE_H_MIN, f64::max);
        layer_y[lv] = layer_y[lv - 1] + prev_max_h + V_GAP;
    }

    let mut px = vec![0.0f64; n];
    let mut py = vec![0.0f64; n];
    let mut canvas_w = 0.0f64;
    let mut canvas_h = 0.0f64;

    for (lv, group) in by_level.iter().enumerate() {
        for (rank, &ni) in group.iter().enumerate() {
            let x = PAD + rank as f64 * (NODE_W + H_GAP);
            let y = layer_y[lv];
            px[ni] = x;
            py[ni] = y;
            canvas_w = canvas_w.max(x + NODE_W + PAD);
            canvas_h = canvas_h.max(y + node_h[ni] + PAD);
        }
    }

    // ── 8. Assemble output ────────────────────────────────────────────────────

    let nodes: Vec<LayoutNode> = all_names.iter().enumerate().map(|(i, name)| {
        let label = truncate(name, MAX_LABEL);
        LayoutNode {
            name:        label,
            full_name:   name.clone(),
            x:           px[i],
            y:           py[i],
            h:           node_h[i],
            is_external: is_external[i],
        }
    }).collect();

    // Remap edges to LayoutEdge (already using slot indices).
    let edges = layout_edges;

    GraphState { nodes, edges, canvas_w, canvas_h, ..Default::default() }
}

// ── Hit testing ───────────────────────────────────────────────────────────────

/// Returns the index of the node at widget position `(wx, wy)`, or `None`.
fn hit_test(state: &GraphState, wx: f64, wy: f64) -> Option<usize> {
    // Convert widget coordinates to canvas coordinates.
    let cx = (wx - state.offset_x) / state.scale;
    let cy = (wy - state.offset_y) / state.scale;
    state.nodes.iter().position(|n| {
        cx >= n.x && cx <= n.x + NODE_W && cy >= n.y && cy <= n.y + n.h
    })
}

// ── Rendering ─────────────────────────────────────────────────────────────────

fn render(state: &GraphState, cr: &cairo::Context) {
    // Background — painted before the transform so it always fills the widget.
    cr.set_source_rgb(0.118, 0.118, 0.157);
    cr.paint().ok();

    if state.nodes.is_empty() {
        // Placeholder shown before a binary is opened.
        cr.set_source_rgb(0.35, 0.37, 0.45);
        cr.select_font_face("monospace", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
        cr.set_font_size(13.0);
        cr.move_to(PAD, PAD + 20.0);
        cr.show_text("Open a binary to see the call graph.").ok();
        return;
    }

    // Apply pan + zoom transform.
    cr.save().ok();
    cr.translate(state.offset_x, state.offset_y);
    cr.scale(state.scale, state.scale);

    // Edges first (drawn behind nodes).
    let max_sites = state.edges.iter().map(|e| e.sites).max().unwrap_or(1).max(1);
    for edge in &state.edges {
        if edge.from < state.nodes.len() && edge.to < state.nodes.len() {
            let alpha = 0.35 + 0.55 * (edge.sites as f64 / max_sites as f64);
            let highlight = match state.hovered {
                Some(h) if edge.from == h => EdgeHighlight::Outgoing,
                Some(h) if edge.to   == h => EdgeHighlight::Incoming,
                _                         => EdgeHighlight::None,
            };
            draw_edge(cr, &state.nodes[edge.from], &state.nodes[edge.to], alpha, highlight);
        }
    }

    // Nodes on top.
    for (i, node) in state.nodes.iter().enumerate() {
        let selected = state.selected == Some(i);
        let hovered  = state.hovered  == Some(i);
        draw_node(cr, node, selected, hovered);
    }

    cr.restore().ok();
}

fn draw_node(cr: &cairo::Context, node: &LayoutNode, selected: bool, hovered: bool) {
    let (x, y, w, h, r) = (node.x, node.y, NODE_W, node.h, 6.0_f64);
    rounded_rect(cr, x, y, w, h, r);

    // Fill — slightly brighter on hover.
    if node.is_external {
        if hovered {
            cr.set_source_rgb(0.12, 0.30, 0.33);
        } else {
            cr.set_source_rgb(0.07, 0.20, 0.22);
        }
        cr.fill_preserve().ok();
        let stroke: (f64, f64, f64) = if selected     { (0.98, 0.82, 0.25) }
                                      else if hovered  { (0.34, 0.72, 0.78) }
                                      else             { (0.22, 0.50, 0.54) };
        cr.set_source_rgba(stroke.0, stroke.1, stroke.2, 0.95);
    } else {
        if hovered {
            cr.set_source_rgb(0.10, 0.26, 0.48);
        } else {
            cr.set_source_rgb(0.07, 0.18, 0.35);
        }
        cr.fill_preserve().ok();
        let stroke: (f64, f64, f64) = if selected     { (0.98, 0.82, 0.25) }
                                      else if hovered  { (0.42, 0.68, 1.00) }
                                      else             { (0.18, 0.43, 0.72) };
        cr.set_source_rgba(stroke.0, stroke.1, stroke.2, 0.95);
    }

    cr.set_line_width(if selected || hovered { 2.0 } else { 1.2 });
    cr.stroke().ok();

    // Function name.
    cr.set_source_rgb(0.86, 0.91, 1.0);
    cr.select_font_face("monospace", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
    cr.set_font_size(10.5);
    let (tw, th) = cr.text_extents(&node.name)
        .map(|e| (e.width(), e.height()))
        .unwrap_or((0.0, 0.0));
    cr.move_to(x + (w - tw) / 2.0, y + (h + th) / 2.0 - 2.0);
    cr.show_text(&node.name).ok();
}

fn draw_edge(
    cr: &cairo::Context,
    from: &LayoutNode,
    to: &LayoutNode,
    alpha: f64,
    highlight: EdgeHighlight,
) {
    // Source: bottom-centre of caller.
    let sx = from.x + NODE_W / 2.0;
    let sy = from.y + from.h;
    // Destination: top-centre of callee.
    let ex = to.x + NODE_W / 2.0;
    let ey = to.y;

    // For back-edges (callee is above caller) or self-loops, use a wider curve.
    let back_edge = ey <= sy;
    let ctrl_dy = if back_edge {
        // Pull the curve far to one side so it doesn't overlap nodes.
        let span = (ex - sx).abs().max(80.0);
        span * 0.8
    } else {
        ((ey - sy) / 2.0).max(24.0)
    };

    let (cx1, cy1, cx2, cy2) = if back_edge {
        // Route around the left of the graph.
        (sx - ctrl_dy, sy + 20.0, ex - ctrl_dy, ey - 20.0)
    } else {
        (sx, sy + ctrl_dy, ex, ey - ctrl_dy)
    };

    let (r, g, b, a, lw) = match highlight {
        EdgeHighlight::Outgoing => (0.34, 0.85, 0.55, (alpha + 0.3).min(1.0), 2.2), // green
        EdgeHighlight::Incoming => (0.95, 0.62, 0.28, (alpha + 0.3).min(1.0), 2.2), // amber
        EdgeHighlight::None     => (0.32, 0.52, 0.78, alpha,                   1.4), // blue
    };

    cr.set_source_rgba(r, g, b, a);
    cr.set_line_width(lw);
    cr.move_to(sx, sy);
    cr.curve_to(cx1, cy1, cx2, cy2, ex, ey);
    cr.stroke().ok();

    draw_arrowhead(cr, cx2, cy2, ex, ey, r, g, b, (a + 0.1).min(1.0));
}

fn draw_arrowhead(
    cr: &cairo::Context,
    fx: f64, fy: f64,
    tx: f64, ty: f64,
    r: f64, g: f64, b: f64, a: f64,
) {
    let angle   = (ty - fy).atan2(tx - fx);
    let spread  = std::f64::consts::PI / 5.5;
    let ax1 = tx - ARROW_SIZE * (angle - spread).cos();
    let ay1 = ty - ARROW_SIZE * (angle - spread).sin();
    let ax2 = tx - ARROW_SIZE * (angle + spread).cos();
    let ay2 = ty - ARROW_SIZE * (angle + spread).sin();

    cr.set_source_rgba(r, g, b, a);
    cr.move_to(tx, ty);
    cr.line_to(ax1, ay1);
    cr.line_to(ax2, ay2);
    cr.close_path();
    cr.fill().ok();
}

fn rounded_rect(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let pi = std::f64::consts::PI;
    cr.new_sub_path();
    cr.arc(x + w - r, y + r,     r, -pi / 2.0,       0.0);
    cr.arc(x + w - r, y + h - r, r,  0.0,             pi / 2.0);
    cr.arc(x + r,     y + h - r, r,  pi / 2.0,        pi);
    cr.arc(x + r,     y + r,     r,  pi,        3.0 * pi / 2.0);
    cr.close_path();
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn truncate(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() > max {
        let t: String = chars[..max - 1].iter().collect();
        format!("{t}…")
    } else {
        s.to_string()
    }
}
