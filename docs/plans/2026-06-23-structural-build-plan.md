<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# m1-visualiser — structural build plan (scaffold → v1)

Status: planning only. Last updated 2026-06-23.

This plan takes `m1-visualiser` from its current structural scaffold to a
complete **structural-first v1**: a graph of channels / parameters / constants /
tables / functions / groups, four edge kinds (hierarchy, schedule, table-axis,
data-flow), and three renderers (interactive self-contained HTML, Graphviz DOT,
JSON). **No numeric value overlay** — that is a later workflow once `m1-eval`
Phase 3 lands and `m1-eval` becomes a dependency.

The plan is TDD and milestone-structured. Every milestone is a
**`cargo test`-greenable deliverable**: it ends with new/changed tests that pass
and the whole suite still green. Milestones are ordered so each builds on the
last with no forward references.

---

## Ground truth (verified against the real toolchain)

These are the **actual** signatures and shapes the build depends on — read out
of the pinned sources, not invented. Pins (already in `Cargo.toml`, matching
`m1-eval`): `m1-core` `v0.12.0`, `m1-typecheck` `v0.36.0`.

### `m1-typecheck` symbol model (`m1_typecheck::symbols`)

- `Project::load(&Path) -> Result<Project, project::LoadError>` — reads the
  `.m1prj` via `m1_workspace::read_motec_xml` (handles Windows-1252).
- `Project::from_xml(&str) -> Result<Project, LoadError>` — in-memory variant
  (used by the loader unit tests already in the scaffold).
- `Project::with_config(self, &Path) -> Result<Project, LoadError>` — augments
  the symbol table with `.m1cfg` table/parameter **shape** (this is what
  populates `Symbol::table_meta`). Consumes and returns `self`.
- `Project::symbols() -> &SymbolTable`; `SymbolTable::iter() -> impl Iterator<Item = &Symbol>`,
  `get(&str) -> Option<&Symbol>`, `immediate_children(&str) -> Vec<&Symbol>`,
  `function_path_for_filename(&str) -> Option<&str>`.
- `Project::group_for_script(file_name: &str) -> Option<String>` — enclosing
  group path for a script's bare filename (needed for data-flow canonicalisation).
- `Project::function_symbol_for_script(file_name: &str) -> Option<String>` —
  the Function/Method symbol path a `.m1scr` backs.
- `enum SymbolKind { Channel, Parameter, Constant, Function, Method, Table, Group, Reference, Object, Other }`.
- `struct Symbol` fields used here (all public): `path: String`,
  `kind: SymbolKind`, `value_type: ValueType`, `declared_type: Option<String>`,
  `unit: Option<String>`, `display_unit: Option<String>`, `security: Option<String>`,
  `filename: Option<String>`, `class/classname: Option<String>`,
  `call_rate_hz: Option<f64>`, `log_rate_hz: Option<f64>`, `tags: Vec<String>`,
  `table_meta: Option<TableMeta>`.
- `struct TableMeta { axes: Vec<TableAxis>, output_unit: Option<String> }`;
  `struct TableAxis { size: u32, unit: Option<String> }`. `axes.len()` is the
  table dimensionality; `output_unit` is the interpolated-output unit.
- `Project::LoadError` is `enum { Io(io::Error), Parse(String) }` (re-exported at
  `m1_typecheck::project::LoadError`).

### Parsed scripts (`m1_typecheck::parsed`)

- `struct ParsedScript { name: String, cst: m1_core::Cst }`.
- `parse_all(&[(String, String)]) -> Vec<ParsedScript>` — parses each
  `(file_name, source)` pair once. `cst.root() -> Node`, `cst.source() -> &str`,
  `cst.syntax_diagnostics()`.

### `m1-core` CST API (used by the data-flow walker)

- `m1_core::{Field, Kind, Node, Cst, parse, is_compound_assign}`.
- `Node`: `kind() -> Kind`, `text() -> &str`, `named_children() -> Vec<Node>`,
  `child_by_field(Field) -> Option<Node>`, `byte_range()`, `descendants()`.
