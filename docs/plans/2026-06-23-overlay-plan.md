<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# m1-visualiser — value + counterfactual-diff overlay plan

Status: planning only. Last updated 2026-06-23.

This plan takes `m1-visualiser` from its complete **structural v1** (GraphModel,
loader, DOT/JSON/HTML renderers, the Cytoscape viewer with search / per-edge-kind
filters / collapse-expand / dependency-cone highlight) to a tool that can
**overlay computed values** onto that graph by consuming `m1-eval`. This is the
"value overlay" workflow the design doc explicitly deferred — adding `m1-eval` as
a dependency only now, when the numeric workflow lands.

Two overlay modes, both opt-in (the structural graph stays the default):

1. **VALUE overlay** — run `m1-eval` against the project (a scenario, the whole
   project, or a CSV/`.ld` log) to get a [`Trace`]; attach to each channel /
   function node the value at a chosen tick (last tick, or value-at-time-T). The
   HTML viewer colours / sizes nodes by value, ships a **time scrubber** that
   steps through `trace.time` and re-colours, a **value readout** on node click,
   and visually distinguishes **externally-driven** channels.
2. **DIFF overlay** (the headline join) — run an `m1-eval` counterfactual (load a
   log + apply an override) to get a [`Counterfactual { trace, diff }`]; highlight
   the changed channels (the downstream cone), colour nodes by `max_abs_delta`,
   and let the user see exactly which nodes the override moved. Reuses the
   existing cone-highlight UX as the visual vocabulary.

The plan is TDD and milestone-structured. Every milestone is a
**`cargo test`-greenable deliverable**: it ends with new/changed tests that pass
and the whole suite still green. Milestones are ordered so each builds on the
last with no forward references. **Back-compatibility is a hard constraint**: with
no overlay flag the binary behaves exactly as v1 (same default HTML, same JSON,
same DOT, same `loader::load` path), and every existing test stays green.

---

## Ground truth (verified against the pinned `m1-eval` source)

These are the **actual** `m1-eval` types and signatures the overlay depends on,
read out of `m1-eval/src/{engine,trace,diff,scenario,log,value}.rs` — not
invented. Pin: `m1-eval = { git = "https://github.com/C-Nucifora/m1-eval.git",
tag = "v0.1.0" }`.

### The one entry point — `m1_eval::Engine` (`engine.rs`)

