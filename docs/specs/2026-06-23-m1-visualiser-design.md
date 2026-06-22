<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# m1-visualiser design

Status: v1 scaffold (structural-first). Last updated 2026-06-23.

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

- **v1 — structural-first (this scaffold).** Build the graph purely from the
  project's static structure as reported by `m1-typecheck`'s symbol table. No
  numeric values. All four edge types below are in scope for v1, though
  data-flow edge extraction is stubbed in this scaffold (see *Stubs / TODO*).
- **Later — value overlay.** Overlay computed numeric values (channel results,
  table lookups, parameter values) onto nodes by consuming `m1-eval`. This is
  explicitly **not** part of the structural scaffold and adds `m1-eval` as a
  dependency only when that workflow lands.

## The four edge types

The graph models four distinct relationship kinds between nodes. Each is a
variant of `EdgeKind`:

1. **DataFlow** — a script reads one symbol and writes another (e.g. a function
   reads `Root.Engine.Speed` and writes `Root.Engine.Limited`). This is the
   read/write dependency between channels/parameters and the functions that
   produce them. Extracting these accurately requires per-script CST analysis of
   reads and writes; see *Stubs / TODO*.
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
  under `templates/` (or referenced via a documented placeholder when it cannot
  be fetched at scaffold time — see `templates/`).
- **Graphviz DOT export** for static rendering / pipelines (`dot -Tsvg`).
- **JSON graph export** (serde) — the canonical machine-readable form of the
  `GraphModel`, and the same payload embedded in the HTML.

## Architecture — mirrors m1-doc

The crate mirrors `m1-doc`'s `loader -> model -> renderers` shape:

```
Project (m1-typecheck)
   │  loader.rs   (all m1-typecheck / m1-core I/O lives here)
   ▼
GraphModel (model.rs — toolchain-agnostic; no m1-typecheck types leak past here)
   │
   ├─ dot.rs   → Graphviz DOT
   ├─ json.rs  → JSON
   └─ html.rs  → self-contained interactive HTML (Cytoscape.js)
```

`loader.rs` is the only module that touches `m1-typecheck` / `m1-core` types;
everything downstream reads the plain `GraphModel`. This matches `m1-doc`, where
`loader.rs` builds a `DocModel` that the markdown/html renderers consume.

## Dependencies

- **`m1-typecheck` (structure).** The symbol table (`Project::symbols()`)
  provides every node and most edges: kinds, dotted paths (hierarchy), table
  metadata (`table_meta`), and rates (`call_rate_hz` / `log_rate_hz`). Pinned to
  the same git tag as the rest of the toolchain (`v0.36.0`).
- **`m1-core`.** Pinned to `v0.12.0`. Used for CST parsing when per-script
  data-flow read/write analysis is implemented (the full version will reuse the
  read/write summary logic that `m1-eval` is building in Workflow 3).
- **`m1-eval` (value overlay, later only).** Not a dependency of this structural
  scaffold. Added when the numeric value-overlay workflow lands.

## Stubs / TODO (deferred to later workflows)

These are intentionally not finished in this scaffold and are marked with
`TODO(...)` in code:

- **Data-flow edge extraction.** Accurate `DataFlow` edges need per-script CST
  read/write analysis. The scaffold ships a minimal placeholder (no/conservative
  edges) with a clear TODO; the full implementation will reuse `m1-eval`'s
  read/write summary module (Workflow 3).
- **Value overlay.** Numeric values on nodes via `m1-eval` (later phase).
- **Interactive layout polish.** Layout tuning (dagre/elk parameters, edge
  routing, styling per kind) is minimal in the scaffold.
- **Collapse/expand UX.** Compound-node collapsing controls and saved view
  state are not yet wired up.

## Notes

This document paraphrases M1 concepts in our own words; it contains no text
copied from any proprietary MoTeC manual.