- Kinds used: `LocalDeclaration`, `AssignmentStatement`, `Identifier`,
  `MemberExpression`, `CallExpression`.
- Fields used: `Name`, `Value`, `Target`, `Operator`, `Arguments`, `Object`,
  `Property`.
- `is_compound_assign(Kind) -> bool`.

### Name resolution (`m1_typecheck::resolve`)

- `struct Scope<'p> { locals: HashMap<String, ValueType>, group: Option<String>, project: Option<&'p Project>, fn_symbol: Option<String> }`.
- `resolve(path: &str, &Scope) -> Resolution` where
  `enum Resolution { Local(ValueType), Symbol(&Symbol), BuiltinObject(&'static str), BuiltinFn(Vec<&Overload>), Opaque, Unresolved }`.
- `resolve` already implements the M1 scope order (local → library → absolute →
  group-relative → `Parent` walk) and `Root.`-prefix canonicalisation, **but it
  does not expand `This`** — the caller must rewrite a leading `This` to the
  group path first.

### How data-flow edges are extracted (the porting plan)

`m1-eval` already implements exactly the read/write extraction we need, in
`m1-eval/src/summary.rs` (`pub fn io_sets(script: &ParsedScript, project: &Project, group: Option<&str>) -> IoSets`,
where `struct IoSets { writes: BTreeSet<String>, reads: BTreeSet<String> }`).
Both crates are ours and GPL-3.0-or-later, so copying our own code is fine.

The algorithm (verified in `summary.rs`):

1. Look up the backing function symbol via
   `project.function_symbol_for_script(&script.name)` (so `In.*` canonicalises).
2. Walk the CST. On `AssignmentStatement`: the `Target` is a **write**; if the
   `Operator` is a compound assign (`+=`, …), the target is **also a read**; the
   `Value` expression is walked for reads.
3. On `LocalDeclaration`: register the `Name` as a local (shadows project
   lookup) and walk its initialiser `Value` for reads.
4. Reads: an `Identifier` or a whole `MemberExpression` (flattened to a path) is
   resolved; only `Resolution::Symbol` paths are recorded. A `CallExpression`
   does **not** count its callee (`Calculate.Max`) as a read but **does** walk
   its `Arguments`.
5. Canonicalisation: flatten member chains to `A.B.C`, rewrite a leading `This`
   to the group path, then `classify(...)` → keep `Target::Symbol(path)`, drop
   `Local` / `Builtin` / `Unresolved`.

**Port shape.** `summary.rs` depends on three `m1-eval` internals:
`crate::ident::classify` (a thin wrapper over `m1_typecheck::resolve::resolve`),
and the `pub(crate)` helpers `crate::expr::flatten_member` and
`crate::expr::rewrite_this`. These are small and self-contained; we copy them
into `m1-visualiser` so the structural crate does **not** take an `m1-eval`
dependency (the design doc forbids that until the value-overlay workflow). The
port lives in a new `src/dataflow.rs` and exposes one function:
`io_sets(script, project, group) -> IoSets`. The only `m1-eval` type the
original referenced that we drop is `crate::value::Value`: `summary.rs` uses
`HashMap<String, Value>` only as a *set of local names* (the values are never
read), so in the port `locals` becomes `HashMap<String, ()>` /
`HashSet<String>` and `classify` builds its `Scope.locals` with
`ValueType::Unknown`, exactly as `ident.rs` does today.

Then in the loader: build a writer-aware edge set — for each script's `IoSets`,
emit `read_channel -> function` (DataFlow) for every read and
`function -> written_channel` (DataFlow) for every write. The `function` node is
the symbol path from `function_symbol_for_script`. This makes "what feeds this
channel" a pure upstream walk (the same orientation the HTML cone highlight and
`m1-eval`'s `build_cone` rely on).

### Renderer / asset facts

- Cytoscape.js v3.30.2 is **really vendored** at `templates/cytoscape.min.js`
  (373 KB, MIT). The HTML renderer `include_str!`s it and `viewer.html`, then
  substitutes `/*__CYTOSCAPE_JS__*/`, `/*__GRAPH_JSON__*/`, `__TITLE__`. No
  network is needed.
