# corpo-fractum-gui

GTK4 graphical front-end for Corpo Fractum.

Provides a three-panel desktop application: a binary explorer on the left, a decompiled code viewer in the center, and an interactive call-graph on the right. An optional console panel shows real-time analysis logs.

---

## Layout

```
┌─────────────────────────────────────────────────────────────┐
│  HeaderBar  [ Open… ]              [ C ▾ ]  [ About ]       │
├──────────────┬──────────────────────────────────────────────┤
│   Explorer   │  Code (decompiled / hex dump)                │
│              │                                              │
│  SECTIONS    │  uint64_t main(int argc, char **argv) {      │
│  [code] .text│    int64_t local_0;                          │
│  [data] .bss │    local_0 = argc;                           │
│              │    …                                         │
│  FUNCTIONS(N)│                                              │
│  [full script│                                              │
│  main        ├──────────────────────────────────────────────┤
│  compute     │  Call Graph                                  │
│  parse_args  │                                              │
│  …           │   ┌─────────┐      ┌──────────┐              │
│              │   │  main   │────▶│ compute  │              │
│  🔍 Search…  │   └─────────┘      └──────────┘              │
└──────────────┴──────────────────────────────────────────────┘
```

Default window size: 1400 × 900.

---

## Features

### Explorer panel

- **Sections list** — one button per binary section, with a coloured kind badge (`code`, `rodata`, `data`, `bss`, `debug`, `other`) and the virtual size.
- **Function list** — populated incrementally as each function is lifted; shows a live counter `FUNCTIONS (N)`.
- **Search bar** — instant case-insensitive substring filter on function names.
- **Full script** button — concatenates all decompiled functions in analysis order (enabled once analysis completes).

### Code panel

- Displays decompiled pseudo-code (C, C++, or Rust depending on the header-bar dropdown).
- Switches to a **hex dump** + extracted-string view when a data/rodata section is selected.
- **Incremental syntax highlighting** via idle ticks (250 lines per frame) to keep the UI responsive on large outputs:
  - Keywords (`void`, `int`, `return`, `if`, `while`, …) — blue, bold
  - Line comments (`//`) — green
  - Hex addresses — cyan
  - ASCII literals — orange

### Call-graph panel

Cairo-rendered, fully interactive:

| Gesture | Action |
|---|---|
| Left click on a node | Select function → updates code panel |
| Left drag | Pan the canvas |
| Scroll wheel | Zoom (0.1× – 8.0×), pivot at pointer |
| Hover over a node | Highlight outgoing edges (green) and incoming edges (amber) |

**Layout algorithm:**
1. Kahn topological sort (SCC-safe).
2. Longest-path layer assignment.
3. Barycenter heuristic (one forward pass) to reduce edge crossings.
4. Cubic Bézier edges with opacity proportional to call-site count.
5. Node height scales with statement count (34 – 64 px range).
6. External / imported functions rendered in a distinct teal colour.

### Console panel (optional)

Streams two sources of text:

- **Bridge lifecycle events** — analysis started, functions lifted, errors.
- **Tracing log records** — all `tracing` events from every crate, colour-coded by level.

| Level |  Colour   |
|-------|-----------|
| INFO  | gray      |
| DEBUG | blue-gray |
| WARN  | gold      |
| ERROR | salmon    |

---

## Build features

| Feature | Effect |
|---|---|
| *(none)* | No console panel |
| `console-bottom` | Console in a vertical split below code + graph |
| `console-tab` | Console as a tab alongside the graph |

Both features together trigger a compile error.

```bash
# Default (no console)
cargo build --bin corpo-fractum

# Console below
cargo build --bin corpo-fractum --features console-bottom

# Console as tab
cargo build --bin corpo-fractum --features console-tab
```

The resulting binary is named `corpo-fractum`.

---

## Dependencies

| Crate | Role |
|---|---|
| `gtk4` (≥ 4.12) | Widgets, layout, events |
| `cairo-rs` | Call-graph rendering |
| `glib` | Event loop, idle callbacks, timeouts |
| `async-channel` | Thread-safe bridge between Tokio and the GTK main thread |
| `tokio` (multi-thread, 4 workers) | Analysis pipeline |
| `tracing` / `tracing-subscriber` | Structured logging |
| `rustdec-loader` | Binary format parsing |
| `rustdec-disasm` | Disassembly |
| `rustdec-ir` | SSA intermediate representation |
| `rustdec-analysis` | CFG, structuration, call graph |
| `rustdec-codegen` | C / C++ / Rust code emission |

---

## Async architecture

```
GTK main thread                     Tokio thread pool
──────────────────                  ─────────────────────────────────────
open_file_dialog()
  └─ bridge.load_file(path)  ──►  spawn_blocking {
                                    1. load_file(path)
                                    2. analyse(&obj)       // CFG + lift
                                    3. extract_call_graph()
                                    4. emit_module()       // codegen
                                    stream BridgeEvents via tx
                                  }
glib::spawn_local(async {
  loop { event = rx.recv().await
    → notify all panel subscribers
  }
})
```

`BridgeEvent` variants:

| Event | Carries |
|---|---|
| `AnalysisStarted` | `PathBuf` |
| `SectionsLoaded` | `Vec<SectionMeta>` |
| `AnalysisFunctionReady` | function name + decompiled source |
| `AnalysisDone` | — |
| `AnalysisError` | error string |
| `CallGraphReady` | `Arc<CallGraphData>` |
| `FunctionSelected` | name + source |
| `SectionSelected` | `SectionMeta` |

Subscribers are plain `Fn(BridgeEvent)` closures with no `Send` bound — they may capture GTK widgets directly.

---

## Logging

The `GtkLogLayer` tracing subscriber captures every log record from every crate and forwards it to the console panel over an `async_channel`. It never blocks: records are silently dropped if the receiver is gone.

The standard `fmt` layer still writes to stdout and is controlled by `RUSTDEC_LOG`:

```bash
RUSTDEC_LOG=debug ./target/release/corpo-fractum-gui
RUSTDEC_LOG=rustdec_analysis=debug,info ./target/release/corpo-fractum-gui
```

---

## Splash screen

An undecorated, always-on-top splash window appears at startup and auto-dismisses after 6 seconds or as soon as the analysis completes. It shows a status label and a progress bar.
