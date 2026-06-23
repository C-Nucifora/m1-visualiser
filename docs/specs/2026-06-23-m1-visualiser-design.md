<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# m1-visualiser design

Status: v1 complete (structural-first); value/diff overlay shipped. Last updated
2026-06-23.

## Purpose

`m1-visualiser` turns a loaded MoTeC M1 project into an interactive graph so an
engineer can *see* the project's structure — how channels, parameters,
constants, tables and functions relate — rather than reading it out of XML or a
flat documentation page. It is a sibling tool to `m1-doc` (project docs) and
`m1-eval` (the evaluator/interpreter), built on the same toolchain.

The primary output is a single self-contained interactive HTML file you can
open in any browser with no server and no network access. Static exports
(Graphviz DOT and a JSON graph) are also produced for embedding in pipelines,
diffing, and rendering with other tooling.

## Phasing

- **v1 — structural-first.** Build the graph purely from the project's static
  structure as reported by `m1-typecheck`'s symbol table. No numeric values. All
  four edge types below — hierarchy, table-axis, schedule, and data-flow — are
  implemented, alongside the interactive viewer.
- **Value + diff overlay (shipped).** Overlay computed numeric values (channel
  results, table lookups, parameter values) onto nodes by consuming `m1-eval`.
  This is opt-in: with no overlay flag the binary is byte-for-byte the v1 tool.
  See [Value + diff overlay](#value--diff-overlay) below.

## The four edge types

The graph models four distinct relationship kinds between nodes. Each is a
variant of `EdgeKind`:

1. **DataFlow** — a script reads one symbol and writes another (e.g. a function
   reads `Root.Engine.Speed` and writes `Root.Engine.Limited`). This is the
   read/write dependency between channels/parameters and the functions that
   produce them, extracted by per-script CST analysis of reads and writes
   (`loader::add_data_flow_edges` over `dataflow::io_sets`). Reads point into the
   backing function; writes point out of it, so "what feeds this channel" is a
   pure upstream walk.
2. **TableAxis** — links a table's input-axis symbols (and its output `.Value`
   channel) to the table node, derived from `Symbol.table_meta` (axis count and
   units) plus dotted-path containment of the table's members. Models the
   lookup/breakpoint relationship.
3. **Hierarchy** — subsystem/group containment, derived from the dotted symbol
   path (`Root.Engine.Speed` is contained by `Root.Engine`, which is contained
   by `Root`). These become Cytoscape compound (parent) nodes so a subsystem can
   be collapsed/expanded.