- `viewer.html` already wires compound `parent` nodes, per-kind colours, and a
  legend, and defaults to the `cose` layout. v1 adds: search, per-edge-kind
  filter toggles, click-to-highlight dependency cone, collapse/expand of
  compounds, and a layered layout — using **only** core Cytoscape + bundled
  extensions vendored the same way as the core lib (or hand-rolled in plain JS
  where an extension would add a second vendored asset).

---

## Current scaffold state (what already exists and passes)

- `model.rs` — `GraphModel`, `GraphNode`, `GraphEdge`, `NodeKind`, `EdgeKind`
  with serde + unit tests. **Complete; extend, don't rewrite.**
- `loader.rs` — nodes for every surfaced `SymbolKind`, ancestor-group
  synthesis, hierarchy edges, table-axis edges (containment-based), schedule
  edges (synthetic `Clock@<rate>Hz` nodes), `add_data_flow_edges` is a **no-op
  stub**. `load(project_path, title)` takes no config and discovers no scripts.
- `dot.rs`, `json.rs`, `html.rs` — working renderers with tests.
- `tests/smoke.rs` — fixture round-trip (already asserts the `Limiter` function
  node exists; the fixture `Limiter.m1scr` already reads Speed + MaxSpeed.Value
  and writes Limited, ready for data-flow once wired).