`Engine` is the public facade; it leaks **no** `m1-core`/`m1-typecheck` type in
any signature (mirrors `m1-doc`'s boundary discipline). All overlay I/O goes
through it:

- `Engine::load(project: &Path, cfg: Option<&Path>) -> Result<Engine, EvalError>`
  — load the `.m1prj` (+ optional `.m1cfg`). Same two paths the visualiser's own
  `loader::load` already takes.
- `Engine::run(&self, scenario: &Scenario) -> Result<Trace, EvalError>` — evaluate
  a scenario (single-function / cone / whole-project per the scenario's mode),
  producing a [`Trace`]. **The VALUE-overlay source for scenario + whole-project.**
- `Engine::load_log(&mut self, path: &Path) -> Result<(), EvalError>` — attach a
  recorded run as ground truth. Dispatches on extension: `.csv` (always available),
  `.ld` (behind the `ld` cargo feature; fails loud without it). Stored as
  `Option<Log>`.
- `Engine::override_channel(&mut self, spec: &str) -> Result<(), EvalError>` —
  register a `CH=value-or-expression` override (accumulates).
- `Engine::run_counterfactual_diff(&self) -> Result<Counterfactual, EvalError>` —
  **the DIFF-overlay source.** Replays the log as ground truth, layers the
  overrides, recomputes only the downstream cone, and diffs against the log.
  Requires a log (fails loud otherwise).
- `Engine::run_counterfactual(&self) -> Result<Trace, EvalError>` — the bare trace
  (no diff); we use `run_counterfactual_diff` so the diff comes for free.

There is **no** `Engine::run` that takes a `Log` directly. The **VALUE-from-log**
source is therefore: `load_log` then `run_counterfactual_diff` with *no overrides*
— the trace is the log replayed onto the project's tick grid (the identity
counterfactual), and `diff.changed_channels()` is empty by construction. (We could
alternatively expose the log itself; but routing through the engine keeps a single
trace shape and gives the computed downstream channels too, not just logged ones.)

### `Trace` (`trace.rs`)

```rust
pub struct Trace {
    pub time: Vec<f64>,                              // shared tick axis, seconds
    pub channels: BTreeMap<String, Vec<Value>>,      // path -> column aligned to time
    pub exprs: BTreeMap<(String, usize), Vec<Value>>,// per-expression sink (sparse)
    pub external: BTreeSet<String>,                  // externally-driven channel paths
}
```

- Columns are aligned to `time`; `channels[path][i]` is the value at `time[i]`.
- `Trace::to_json()` → `{"time":[…],"channels":{path:[…]},"external":[…]}` — but we
  do **not** reuse it; the overlay needs its values keyed by graph-node id and
  carried *inside the GraphModel JSON*, so the renderer embeds one document, not two.
- `external` is the set the VALUE overlay renders with a distinct visual (dashed
  border) — "this column is simulated input, not evaluated output".

### `Value` (`value.rs`)

```rust
pub enum Value { Bool(bool), Int(i64), Uint(u64), Float(f64), Enum{id,member}, Str(String) }
```

- `Value::as_f64(&self) -> Result<f64, EvalError>` — numeric coercion (`Int`/`Uint`/
  `Float`); non-numeric (`Bool`/`Enum`/`Str`) is a `TypeError`. The overlay uses
  `as_f64().ok()` to decide whether a node is **colour-by-magnitude** (numeric) or
  **label-only** (carries a display string but no colour ramp).
- For the JSON we attach, we render each value to a small tagged form
  (`{"num": 50.0}` / `{"bool": true}` / `{"str": "Idle"}`) so the viewer can show a
  faithful readout *and* know whether a numeric ramp applies — `Trace::to_json`'s
  bare-scalar form loses the bool/enum/string distinction, so we render our own.

### `Diff` / `Counterfactual` (`diff.rs`)

```rust
pub struct Counterfactual { pub trace: Trace, pub diff: Diff }
pub struct Diff { pub time: Vec<f64>, pub channels: BTreeMap<String, ChannelDiff>, pub eps: f64 }
pub struct ChannelDiff {
    pub logged: Vec<f64>, pub counterfactual: Vec<f64>, pub delta: Vec<f64>,
    pub max_abs_delta: f64, pub changed: bool,
}
```

- `Diff::changed_channels(&self) -> Vec<&str>` — the channel paths the override
  moved (sorted). **Empty for a no-op override** — the load-bearing invariant the
  DIFF overlay must preserve (no override ⇒ no highlighted nodes).
- A `ChannelDiff` exists only for channels present in BOTH trace and log as a
  numeric column (non-numeric / log-absent channels are skipped). So the DIFF
  overlay colours a subset of nodes; the rest render neutral.
- Channel **keys are the trace's canonical paths** (e.g. `Root.CF.Mid`). The log
  matcher already canonicalises (`Root.`-stripped / leaf-name) internally, so the
  diff's keys line up with **GraphModel node ids** without us re-canonicalising.

### `Scenario` (`scenario.rs`)

- `Scenario::from_toml_str(&str) -> Result<Scenario, EvalError>` (primary).
- `Scenario::from_json_str(&str) -> Result<Scenario, EvalError>` (same shape).
- Modes: `mode = "function" | "cone" | "whole-project"`. `function`/`cone` need a
  `target`; `whole-project` needs none and may omit `base_rate_hz`.
- The visualiser reads a scenario **file** (`.toml`/`.json`) from a CLI flag and
  hands the string to the matching constructor; it never builds a `Scenario` by
  hand.

### Node-id ↔ channel-path alignment (the join key)

The `GraphModel` node `id` is the symbol's dotted path (`Root.Engine.Speed`), and
`Trace::channels` / `Diff::channels` are keyed by the same canonical paths the
`m1-eval` runner emits (it loads the *same* project the visualiser does, via the
same `Project` symbol table). **So the join is a direct `node.id == channel_path`
lookup** — no fuzzy matching in the visualiser. Where a trace channel has no
matching node (a builtin or an expression column) it is simply ignored; where a
node has no trace column (an unscheduled function, a constant the run never
touched) it renders neutral/un-valued. Both crates pin the **same**
`m1-typecheck v0.36.0`, so the path canonicalisation is identical on both sides.

---

## Current state (what exists and passes — do not rewrite)

- `model.rs` — `GraphModel { title, nodes, edges }`, `GraphNode { id, path, kind,
  rate_hz, unit, type_label, table_dims, parent }`, `GraphEdge`, `NodeKind`,
  `EdgeKind`, serde derives + tests. **Extend additively** (new optional fields
  `#[serde(skip_serializing_if = "Option::is_none")]` so existing JSON is byte-
  identical when no overlay is attached).
- `loader.rs` — builds the structural `GraphModel` from `Project` + scripts. The
  overlay does **not** touch the loader's symbol→graph translation; it *augments*
  an already-built model with values keyed by node id.
- `dot.rs`, `json.rs`, `html.rs` — renderers. `html::render(&GraphModel)` embeds
  `json::render_compact(model)` into `templates/viewer.html` by substituting
  `/*__GRAPH_JSON__*/{…}` / `/*__CYTOSCAPE_JS__*/` / `__TITLE__`. The overlay rides
  in **inside that same GraphModel JSON** — no second embedded document, no network.
- `templates/viewer.html` — Cytoscape viewer; already has the cone-highlight
  machinery (`highlightCone`, `walkCone`, `clearCone`, `cone-up`/`cone-down`/
  `cone-dimmed` classes) the DIFF overlay reuses, plus `NODE_COLOR` by kind.
- `main.rs` — `--project --config --out --format dot|json|html --title`. Overlay
  adds flags; defaults unchanged.

---

## Design: where overlay data lives

A new **toolchain-agnostic** overlay type hangs off the model — `m1-eval` types do
**not** leak past a single new `eval.rs` boundary module (mirroring how `loader.rs`
is the only module touching `m1-typecheck`). Concretely:

- `model.rs` gains an `Overlay` carried optionally on `GraphModel`:

  ```rust
  pub struct GraphModel { …, pub overlay: Option<Overlay> }   // skip_serializing_if None

  pub struct Overlay {
      pub kind: OverlayKind,                 // Value | Diff (serde snake_case)
      pub time: Vec<f64>,                    // the trace/diff tick axis
      pub nodes: BTreeMap<String, NodeOverlay>, // node-id -> per-node overlay
      pub external: BTreeSet<String>,        // externally-driven node ids (Value mode)
      pub changed: Vec<String>,              // changed node ids (Diff mode; empty in Value)
      pub eps: Option<f64>,                  // the diff threshold (Diff mode only)
  }

  pub struct NodeOverlay {
      pub series: Vec<OverlayCell>,          // one cell per tick, aligned to Overlay.time
      pub delta: Option<Vec<f64>>,           // per-tick delta (Diff mode)
      pub max_abs_delta: Option<f64>,        // Diff mode summary for the colour ramp
  }

  pub enum OverlayCell { Num(f64), Bool(bool), Str(String) }  // faithful, ramp-aware
  ```

  All `Serialize`/`Deserialize` with `skip_serializing_if` on the `Option`s so an
  un-overlaid model serialises **identically** to today's.

- `eval.rs` (new, the **only** module that depends on `m1-eval`) builds an
  `Overlay` from a `Trace` (Value mode) or a `Counterfactual` (Diff mode), keying
  by node id. It exposes:

  ```rust
  pub fn value_overlay_from_trace(trace: &Trace, nodes: &[GraphNode]) -> Overlay;
  pub fn diff_overlay(cf: &Counterfactual, nodes: &[GraphNode]) -> Overlay;
  // plus the run helpers that produce the Trace/Counterfactual via Engine (below).
  ```

  It maps `Value -> OverlayCell` via a small `value_cell(&Value)` (numeric →
  `Num`, bool → `Bool`, everything else → `Str(display)`), and only emits a
  `NodeOverlay` for node ids that appear in the trace/diff (others stay neutral).

This keeps the boundary clean: `model.rs`/`json.rs`/`dot.rs`/`html.rs` see only the
plain `Overlay`; `eval.rs` is the airlock for `m1-eval`.

---

## Milestones

Each milestone lists: goal, the test(s) that define "done" (write first), the
implementation, and the green bar.

### O1 — Dependency + overlay model types (no behaviour change)

**Goal.** Add the `m1-eval` dependency and the toolchain-agnostic `Overlay` model
types, with serde that is **invisible when absent** — an un-overlaid `GraphModel`
serialises byte-identically to v1.

**Tests (write first).**
- `model::tests`: `model_without_overlay_serialises_unchanged` — a `GraphModel`
  with `overlay: None` produces JSON containing no `"overlay"` key (locks the
  `skip_serializing_if`, so every existing JSON/HTML/DOT test stays valid).
- `model::tests`: `overlay_round_trips_through_json` — build an `Overlay` with a
  couple of `NodeOverlay`s (one numeric series, one string cell), serialise +
  deserialise, assert equality.
- `model::tests`: `overlay_cell_serialises_tagged` — `OverlayCell::Num(50.0)` /
  `Bool(true)` / `Str("Idle")` each round-trip and carry their tag (so the viewer
  can tell numeric-rampable from label-only).

**Implementation.**
- `Cargo.toml`: add `m1-eval = { git = "…/m1-eval.git", tag = "v0.1.0" }`. (No
  `ld` feature by default; an opt-in `[features] ld = ["m1-eval/ld"]` is added in
  O6 when `.ld` logs are wired — until then `.ld` fails loud through the engine.)
- `model.rs`: add `Overlay`, `OverlayKind`, `NodeOverlay`, `OverlayCell`, and the
  `overlay: Option<Overlay>` field with `#[serde(skip_serializing_if = "Option::is_none", default)]`.
  Pure data; no `m1-eval` import here (the model stays toolchain-agnostic).

**Green bar.** New model tests pass; **every** existing test (loader, json, dot,
html, smoke, cli) stays green because the model JSON is unchanged when overlay is
absent.

### O2 — `eval.rs` airlock: build a VALUE overlay from a `Trace`

**Goal.** Translate an `m1-eval` `Trace` into an `Overlay` keyed by graph-node id,
in the single module allowed to see `m1-eval`. Pure mapping, unit-tested with a
hand-built `Trace` — no engine run yet, so the trickiest join logic is isolated.

**Tests (write first).**
- `eval::tests`: `value_overlay_keys_by_node_id` — a `Trace` with `channels`
  `Root.Demo.Output -> [Float(50), Float(50)]` and a node list containing that id
  yields an `Overlay { kind: Value }` whose `nodes["Root.Demo.Output"].series ==
  [Num(50.0), Num(50.0)]` and whose `time` equals the trace's.
- `eval::tests`: `trace_channels_without_a_node_are_dropped` — a trace channel
  with no matching graph node produces no `NodeOverlay` (no spurious keys).
- `eval::tests`: `external_channels_are_flagged` — a trace channel in
  `trace.external` lands in `overlay.external` (and only if it is also a node).
- `eval::tests`: `non_numeric_values_become_string_cells` — an enum/bool column
  maps to `OverlayCell::Bool`/`Str`, not `Num` (so the viewer label-only-renders it).

**Implementation.**
- `eval.rs`: `pub fn value_overlay_from_trace(trace: &Trace, nodes: &[GraphNode]) -> Overlay`.
  Build a `BTreeSet` of node ids; for each `trace.channels` entry whose path is a
  node id, map the column to `Vec<OverlayCell>` via `value_cell`; collect
  `trace.external ∩ node_ids` into `overlay.external`; set `kind = Value`,
  `changed = []`, `eps = None`.
- `value_cell(&Value) -> OverlayCell`: `Float/Int/Uint -> Num(as_f64)`,
  `Bool -> Bool`, `Enum/Str -> Str(display)`.
- Register `pub mod eval;` in `lib.rs`. Module doc credits the `m1-eval` boundary
  and states it is the only `m1-eval`-touching module.

**Green bar.** `eval::tests` pass; nothing else changes (no renderer or CLI wiring
yet).

### O3 — `eval.rs`: build a DIFF overlay from a `Counterfactual`

**Goal.** Translate a `Counterfactual { trace, diff }` into an `Overlay { kind:
Diff }` carrying per-node `delta` / `max_abs_delta` and the `changed` node-id set,
keyed by node id. Preserve the **no-op ⇒ no change** invariant.

**Tests (write first).**
- `eval::tests`: `diff_overlay_marks_changed_nodes` — a `Counterfactual` whose
  `diff.changed_channels()` is `["Root.CF.Mid"]` yields `overlay.changed ==
  ["Root.CF.Mid"]`, `overlay.kind == Diff`, and `nodes["Root.CF.Mid"].max_abs_delta
  == Some(>0)`.
- `eval::tests`: `diff_overlay_carries_per_tick_delta` — `nodes[ch].delta` equals
  the `ChannelDiff.delta` for that channel, aligned to `overlay.time`.
- `eval::tests`: `noop_diff_has_no_changed_nodes` — a diff with no changed channels
  yields `overlay.changed.is_empty()` (the load-bearing invariant).
- `eval::tests`: `diff_overlay_series_is_the_counterfactual_trace` —
  `nodes[ch].series` is the counterfactual trace column (so the scrubber still
  reads values, while `delta`/`changed` drive the highlight/ramp).

**Implementation.**
- `eval.rs`: `pub fn diff_overlay(cf: &Counterfactual, nodes: &[GraphNode]) -> Overlay`.
  Start from `value_overlay_from_trace(&cf.trace, nodes)` (so `series` + `external`
  are populated), then set `kind = Diff`; for each `cf.diff.channels` entry whose
  path is a node id, attach `delta = Some(d.delta.clone())` and `max_abs_delta =
  Some(d.max_abs_delta)`; set `overlay.changed = cf.diff.changed_channels()
  filtered to node ids`, and `overlay.eps = Some(cf.diff.eps)`.

**Green bar.** `eval::tests` pass; structural path untouched.

### O4 — `eval.rs`: run helpers (Engine plumbing) + run-source selection

**Goal.** Wire the actual `Engine` runs behind the airlock: produce a VALUE
overlay from a scenario / whole-project / log, and a DIFF overlay from a log +
overrides. This is where `m1-eval`'s `Engine` is driven; everything above consumed
hand-built traces.

**Tests (write first)** — use a small fixture project under
`tests/fixtures/overlay/` (a `Root.Demo` group with `Speed`/`Gain`/`Output`
channels and an `Update` `FuncUser`, mirroring `m1-eval`'s `mini` fixture so a
scenario produces a known column), loaded via the real `Engine`:
- `eval::tests`: `scenario_run_produces_value_overlay` — load the fixture, run a
  `function` scenario (`Output = Speed*Gain`, Speed=20, Gain=2.5), build a VALUE
  overlay, assert `nodes["Root.Demo.Output"]`'s last cell is `Num(50.0)`.
- `eval::tests`: `log_replay_produces_value_overlay` — write a tiny `time`-first
  CSV log, `load_log` + `run_counterfactual_diff` with **no overrides**, build a
  VALUE overlay from `cf.trace`, assert a logged channel's series is present and
  `cf.diff.changed_channels()` is empty.
- `eval::tests`: `override_produces_diff_overlay` — `load_log` + `override_channel`
  + `run_counterfactual_diff`, build a DIFF overlay, assert the overridden cone is
  in `overlay.changed`.
- `eval::tests`: `missing_log_for_diff_fails_loud` — a Diff request with no log
  surfaces the engine's `EvalError` (we propagate, never swallow).

**Implementation.**
- `eval.rs` adds a small run API the CLI calls (returns `Result<Overlay, EvalError>`):
  - `run_value_scenario(project, cfg, scenario_src, format) -> Overlay` —
    `Engine::load` → `Scenario::from_toml_str`/`from_json_str` → `engine.run` →
    `value_overlay_from_trace`.
  - `run_value_log(project, cfg, log_path) -> Overlay` — `Engine::load` →
    `engine.load_log` → `engine.run_counterfactual_diff` (no overrides) →
    `value_overlay_from_trace(&cf.trace, …)`.
  - `run_diff(project, cfg, log_path, overrides: &[String]) -> Overlay` —
    `Engine::load` → `load_log` → `override_channel` per spec →
    `run_counterfactual_diff` → `diff_overlay`.
  - The node list each helper needs is the just-built structural `GraphModel`'s
    `nodes` (the CLI builds the model first, then asks `eval.rs` to overlay it).
- All three return `Result<Overlay, EvalError>`; `EvalError` is re-exported from
  `eval.rs` so `main.rs` matches on it without importing `m1-eval` directly
  (keeps `main` overlay-agnostic except via this module).

**Green bar.** `eval::tests` pass against the real engine; structural tests green.

### O5 — `attach`: fold an `Overlay` onto a `GraphModel` (the join)

**Goal.** A tiny, well-tested seam that takes a built structural `GraphModel` and
an `Overlay` and produces the overlaid model the renderers embed — keeping
`eval.rs` (which knows `m1-eval`) separate from the act of mutating the model.

**Tests (write first).**
- `model::tests` (or `eval::tests`): `attach_sets_overlay_and_preserves_structure`
  — attaching an overlay leaves `nodes`/`edges` byte-identical and sets
  `model.overlay = Some(_)`.
- `model::tests`: `attach_is_idempotent_replacement` — attaching twice replaces,
  never appends (last overlay wins).

**Implementation.**
- `GraphModel::with_overlay(self, overlay: Overlay) -> GraphModel` (or
  `attach_overlay(&mut self, Overlay)`), pure model code (no `m1-eval`). The CLI:
  `let model = loader::load(...)?; let overlay = eval::run_*(...)?; let model =
  model.with_overlay(overlay);`.

**Green bar.** New tests pass; structural renderers still render an un-overlaid
model unchanged.

### O6 — CLI: opt-in overlay flags, back-compatible (`.ld` feature seam)

**Goal.** Extend `main.rs` with overlay flags that are **off by default**; with
none of them the binary is byte-for-byte the v1 tool. Add the optional `ld`
feature so `.ld` logs work when built with it (fails loud otherwise, via the
engine).

**Tests (write first)** — `tests/cli.rs` (extend, `assert_cmd`):
- `cli_without_overlay_is_unchanged` — `--project <fixture>` with no overlay flag
  writes the same HTML the v1 path does (no `"overlay"` in the embedded JSON).
- `cli_overlay_scenario_embeds_values` — `--overlay-scenario <scn.toml>` writes
  HTML whose embedded JSON contains `"overlay"` with `"kind":"value"` and a node
  series.
- `cli_overlay_log_embeds_values` — `--overlay-log <log.csv>` produces a value
  overlay (identity counterfactual).
- `cli_overlay_diff_marks_changed` — `--overlay-log <log.csv> --override
  "Root.CF.Sensor=100"` produces `"kind":"diff"` with a non-empty `changed` set.
- `cli_at_time_selects_tick` — `--overlay-log … --at-time 0.0` records the chosen
  default scrubber tick in the overlay JSON (a `start_tick`/`at_time` hint the
  viewer opens on; the full series is always embedded so the scrubber still works).
- `cli_overlay_requires_log_for_override` — `--override` without `--overlay-log`
  exits non-zero with a clear message (don't silently produce a value overlay).
- `cli_ld_log_without_feature_fails_loud` — `--overlay-log run.ld` built without
  `--features ld` exits non-zero naming the feature (propagates the engine error).

**Implementation (in `main.rs`).**
- New args (all `Option`, default `None`):
  - `--overlay-scenario <FILE>` (`.toml`/`.json`) → VALUE overlay via `run_value_scenario`.
  - `--overlay-log <FILE>` (`.csv`/`.ld`) → VALUE overlay via `run_value_log`,
    unless `--override` is present, in which case → DIFF via `run_diff`.
  - `--override <SPEC>` (repeatable; `CH=value-or-expr`) → switches `--overlay-log`
    into DIFF mode; requires `--overlay-log` (fail loud otherwise).
  - `--at-time <SECONDS>` → the scrubber's initial tick (nearest `trace.time` index);
    recorded in the overlay JSON as a start hint. Default: last tick (the design's
    "last-tick value").
- Mutually-exclusive guard: `--overlay-scenario` xor `--overlay-log` (a clap
  `ArgGroup` or a manual check); both set ⇒ fail loud.
- Wiring: build the structural model as today; if an overlay flag is set, call the
  matching `eval::run_*`, then `model.with_overlay(overlay)`. Render exactly as
  before. The overlay is embedded for **all** formats (JSON carries it for
  pipelines; HTML for the viewer; DOT ignores it — see O7/O8).
- `Cargo.toml`: `[features] ld = ["m1-eval/ld"]` so `--features ld` enables `.ld`.
- Exit codes: keep 2 = no project, 1 = load/write/eval error (an `EvalError` from a
  run prints `m1-visualiser: <project>: <err>` and exits 1, same shape as a load
  error).

**Green bar.** New `cli.rs` cases pass; `cli_without_overlay_is_unchanged` proves
back-compat; existing CLI tests green.

### O7 — JSON/DOT carry the overlay (machine-readable; DOT degrades gracefully)

**Goal.** The JSON export carries the overlay verbatim (it is the canonical
machine-readable form and the HTML payload); DOT stays valid and **optionally**
annotates node labels with the at-time value, but never breaks when overlay is
absent.

**Tests (write first).**
- `json::tests`: `overlay_is_present_in_json_when_attached` — an overlaid model's
  pretty JSON contains `"overlay"` with the node series; an un-overlaid model's
  does not (re-asserts O1's invariant at the renderer).
- `dot::tests`: `dot_renders_without_overlay_unchanged` — DOT output for an
  overlaid model still starts `digraph m1 {`, ends `}`, and (decision, locked by
  test) appends the at-time numeric value to a node's label when an overlay is
  present, e.g. `Output\n50`. Un-overlaid DOT is byte-identical to v1.

**Implementation.**
- `json.rs`: no change needed beyond O1's serde — the overlay rides in the model.
  Add the test to lock it.
- `dot.rs`: when `model.overlay` is `Some`, look up each node's at-time cell (the
  start-tick / last-tick `OverlayCell`) and, if `Num`, append it to the node label.
  Guarded so absent overlay ⇒ unchanged output.

**Green bar.** JSON/DOT tests pass; structural DOT/JSON tests green.

### O8 — HTML viewer: VALUE overlay (colour/size by value, scrubber, readout, externals)

**Goal.** Teach `templates/viewer.html` to render a VALUE overlay: colour and size
nodes by their at-time numeric value, a **time scrubber** that steps through
`overlay.time` and re-colours, a **value readout** on node click, and a distinct
visual for **externally-driven** channels — all from the embedded JSON, no network,
no second vendored asset. Opt-in: with no `overlay` the page is the v1 structural
viewer.

**Tests (write first)** — `html::tests` (string-level; the renderer + template are
the unit under test, since `cargo test` can't drive a browser):
- `html::tests`: `value_overlay_embeds_overlay_json` — rendering an overlaid model
  embeds `"overlay"` and `"kind":"value"` in the page's GRAPH literal.
- `html::tests`: `viewer_has_time_scrubber_when_overlay_present` — the page ships
  a `<input type="range" id="scrubber">` and a `applyOverlayAtTick` function wired
  to it (asserted by id/name presence).
- `html::tests`: `viewer_has_value_readout` — a `#readout` element and a click
  handler that writes the focused node's value into it.
- `html::tests`: `viewer_distinguishes_external_nodes` — the viewer references an
  `external` class / `overlay.external` so externally-driven channels get the
  dashed-border treatment.
- `html::tests`: `viewer_value_ramp_present` — a `valueColor(v)` ramp function and
  a size-by-value rule exist (so numeric nodes colour/size by magnitude).
- Keep all existing `html::tests` (`embeds_cytoscape_and_graph_…`,
  `viewer_has_search_and_filter_controls`, `viewer_has_collapse_and_cone_handlers`,
  `viewer_layout_is_layered`, legend) green — the scrubber/readout toolbar is
  **additive** and the whole overlay block is JS-guarded by `if (GRAPH.overlay)`.

**Implementation (in `templates/viewer.html`).**
- Toolbar (only meaningful when `GRAPH.overlay` exists; hidden otherwise via JS):
  a `<input type="range" id="scrubber" min=0 max=time.length-1>`, a time label, and
  a `<span id="readout">` value display.
- `applyOverlayAtTick(i)`: for each node with an `overlay.nodes[id]`, read
  `series[i]`; if `Num`, set fill from a `valueColor(v)` ramp (min→max across that
  tick or a fixed perceptual ramp) and scale `width`/`height` by magnitude; if
  `Bool`/`Str`, keep the kind colour and show the label string. Default tick =
  the overlay's start hint (`--at-time`) or the last tick.
- Externally-driven nodes (`overlay.external`) get a dashed border + a small "ext"
  badge so simulated inputs read distinctly from evaluated outputs.
- Node click: in addition to the existing `highlightCone`, write
  `id = value (unit)` into `#readout`.
- Everything is inside `if (GRAPH.overlay && GRAPH.overlay.kind === "value")`, so
  the structural page is untouched when no overlay is embedded.

**Green bar.** New + existing `html::tests` pass; page still self-contained
(Cytoscape inlined, placeholders consumed); structural smoke tests green.

### O9 — HTML viewer: DIFF overlay (highlight changed cone, colour by max-abs-delta)

**Goal.** Render the DIFF overlay by **reusing the existing cone-highlight UX** as
the visual vocabulary: the changed channels (the override's downstream cone) are
highlighted, nodes are coloured by `max_abs_delta`, and the user sees exactly which
nodes the override moved. Scrubber steps the per-tick `delta`.

**Tests (write first)** — `html::tests`:
- `html::tests`: `diff_overlay_embeds_diff_json` — an overlaid (diff) model embeds
  `"kind":"diff"` and the `changed` node-id list.
- `html::tests`: `viewer_highlights_changed_nodes` — the viewer references
  `overlay.changed` and a `markChanged`/`applyDiff` function that adds a highlight
  class to those nodes (reusing the cone-highlight classes / a new `changed` class).
- `html::tests`: `viewer_diff_ramp_by_max_abs_delta` — a `deltaColor(d)` ramp keyed
  on `max_abs_delta` exists (nodes coloured by how much they moved).
- `html::tests`: `viewer_diff_default_focus_is_changed_cone` — on load, a diff
  overlay auto-highlights the changed set (so the headline answer — "what did this
  override move" — is visible without a click).
- Keep all prior `html::tests` green (the diff block is `if (overlay.kind ===
  "diff")`-guarded; value-mode and structural pages unaffected).

**Implementation (in `templates/viewer.html`).**
- `applyDiff()`: add a `changed` class to nodes in `overlay.changed`; colour each
  by `deltaColor(node.max_abs_delta)` (a perceptual ramp, neutral at 0); dim the
  unchanged remainder using the existing `cone-dimmed` style so the moved cone
  reads instantly. Reuse `cone-down`/`changed` styling so the visual language
  matches the structural cone highlight the user already knows.
- The scrubber (O8) in diff mode steps `overlay.nodes[id].delta[i]` so the user can
  watch the delta evolve over `overlay.time`.
- A readout line shows `id: logged → counterfactual (Δ delta)` for the focused
  node (the per-tick values come from `series` + `delta`).
- Auto-run `applyDiff()` once on load (the changed cone is the headline), while the
  existing click-to-`highlightCone` still works for ad-hoc exploration.

**Green bar.** New + existing `html::tests` pass; page self-contained; structural +
value-overlay paths unaffected.

### O10 — End-to-end integration, determinism, docs sync

**Goal.** Lock the whole overlay workflow end-to-end through the CLI and the
renderers, prove determinism and back-compat, and update the design doc to record
that the value-overlay workflow has landed.

**Tests (write first)** — extend `tests/smoke.rs` / `tests/cli.rs`:
- `smoke`: `value_overlay_round_trips_through_html` — load a fixture, run a scenario
  via `eval::run_value_scenario`, attach, render HTML; assert the embedded JSON has
  the overlay and a known node value, and the page is self-contained.
- `smoke`: `diff_overlay_round_trips_through_html` — log + override → diff overlay →
  HTML; assert `changed` is embedded and the changed node id is present.
- `smoke`: `overlay_output_is_deterministic` — building + rendering the same overlay
  twice yields byte-identical HTML/JSON (BTreeMap ordering + sorted node ids make
  this hold; guards against any HashMap iteration leak in `eval.rs`).
- `smoke`: `no_overlay_output_matches_v1` — the un-overlaid render is byte-identical
  to the structural render (back-compat proof at the integration level).

**Implementation.**
- Add the `tests/fixtures/overlay/` project + a sample scenario `.toml` + a sample
  `.csv` log (mutually-consistent, mirroring `m1-eval`'s `counterfactual` fixture so
  an override has a known cone).
- Update `docs/specs/2026-06-23-m1-visualiser-design.md`: move "Value overlay" from
  **Deferred** to a shipped capability; document the two modes, the `m1-eval`
  dependency (tag `v0.1.0`) and the `ld` feature, and the overlay JSON shape. Note
  the join key (`node.id == channel path`, same `m1-typecheck v0.36.0` both sides).
  (Doc edit, not code.)

**Green bar.** Entire `cargo test` suite green (`--features ld` too, where the env
has it); `cargo build --release` produces a working binary; a manual `--overlay-log
… --override …` open shows the moved cone (smoke-checked structurally in tests).

---

## Sequencing rationale

- **O1** adds the dependency and the model types with *zero* behaviour change —
  the back-compat foundation everything else preserves.
- **O2 → O3** isolate the two pure `Trace`/`Counterfactual` → `Overlay` mappings
  with hand-built inputs, so the join logic (node-id keying, value classification,
  the no-op invariant) is tested before any engine run touches it.
- **O4** drives the real `Engine`; by then the mapping it feeds is already proven.
- **O5** is the trivial model-side join seam, kept separate so `m1-eval` never
  leaks past `eval.rs`.
- **O6** wires the CLI opt-in flags and the `ld` feature, with back-compat as an
  explicit test.
- **O7 (JSON/DOT) and O8–O9 (HTML)** are renderer work; JSON/DOT first because
  they are smaller and the HTML viewer embeds the same JSON. O8 (value) precedes
  O9 (diff) because the diff viewer reuses the value scrubber/readout.
- **O10** is the integration + determinism + docs sweep.

## Non-goals (explicitly out of this overlay workflow)

- Editing values in the browser / re-running the engine from the page (the HTML is
  a static, self-contained snapshot; re-runs are a CLI re-invocation).
- A live/streaming trace or animation export (the scrubber steps embedded ticks
  only).
- Vendoring a second JS asset (dagre/elk): unchanged from v1 — core Cytoscape only.
- Per-expression (`trace.exprs`) overlay onto sub-node sites: v1 overlay keys on
  channel/function **nodes** only; the `(script, byte_offset)` expr sink is a
  documented later seam.

## Key API calls (the exact `m1-eval` surface this plan uses)

| Need | Call |
|---|---|
| Load project (+cfg) | `Engine::load(project: &Path, cfg: Option<&Path>) -> Result<Engine, EvalError>` |
| VALUE (scenario / whole-project) | `Scenario::from_toml_str` / `from_json_str` → `engine.run(&scenario) -> Result<Trace, EvalError>` |
| Attach log | `engine.load_log(path: &Path) -> Result<(), EvalError>` (`.csv` always; `.ld` behind `ld` feature) |
| VALUE (from log) | `engine.run_counterfactual_diff() -> Result<Counterfactual, EvalError>` with no overrides; use `cf.trace` |
| Register override | `engine.override_channel(spec: &str) -> Result<(), EvalError>` (`CH=value-or-expr`, repeatable) |
| DIFF (headline) | `engine.run_counterfactual_diff() -> Result<Counterfactual { trace, diff }, EvalError>` |
| Changed cone | `cf.diff.changed_channels() -> Vec<&str>` (sorted; **empty for no-op**) |
| Per-channel delta | `cf.diff.channels[path]: ChannelDiff { delta, max_abs_delta, changed, logged, counterfactual }` |
| Value → cell | `Value::as_f64() -> Result<f64, _>` (numeric vs. label-only) |

## How values attach to the GraphModel (the join, in one paragraph)

`GraphModel` node `id` is the symbol's dotted path; `Trace::channels` and
`Diff::channels` are keyed by the **same** canonical paths (both crates pin
`m1-typecheck v0.36.0` and load the same `Project`), so the join is a direct
`node.id == channel_path` map — no fuzzy matching. `eval.rs` (the only module that
imports `m1-eval`) builds a toolchain-agnostic `Overlay { kind, time, nodes:
BTreeMap<node_id, NodeOverlay { series: Vec<OverlayCell>, delta, max_abs_delta },
external, changed, eps }` from a `Trace` (Value) or `Counterfactual` (Diff),
emitting a `NodeOverlay` only for trace/diff channels that are graph nodes.
`GraphModel::with_overlay` folds it onto the model as `overlay: Option<Overlay>`
(`skip_serializing_if = "None"`, so an un-overlaid model serialises identically to
v1). `json.rs` carries it verbatim; `html.rs` embeds the same model JSON the v1
viewer already embeds — so the page stays self-contained (no network), and the
viewer's scrubber/readout/highlight read straight out of `GRAPH.overlay`.
