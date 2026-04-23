# GUI user guide

`rustdec-gui` is the GTK4 desktop application. It provides the same decompilation pipeline as the CLI in an interactive three-panel interface.

---

## Build and run

```bash
# System dependencies (GTK4 + Cairo headers)
sudo apt install libgtk-4-dev libcairo2-dev pkg-config   # Debian/Ubuntu
sudo pacman -S gtk4 cairo pkgconf                         # Arch
sudo dnf install gtk4-devel cairo-devel                   # Fedora

# Build (default — no console panel)
cargo build --release -p rustdec-gui

# Build with console panel below code + graph
cargo build --release -p rustdec-gui --features console-bottom

# Build with console panel as a tab next to the call graph
cargo build --release -p rustdec-gui --features console-tab

# Run
./target/release/corpo-fractum
```

---

## Window layout

```
┌──────────────────────────────────────────────────────────────────┐
│  HeaderBar   [ Open… ]                    [ C ▾ ]   [ About ]   │
├─────────────────┬────────────────────────────────────────────────┤
│                 │                                                 │
│   Explorer      │   Code panel                                   │
│   (240 px)      │                                                 │
│                 │   Decompiled pseudo-code or section hex dump   │
│   SECTIONS      │                                                 │
│   [code] .text  │                                                 │
│   [data] .bss   ├─────────────────────────────────────────────────┤
│   …             │                                                 │
│                 │   Call Graph                                    │
│   FUNCTIONS (N) │                                                 │
│   [full script] │   Pan · Zoom · Click                           │
│   main          │                                                 │
│   compute       │                                                 │
│   …             │                                                 │
│                 │                                                 │
│   🔍 Search…    │                                                 │
└─────────────────┴─────────────────────────────────────────────────┘
```

Default window size: 1400 × 900. All pane dividers are draggable.

---

## Opening a binary

Click **Open…** in the header bar and select any ELF, PE, or Mach-O binary. Analysis starts immediately in the background. The explorer panel fills incrementally as functions are lifted — you do not need to wait for the full analysis to start reading results.

---

## Explorer panel

**Sections** — click any section button to view its content in the code panel. Each button shows:
- A coloured kind badge: `code`, `rodata`, `data`, `bss`, `debug`, or `other`.
- The section name.
- The virtual size in bytes.

**Functions** — the list grows as each function is decompiled. The counter `FUNCTIONS (N)` updates in real time.

**Search** — type in the search bar to filter function names (case-insensitive substring match).

**Full script** — the button in the functions header becomes active once the analysis completes. Clicking it loads all decompiled functions concatenated in analysis order into the code panel.

---

## Code panel

Displays the output for the currently selected function or section.

**Function view** — syntax-highlighted pseudo-code in the language chosen from the header-bar dropdown (C, C++, or Rust). Highlighting is applied incrementally (250 lines per idle tick) to keep the UI responsive on large functions.

| Element | Colour |
|---|---|
| Keywords | blue, bold |
| `//` comments | green |
| Hex addresses | cyan |
| ASCII string literals | orange |

**Section view** — for data sections: a hex dump (first 4 KB) plus extracted printable strings with their offsets.

---

## Call-graph panel

An interactive Cairo-rendered graph of all function calls in the binary.

### Navigation

| Gesture | Effect |
|---|---|
| Left click on a node | Select function; code panel updates immediately |
| Left drag | Pan the canvas |
| Scroll wheel | Zoom in/out (range: 0.1× – 8.0×); pivot at the pointer |
| Hover | Highlights the node's outgoing (green) and incoming (amber) edges |

### Node appearance

| Node type | Colour | Height |
|---|---|---|
| Internal function | dark blue | 34 – 64 px, scales with statement count |
| External / import | dark teal | 34 px (fixed) |
| Selected | yellow/gold border | — |

Edge opacity is proportional to the number of call sites between the two functions. Back-edges (recursive or cyclic calls) curve around the left side of the graph.

### Layout

The graph is laid out once when `CallGraphReady` is received:

1. Topological sort (Kahn, handles SCCs).
2. Longest-path layer assignment.
3. Barycenter heuristic (one forward pass) to reduce edge crossings.
4. Cubic Bézier curves with filled arrowheads.

---

## Console panel

Available only when built with `console-bottom` or `console-tab`.

Shows two streams of text:

**Lifecycle events** (from the bridge):
- `[info] analysis started: /path/to/binary`
- `[info] analysis done — N function(s) lifted`
- `[error] …` on failure

**Tracing log records** from all crates, colour-coded by level:

| Level | Colour |
|---|---|
| INFO | grey |
| DEBUG | blue-grey |
| WARN | gold |
| ERROR | salmon |

The console auto-scrolls to the latest entry.

---

## Language selection

The dropdown in the header bar changes the output language for all subsequent `FunctionSelected` and `Full script` events. It does **not** re-run the analysis; language switching is done in the codegen step and is instant.

---

## Logging to stdout

Independent of the console panel, all tracing events are also written to stdout. Filter with `RUSTDEC_LOG`:

```bash
RUSTDEC_LOG=debug ./target/release/corpo-fractum
RUSTDEC_LOG=rustdec_analysis=debug,info ./target/release/corpo-fractum
```

---

## About dialog

Click **About** in the header bar to view the version, description, contributor list, and build information (GTK version, Rust toolchain).
