# m1-visualiser

Interactive structural graph/visualiser for MoTeC M1 Build projects.

`m1-visualiser` turns a loaded M1 project into a graph an engineer can *see* —
how channels, parameters, constants, tables, and functions relate — rather than
reading it out of XML. The primary output is a single **self-contained
interactive HTML file** (vendored offline Cytoscape; no server, no network);
Graphviz **DOT** and a **JSON** graph are also produced for pipelines, diffing,
and other renderers.

It is a sibling tool to [`m1-doc`](https://github.com/C-Nucifora/m1-doc)
(project documentation) and [`m1-eval`](https://github.com/C-Nucifora/m1-eval)
(the evaluator), built on the same toolchain
([`m1-core`](https://github.com/C-Nucifora/m1-core) /
[`m1-typecheck`](https://github.com/C-Nucifora/m1-typecheck)).

## Install

Prebuilt binaries for Linux/macOS/Windows are attached to each
[release](../../releases) (with `SHA256SUMS` and GitHub build provenance), or
build from source:

```sh
cargo install --git https://github.com/C-Nucifora/m1-visualiser --tag vX.Y.Z
```

## Quick start

```sh
# From anywhere inside a project (nearest Project.m1prj upward, or $M1_PROJECT):
m1-visualiser                          # writes m1-graph.html
m1-visualiser --format dot             # Graphviz DOT
m1-visualiser --format json            # JSON graph
m1-visualiser --project path/to/Project.m1prj --out graph.html
```

The structural graph models four edge kinds: **data-flow** (a function reads
one symbol, writes another), **table-axis** (a table's input axes and output
`.Value`), **hierarchy** (group containment — collapsible compound nodes in the
HTML viewer), and **schedule** (which event clock triggers which function).

## Value and diff overlays (experimental)

Numeric overlays run the project through `m1-eval` and paint computed values
(or counterfactual diffs) onto the graph:

```sh
m1-visualiser --overlay-scenario run.toml            # values from a scenario
m1-visualiser --overlay-log run.csv                  # values from a recorded log
m1-visualiser --overlay-log run.csv \
  --override "Root.Engine.Sensor=95.0"               # diff of the override's cone
m1-visualiser --overlay-log run.ld ...               # .ld needs --features ld
```

`--at-time SECONDS` opens the viewer's scrubber at a given tick (default: the
last tick).

**Numeric overlays inherit the evaluator's limitations and should be treated as
experimental.** `m1-eval` models the project's *schedule*, not the ECU itself
(no execution budgets, stalls, preemption, or watchdogs), and its offline
whole-project runs are only as good as the scenario driving them — see
[m1-eval's README](https://github.com/C-Nucifora/m1-eval#readme) for the
fail-loud contract and the `allow_default_inputs` opt-in. Do not use overlay
numbers for calibration, safety, or behavioural validation. The structural
graph carries no such caveat — it reflects the project as parsed.

## Scope and limitations

- **Read-only.** The visualiser never modifies a project. Structured edits
  belong to [`m1-project`](https://github.com/nedlane/m1-project).
- **Structure from the symbol table.** Nodes/edges come from `m1-typecheck`'s
  symbol model plus per-script CST analysis; anything that model cannot resolve
  (e.g. `$(…)`-parameterised triggers) is absent from the graph rather than
  guessed.
- **Self-contained HTML.** The viewer embeds a vendored Cytoscape and escapes
  all project-derived strings; it makes no network requests.

## Build and test

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

## License

GPL-3.0 — see [LICENSE](LICENSE).

## Trademark

Independent, community-built open-source tooling for the MoTeC® M1 script
language. Not affiliated with, authorised, or endorsed by MoTeC Pty Ltd.
"MoTeC" and "M1" are trademarks of MoTeC Pty Ltd.
