//! CFG graph panel — renders the Control Flow Graph using Cairo.

use gtk4::prelude::*;
use gtk4::{Box as GtkBox, DrawingArea, Label, Orientation, ScrolledWindow, Widget};
use std::cell::RefCell;
use std::rc::Rc;

use crate::bridge::{AnalysisBridge, BridgeEvent};

// ── Simple node for rendering ─────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct GraphNode {
    label: String,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

// ── Panel ─────────────────────────────────────────────────────────────────────

pub struct GraphPanel {
    root: GtkBox,
}

impl GraphPanel {
    pub fn new(bridge: AnalysisBridge) -> Self {
        let root = GtkBox::new(Orientation::Vertical, 0);

        let header = Label::new(Some("Control Flow Graph"));
        header.add_css_class("panel-header");
        root.append(&header);

        let canvas = DrawingArea::new();
        canvas.set_vexpand(true);
        canvas.set_hexpand(true);
        canvas.set_content_width(800);
        canvas.set_content_height(600);

        // Rc<RefCell> is fine — set_draw_func and bridge.subscribe both run
        // on the GTK main thread, so no Send requirement here.
        let nodes: Rc<RefCell<Vec<GraphNode>>> = Rc::new(RefCell::new(vec![]));

        {
            let nodes = nodes.clone();
            canvas.set_draw_func(move |_area, cr, _w, _h| {
                draw_graph(cr, &nodes.borrow());
            });
        }

        let scroll = ScrolledWindow::builder()
            .vexpand(true)
            .hexpand(true)
            .child(&canvas)
            .build();
        root.append(&scroll);

        // Subscribe: rebuild node list on analysis done.
        // Runs on GTK main thread — Rc, RefCell, DrawingArea are all safe here.
        {
            let nodes  = nodes.clone();
            let canvas = canvas.clone();
            bridge.subscribe(move |event| {
                if let BridgeEvent::AnalysisDone(funcs) = event {
                    let cols = 4usize;
                    let layout: Vec<GraphNode> = funcs
                        .iter()
                        .enumerate()
                        .map(|(i, (name, _))| GraphNode {
                            label: name.clone(),
                            x: 20.0 + (i % cols) as f64 * 190.0,
                            y: 20.0 + (i / cols) as f64 * 80.0,
                            w: 170.0,
                            h: 50.0,
                        })
                        .collect();
                    *nodes.borrow_mut() = layout;
                    canvas.queue_draw();
                }
            });
        }

        Self { root }
    }

    pub fn widget(&self) -> &impl IsA<Widget> {
        &self.root
    }
}

// ── Cairo rendering ───────────────────────────────────────────────────────────

fn draw_graph(cr: &cairo::Context, nodes: &[GraphNode]) {
    cr.set_source_rgb(0.13, 0.13, 0.16);
    cr.paint().ok();
    for node in nodes {
        draw_node(cr, node);
    }
}

fn draw_node(cr: &cairo::Context, node: &GraphNode) {
    let (x, y, w, h, r) = (node.x, node.y, node.w, node.h, 6.0_f64);

    // Rounded rectangle.
    cr.new_sub_path();
    cr.arc(x + w - r, y + r,     r, -std::f64::consts::FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0,                          std::f64::consts::FRAC_PI_2);
    cr.arc(x + r,     y + h - r, r, std::f64::consts::FRAC_PI_2,  std::f64::consts::PI);
    cr.arc(x + r,     y + r,     r, std::f64::consts::PI,         3.0 * std::f64::consts::FRAC_PI_2);
    cr.close_path();

    cr.set_source_rgb(0.12, 0.30, 0.54);
    cr.fill_preserve().ok();
    cr.set_source_rgb(0.22, 0.55, 0.87);
    cr.set_line_width(1.0);
    cr.stroke().ok();

    // Label — truncate if too long.
    let display = if node.label.len() > 22 {
        format!("{}…", &node.label[..21])
    } else {
        node.label.clone()
    };

    cr.set_source_rgb(0.88, 0.92, 1.0);
    cr.select_font_face("monospace", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
    cr.set_font_size(11.0);

    // text_extents returns Result<TextExtents, _> — TextExtents has no Default,
    // so we match instead of unwrap_or_default.
    let (tw, th) = match cr.text_extents(&display) {
        Ok(ext) => (ext.width(), ext.height()),
        Err(_)  => (0.0, 0.0),
    };
    cr.move_to(x + (w - tw) / 2.0, y + (h + th) / 2.0);
    cr.show_text(&display).ok();
}