4. **Schedule** — execution-rate / scheduling relationship, derived from
   `Symbol.call_rate_hz` (a function's trigger rate in Hz) and channel
   `log_rate_hz`. Connects timing/clock context to the nodes that run at that
   rate.

## Node kinds

`NodeKind` covers the structural entities we surface:

- `Channel` — a runtime signal (`SymbolKind::Channel`).
- `Parameter` — a tunable (`SymbolKind::Parameter`).
- `Constant` — a fixed value (`SymbolKind::Constant`).
- `Table` — a lookup table (`SymbolKind::Table`), carrying axis/dimension info.
- `Function` — a user function/method (`SymbolKind::Function` / `Method`),
  carrying its execution rate when known.
- `Group` — a subsystem/container (`SymbolKind::Group`, and synthesized
  ancestors implied by dotted paths). Rendered as a Cytoscape compound node.

Each `GraphNode` carries its dotted `path`, an `id`, its `kind`, an optional
`rate_hz` (for functions/scheduled nodes), and optional structural metadata
(unit, declared type, table dimensions) for labels and tooltips.

## Rendering and export decision

- **Interactive HTML + Cytoscape.js.** The viewer is built on
  [Cytoscape.js](https://js.org/cytoscape) because it natively supports
  *compound nodes* (needed for the subsystem hierarchy and collapse/expand UX)
  and pluggable layered layouts (`dagre` / `elk`) that suit a data-flow DAG.
  The HTML file embeds the `GraphModel` as inline JSON plus the Cytoscape viewer
  so it is fully self-contained and offline. The Cytoscape library is vendored
  under `templates/cytoscape.min.js` and `include_str!`'d into the page, so no
  network access is ever needed.
- **Graphviz DOT export** for static rendering / pipelines (`dot -Tsvg`).
- **JSON graph export** (serde) — the canonical machine-readable form of the
  `GraphModel`, and the same payload embedded in the HTML.

## Architecture — mirrors m1-doc

The crate mirrors `m1-doc`'s `loader -> model -> renderers` shape:

```
Project (m1-typecheck)                    Trace / Counterfactual (m1-eval)
   │  loader.rs   (m1-typecheck / m1-core)     │  eval.rs   (m1-eval airlock)
   ▼                                           ▼
GraphModel (model.rs — toolchain-agnostic) ◄── Overlay  (model.rs; folded on via
   │            no toolchain type leaks past here          GraphModel::with_overlay)
   ├─ dot.rs   → Graphviz DOT
   ├─ json.rs  → JSON
   └─ html.rs  → self-contained interactive HTML (Cytoscape.js)
```

`loader.rs` is the only module that touches `m1-typecheck` / `m1-core` types,
and `eval.rs` is the symmetric airlock — the only module that touches `m1-eval`.
Both produce plain `model.rs` types (a `GraphModel`, an `Overlay`) that the
renderers consume; no toolchain type leaks past either. This matches `m1-doc`,
where `loader.rs` builds a `DocModel` that the markdown/html renderers consume.

## Dependencies

- **`m1-typecheck` (structure).** The symbol table (`Project::symbols()`)
  provides every node and most edges: kinds, dotted paths (hierarchy), table
  metadata (`table_meta`), and rates (`call_rate_hz` / `log_rate_hz`). Pinned to
  the same git tag as the rest of the toolchain (`v0.36.0`).
- **`m1-core`.** Pinned to `v0.12.0`. Used for CST parsing in the per-script
  data-flow read/write analysis (`dataflow::io_sets`, a self-contained port of
  the read/write summary logic from `m1-eval/src/summary.rs`, kept in-crate so
  the structural build takes no `m1-eval` dependency).
- **`m1-eval` (value + diff overlay).** Pinned to `v0.1.0`. The numeric
  value/diff overlay's only source of computed values; consumed exclusively by
  `eval.rs` (the airlock — see below), so no `m1-eval` type leaks past it. Its
  optional `ld` feature is re-exported as this crate's own `ld` feature
  (`ld = ["m1-eval/ld"]`); off by default, so a binary `.ld` overlay log fails
  loud through the engine (naming the feature) unless built with `--features ld`,
  while `.csv` logs always work. `m1-eval` pins the **same** `m1-typecheck
  v0.36.0` this crate's loader does, so a trace/diff channel path and a graph
  node id are the same canonical string — the join key (below) needs no fuzzy
  matching.

## Value + diff overlay

The overlay attaches `m1-eval`'s computed values to the structural graph. It is
**opt-in**: with no overlay flag the binary is byte-for-byte the structural v1
tool (same default HTML/JSON/DOT, same loader path), so back-compatibility is a
locked invariant. There are **two modes**:

1. **Value overlay** — run `m1-eval` against the project to get a `Trace`, then
   attach each channel/function node's per-tick value. Two sources:
   `--overlay-scenario <FILE>` (a `.toml`/`.json` scenario, run via
   `Engine::run`) and `--overlay-log <FILE>` with no override (a recorded `.csv`
   /`.ld` run, sampled onto its keyframe grid). The viewer colours/sizes nodes by
   value, ships a time scrubber over `overlay.time`, a value readout on node
   click, and a dashed-border distinction for externally-driven channels.
2. **Diff overlay** — `--overlay-log <FILE> --override "CH=value-or-expr"`
   (override repeatable) replays the log as ground truth, layers the overrides,
   recomputes only the downstream cone, and diffs against the log
   (`Engine::run_counterfactual_diff` → `Counterfactual { trace, diff }`). The
   viewer highlights the changed cone (reusing the structural cone-highlight UX),
   colours nodes by `max_abs_delta`, and steps the per-tick delta on the
   scrubber. The **no-op invariant** holds: with no override the changed set is
   empty, so nothing is highlighted.

`--at-time <SECONDS>` records the scrubber's initial tick (nearest index in
`overlay.time`); the full series is always embedded so the scrubber still works.
`--overlay-scenario` and `--overlay-log` are mutually exclusive, and `--override`
requires `--overlay-log` — both are fail-loud usage errors.

### The join key

A `GraphNode.id` is the symbol's dotted path (e.g. `Root.Demo.Output`);
`Trace::channels` and `Diff::channels` are keyed by the **same** canonical paths
the `m1-eval` runner emits, because both crates pin the same `m1-typecheck
v0.36.0` and load the same `Project`. So the join is a direct `node.id ==
channel path` lookup — **no fuzzy matching**. A trace/diff channel with no
matching node (a builtin or an expression column) is ignored; a node with no
trace column (an unscheduled function, an untouched constant) renders neutral.

### The overlay JSON shape

The overlay rides **inside the same `GraphModel` JSON** the v1 viewer already
embeds — one self-contained document, no second embedded payload, no network.
`model.rs` carries it as `GraphModel { …, overlay: Option<Overlay> }`, with
`#[serde(skip_serializing_if = "Option::is_none")]` so an un-overlaid model
serialises byte-identically to v1. The shape:

```jsonc
"overlay": {
  "kind": "value" | "diff",        // overlay mode
  "time": [0.0, 0.01, …],          // shared tick axis (seconds)
  "nodes": {                       // node-id -> per-node overlay (BTreeMap: sorted)
    "Root.Demo.Output": {
      "series": [{"num": 50.0}, …],// one ramp-aware cell per tick:
                                   //   {"num": f64} | {"bool": b} | {"str": s}
      "delta": [5.0, …],           // per-tick delta            (diff mode only)
      "max_abs_delta": 5.0         // colour-ramp summary       (diff mode only)
    }
  },
  "external": ["Root.Demo.Speed"], // externally-driven node ids (value mode)
  "changed": ["Root.Demo.Output"], // changed node ids (diff mode; empty in value)
  "eps": 1e-9,                      // diff threshold            (diff mode only)
  "start_tick": 0                  // scrubber's initial tick   (--at-time hint)
}
```

`OverlayCell` is tagged (`num`/`bool`/`str`) so the viewer knows which nodes are
numeric-rampable versus label-only, and shows a faithful readout for enums and
strings. The `nodes` map is a `BTreeMap` and `changed` is sorted, so the rendered
JSON/HTML/DOT are byte-deterministic across builds (no `HashMap` iteration leak).

## Deferred (later workflows)

The structural v1 and the value/diff overlay are both complete. Remaining seams:

- **Richer layered routing.** Vendoring the `dagre`/`elk` Cytoscape layout
  extensions the same way the core library is vendored under `templates/`; the
  shipped viewer uses only core Cytoscape layouts and adds no second vendored
  asset.
- **Per-expression overlay.** The overlay keys on channel/function **nodes**
  only; `m1-eval`'s per-expression `(script, byte_offset)` trace sink onto
  sub-node sites is a documented later seam.
- **In-browser re-runs.** The HTML is a static, self-contained snapshot; editing
  values or re-running the engine from the page is out of scope — a re-run is a
  CLI re-invocation.

## Notes

This document paraphrases M1 concepts in our own words; it contains no text
copied from any proprietary MoTeC manual.