- CLI `main.rs` — `--project --out --format dot|json|html --title`. **Missing
  `--config`** (required by the task's CLI spec).

The scaffold compiles and its tests pass; this plan only **adds** behaviour and
keeps every existing test green (adjusting an existing test only where a
deliberate behaviour change requires it, called out per milestone).

---

## Milestones

Each milestone lists: goal, the test(s) that define "done" (write first), the
implementation, and the green bar.

### M1 — Loader ingests config + scripts (plumbing, no new edges yet)

**Goal.** Give the loader the inputs every later edge kind needs: the `.m1cfg`
(for `table_meta`) and the project's parsed `.m1scr` scripts (for data-flow),
without changing any emitted edges yet. This is the seam everything else hangs
off, so it lands first.

**Tests (write first).**
- `loader::tests`: `load_with_config_populates_table_meta` — load the existing
  scaffold-style XML plus a tiny `.m1cfg` declaring a 2-D table; assert the
  table node's `table_dims == Some(2)`.
- `loader::tests`: `collect_scripts_finds_m1scr` — point the loader at a temp
  dir with a `Project.m1prj` + one `.m1scr`; assert one script is discovered and
  parsed (name + non-empty source).
- Keep `smoke.rs` green by updating its single `loader::load(...)` call to the
  new signature (see below).

**Implementation.**
- Change `loader::load` to
  `load(project_path: &Path, config_path: Option<&Path>, title: Option<String>) -> Result<GraphModel, LoadError>`.
  Internally: `Project::load` → optional `.with_config(cfg)` → discover scripts
  by walking `project_path.parent()` recursively for `*.m1scr`
  (`collect_scripts`, ported near-verbatim from `m1-eval/src/loader.rs`,
  deterministic sort by basename, lossy UTF-8) → `parse_all`. Add a private
  `build_model_with_scripts(project, &[ParsedScript], title)` that `build_model`
  delegates to (so the existing `build_model` unit tests keep working with an
  empty script slice).
- The data-flow pass still no-ops; this milestone is pure plumbing.

**Green bar.** New loader tests pass; `smoke.rs` and all existing
`loader::tests` pass with the updated signature.

### M2 — Data-flow walker ported (`dataflow.rs`), unit-tested in isolation

**Goal.** Port `m1-eval`'s `io_sets` read/write extraction into
`src/dataflow.rs` with its own copies of `classify`, `flatten_member`,
`rewrite_this`, fully unit-tested against a fixture project — **before** wiring
it into the graph. This isolates the trickiest logic.

**Tests (write first)** — mirror `summary.rs`'s own tests (they are the spec):
- `assignment_target_is_write_rhs_idents_are_reads`
- `compound_assignment_target_is_both_read_and_write`
- `locals_are_not_dependencies`
- `builtin_callee_is_not_a_read_but_args_are`
- plus `this_anchor_rewrites_to_group` (read of `This.Speed` from group
  `Root.Demo` canonicalises to `Root.Demo.Speed`).
- Use a `mini` fixture project under `tests/fixtures/mini/` (port the shape of
  `m1-eval/tests/fixtures/mini`: a `Root.Demo` group with `Speed`, `Gain`,
  `Output` channels and a `FuncUser`), loaded via `Project::from_xml` in the
  unit test for speed.

**Implementation.**
- New module `src/dataflow.rs`:
  - `pub struct IoSets { pub writes: BTreeSet<String>, pub reads: BTreeSet<String> }`.
  - `pub fn io_sets(script: &ParsedScript, project: &Project, group: Option<&str>) -> IoSets`
    — the `Walker` from `summary.rs`, with `locals: HashSet<String>` instead of
    `HashMap<String, Value>` (values are never used).
  - Private `classify(name, group, fn_symbol, project, &locals) -> Target`
    (copied from `m1-eval/src/ident.rs`, `Value`-free) and a local `enum Target`.
  - Private `flatten_member(&Node) -> Option<String>` and
    `rewrite_this(&str, Option<&str>) -> Option<String>` (copied from
    `m1-eval/src/expr.rs`; the `Result`/`EvalError` plumbing collapses to
    `Option` since we only need the happy path).
- Register `pub mod dataflow;` in `lib.rs`. Keep it crate-internal in spirit
  (the loader is its only consumer) but `pub` so its tests and the loader see it.
- Add an SPDX header + a module doc note crediting the port from
  `m1-eval/src/summary.rs` (same project, GPL-3.0).

**Green bar.** All `dataflow::tests` pass; nothing else changes.

### M3 — Data-flow edges in the graph (`add_data_flow_edges` real)

**Goal.** Replace the loader's `add_data_flow_edges` no-op with real edges built
from `dataflow::io_sets`, oriented so reads point **into** functions and writes
point **out of** functions.

**Tests (write first).**
- `loader::tests`: `data_flow_edges_orient_reads_in_writes_out` — load the
  scaffold fixture (`Limiter` reads `Root.Engine.Speed` +
  `Root.Engine.MaxSpeed.Value`, writes `Root.Engine.Limited`); assert
  DataFlow edges `Root.Engine.Speed -> Root.Engine.Limiter`,
  `Root.Engine.MaxSpeed.Value -> Root.Engine.Limiter`, and
  `Root.Engine.Limiter -> Root.Engine.Limited` exist, all `EdgeKind::DataFlow`.
- `loader::tests`: `data_flow_edges_skip_unknown_endpoints` — a read/write whose
  canonical path is not a graph node (e.g. a builtin) produces no edge.
- Update `smoke.rs` to assert `model.edge_count(EdgeKind::DataFlow) > 0`.

**Implementation.**
- `add_data_flow_edges(project, scripts, model)`: for each script, resolve its
  group via `project.group_for_script(&script.name)`, compute
  `io_sets`, look up the function node id via
  `project.function_symbol_for_script(&script.name)`; for each `read` emit
  `read -> fn` and for each `write` emit `fn -> write`, **only when both
  endpoints exist in `node_ids`** (guard against builtins / external channels).
- Thread the `&[ParsedScript]` slice from `build_model_with_scripts` (M1).
- Determinism: edges are sorted/deduped by the existing
  `sort_for_determinism`.

**Green bar.** New loader tests + updated smoke test pass; DOT/JSON/HTML tests
unaffected (they assert structure, not counts).

### M4 — Table-axis edges grounded in `table_meta` + usage

**Goal.** Make table-axis edges reflect the real table model rather than only
naive path-containment. A table links to (a) the channels feeding its axes and
(b) the channel it outputs.

**Tests (write first).**
- `loader::tests`: `table_axis_links_members_and_marks_dims` — with a `.m1cfg`
  giving a 2-D `Demo.Map`, assert the table node has `table_dims == Some(2)` and
  `TableAxis` edges connect the table to its auto-created members
  (`Demo.Map.Value` output + axis breakpoint channels nested under it).
- `loader::tests`: `table_with_no_members_has_no_axis_edges` — a table whose
  members are absent emits no spurious edges (no panics, empty edge set for it).

**Implementation.**
- Keep the containment pass (it already links nested members) but:
  - drive table identification off `SymbolKind::Table` (unchanged), and
  - record `table_dims` from `table_meta.axes.len()` (already done in
    `build_node`) — assert it in tests now that `.m1cfg` is loaded.
  - Orient axis edges `table -> member` (table is the hub), matching the
    viewer's expectation. Confirm `.Value` output channel is included.
- This milestone mostly **locks behaviour with tests** now that `table_meta` is
  actually populated (it never was before M1 added config loading); only small
  orientation/guard fixes if a test reveals one.

**Green bar.** New table tests pass; existing edge tests green.

### M5 — Schedule edges + function rate grouping

**Goal.** Tighten the schedule model so scheduled **functions** are first-class
and grouped by rate (`call_rate_hz`), and channels group by `log_rate_hz`,
without changing the synthetic-clock approach that already works.

**Tests (write first).**
- `loader::tests`: `function_rate_drives_schedule_edge` — a `FuncUser` with a
  resolved `call_rate_hz` (XML `SelectedTrigger=…On 100Hz`) gets a
  `Clock@100Hz -> Root.…Func` Schedule edge and the clock node exists once.
- `loader::tests`: `two_nodes_same_rate_share_one_clock` — two rated nodes at
  100 Hz share a single `Clock@100Hz` node (dedup).
- `loader::tests`: `unrated_nodes_have_no_schedule_edge`.

**Implementation.**
- Largely already implemented; this milestone formalises it with tests and
  ensures a function node's `rate_hz` prefers `call_rate_hz` over `log_rate_hz`
  (current `build_node` already does `call_rate_hz.or(log_rate_hz)` — assert it).
- Verify clock-node ids are stable (`format_rate` strips trailing `.0`).

**Green bar.** New schedule tests pass; no regressions.

### M6 — CLI completion: `--config`, format wiring, exit codes

**Goal.** Complete the CLI to the task spec: `--project --config --out
--format dot|json|html`, threading the config into the loader.

**Tests (write first)** — `tests/cli.rs` (new, using `assert_cmd`):
- `cli_writes_html_by_default` — run with `--project <fixture>`; assert exit 0
  and an `m1-graph.html` written containing `cytoscape`.
- `cli_dot_and_json_formats` — `--format dot` / `--format json` write the
  matching extension and parse/round-trip.
- `cli_with_config_threads_table_meta` — `--project ... --config ...` produces a
  graph whose JSON contains a table node with `table_dims`.
- `cli_missing_project_exits_nonzero` — no project found → exit 2 with a message.

**Implementation.**
- Add `#[arg(long)] config: Option<PathBuf>` to `Args`; pass through to
  `loader::load(&project, config.as_deref(), title)`.
- Default `--config` discovery (optional, low-risk): if `--config` is omitted,
  look for a sibling `*.m1cfg` next to the project and use it if exactly one
  exists; otherwise none. (Keep it conservative — ambiguity ⇒ none, never guess.)
- Keep existing exit codes (2 = no project, 1 = load/write error).

**Green bar.** `tests/cli.rs` passes; existing smoke/unit tests green.

### M7 — DOT export polish: compound subsystems as clusters

**Goal.** Make the DOT export render subsystem nesting as boxes
(`subgraph cluster_*`) rather than only hierarchy edges, and keep per-kind
node/edge styling (already present).

**Tests (write first)** — `dot::tests`:
- `emits_cluster_for_group_nodes` — a model with `Root` / `Root.Engine` groups
  and a nested channel emits `subgraph "cluster_Root.Engine"` containing the
  channel, and the channel is **not** also emitted at top level.
- `data_flow_edges_render_solid` / per-kind style assertions (extend the
  existing style test) so all four `EdgeKind`s have a distinct
  `(style,color)` — already in `edge_style`, lock with a test.
- Keep `renders_digraph_with_nodes_and_edges` and the quoting test green.

**Implementation.**
- Build a group tree from node `parent` links; recursively emit nested
  `subgraph cluster_<id>` blocks (label = group leaf), placing each non-group
  node inside its nearest group cluster. Hierarchy edges become redundant inside
  clusters — keep emitting them but they read as nesting too; or suppress
  `Hierarchy` edges when both endpoints are in the same cluster (decide via the
  test — default: suppress to reduce clutter, documented in code).
- Leave `rankdir=LR` and per-kind styling intact.

**Green bar.** DOT tests pass; smoke `dot::render` assertions unchanged-compatible
(still `digraph m1 {` … `}` and mentions `Root.Engine.Speed`).

### M8 — Interactive HTML viewer: search + edge-kind filters + counts

**Goal.** Upgrade `viewer.html` (the only file the HTML renderer substitutes
into) with a node **search** box and per-**edge-kind filter toggles**, driven by
the embedded JSON — no extra vendored assets, plain JS only.

**Tests (write first)** — `html::tests` (string-level; the renderer is the unit
under test):
- `viewer_has_search_and_filter_controls` — rendered HTML contains the search
  input id and a filter checkbox per edge kind (`data_flow`, `table_axis`,
  `hierarchy`, `schedule`).
- `viewer_embeds_all_four_edge_kinds_legend` — legend still present (existing).
- Keep `embeds_cytoscape_and_graph_and_has_no_placeholders_left`,
  `embeds_node_ids_from_the_model`, `title_is_escaped` green.

**Implementation (in `templates/viewer.html`).**
- Toolbar: a `<input id="search">` that dims/undims nodes by label/id substring,
  and a checkbox per `EdgeKind` that shows/hides edges of that kind (toggle a
  Cytoscape class / `display`).
- Wire to the already-embedded `GRAPH`; no renderer-side Rust change beyond the
  string passing through (the test asserts the controls survive substitution).
- Verify no `</script>` sequence is introduced (the JSON path already avoids it).

**Green bar.** `html::tests` pass; HTML still self-contained (cytoscape inlined,
placeholders consumed).

### M9 — Interactive HTML viewer: layered layout + collapse/expand + cone highlight

**Goal.** Finish the interactive UX the task calls for: a layered
(dagre-style) layout, collapsible compound subsystems, and click-to-highlight a
node's dependency cone (upstream + downstream).

**Tests (write first)** — `html::tests` (string presence + a structural sanity
test, since we can't run a browser in `cargo test`):
- `viewer_has_collapse_and_cone_handlers` — rendered HTML references the
  collapse toggle control and the cone-highlight click handler (asserted by id /
  function name embedded in the script).
- `viewer_layout_is_layered` — the layout config names the layered layout we
  ship (e.g. `breadthfirst` from core Cytoscape, used as the offline layered
  fallback so we add **no** second vendored asset; document that dagre/elk can be
  vendored later for nicer routing).
- An `html_render` integration assertion in `smoke.rs`:
  `viewer_runs_cone_over_real_dataflow` — the rendered page embeds the
  `Root.Engine.Limiter` data-flow edges so the cone walk has data (structural
  check on the embedded JSON, not a DOM test).

**Implementation (in `templates/viewer.html`).**
- Layout: switch default to a layered layout that ships with core Cytoscape
  (`breadthfirst`, directed) for the DAG read; keep `cose` selectable. (Note in
  code: dagre/elk extensions can be vendored under `templates/` the same way as
  the core lib for production-grade routing — out of scope to add a second asset
  in v1.)
- Collapse/expand: a button + dblclick on a compound node toggles visibility of
  its descendants (plain JS over `node.descendants()` / restore), tracking
  collapsed state in a `Set`.
- Cone highlight: on node click/tap, BFS over edges to collect the **upstream**
  cone (follow edges *into* the node, transitively — "what feeds this") and the
  **downstream** cone (edges *out*); add highlight classes and dim the rest;
  click background to clear. The data-flow orientation from M3
  (read→fn→write) makes "what feeds this channel" a pure reverse-reachability
  walk. A legend/help line documents the colours.

**Green bar.** `html::tests` + smoke pass; output still fully offline
self-contained.

### M10 — End-to-end integration + determinism + docs sync

**Goal.** Lock the whole v1 behaviour end-to-end and update the design doc to
drop the "data-flow stubbed" caveat.

**Tests (write first)** — extend `tests/smoke.rs`:
- `full_v1_graph_has_all_four_edge_kinds` — load a fixture exercising every
  edge kind (table via `.m1cfg`, a rated function, a data-flow script) and
  assert `edge_count(kind) > 0` for **all four** kinds.
- `output_is_deterministic` — building the model twice yields byte-identical
  JSON (guards the sort/dedup).
- `html_dot_json_all_render_from_full_fixture` — all three renderers emit
  non-trivial output from the full fixture.

**Implementation.**
- Add/extend `tests/fixtures/` so one project drives all four kinds (extend the
  existing scaffold fixture: add a `BuiltIn.Table` + sibling `.m1cfg`, ensure
  the `FuncUser` has a `SelectedTrigger` rate, keep `Limiter.m1scr`).
- Remove the data-flow stub doc-comment in `loader.rs`; update
  `docs/specs/2026-06-23-m1-visualiser-design.md` "Stubs / TODO" to reflect that
  data-flow, search, filters, collapse, and cone highlight are **done** in v1
  (value overlay remains the only deferred item). (Doc edit, not code.)

**Green bar.** Entire `cargo test` suite green; `cargo build --release`
produces a working binary; a manual `--format html` open shows the interactive
graph (smoke-checked structurally in tests).

---

## Sequencing rationale

- **M1 → M3** front-load the data plumbing (config + scripts) and the hardest
  logic (the data-flow walker), isolating the port in M2 before it touches the
  graph in M3 — each is independently greenable.
- **M4 / M5** lock the two edge kinds that only become meaningful once config is
  loaded (table_meta) and rates are asserted.
- **M6** completes the CLI so the tool is usable end-to-end at the halfway mark.
- **M7 (DOT) and M8–M9 (HTML)** are renderer polish, independent of each other;
  DOT first because it is smaller and validates the group-tree shaping that the
  HTML compound/collapse work also relies on.
- **M10** is the integration + docs sweep.

## Non-goals (explicitly out of v1)

- Numeric value overlay (channel results, table lookups, parameter values) via
  `m1-eval` — a later workflow; `m1-eval` stays a non-dependency.
- Vendoring a second JS asset (dagre/elk): v1 uses core Cytoscape layouts only,
  with a documented seam to add dagre/elk later.
- Saved view state / persisted layouts.

## Test inventory (new or extended, by milestone)

| Milestone | New/changed tests |
|---|---|
| M1 | `load_with_config_populates_table_meta`, `collect_scripts_finds_m1scr`, smoke signature update |
| M2 | 5 `dataflow::tests` (mirror `summary.rs`) |
| M3 | `data_flow_edges_orient_reads_in_writes_out`, `data_flow_edges_skip_unknown_endpoints`, smoke DataFlow count |
| M4 | `table_axis_links_members_and_marks_dims`, `table_with_no_members_has_no_axis_edges` |
| M5 | `function_rate_drives_schedule_edge`, `two_nodes_same_rate_share_one_clock`, `unrated_nodes_have_no_schedule_edge` |
| M6 | `tests/cli.rs` (4 cases) |
| M7 | `emits_cluster_for_group_nodes`, per-kind edge-style lock |
| M8 | `viewer_has_search_and_filter_controls`, legend lock |
| M9 | `viewer_has_collapse_and_cone_handlers`, `viewer_layout_is_layered`, `viewer_runs_cone_over_real_dataflow` |
| M10 | `full_v1_graph_has_all_four_edge_kinds`, `output_is_deterministic`, all-renderers integration |
