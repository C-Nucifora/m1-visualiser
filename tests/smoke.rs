// SPDX-License-Identifier: GPL-3.0-or-later
//! Smoke / integration test: build a [`GraphModel`] from a tiny synthetic
//! fixture project and assert that nodes/edges are produced and that the DOT,
//! JSON and HTML renderers all emit non-trivial output.

use m1_visualiser::eval::{self, ScenarioFormat};
use m1_visualiser::model::EdgeKind;
use m1_visualiser::{dot, html, json, loader};
use std::path::PathBuf;

/// Path to the hand-authored synthetic fixture project shipped under
/// `tests/fixtures/`.
fn fixture_project() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/Project.m1prj")
}

/// Path to the fixture's sibling `.m1cfg`, which supplies the `Boost` table's
/// 2-D shape (so the table node records `table_dims`). It lives next to the
/// project file and is the same config the CLI's sibling-discovery would pick.
fn fixture_config() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/parameters.m1cfg")
}

/// Load the full fixture exactly as the CLI does — project plus its sibling
/// `.m1cfg` — so all four edge kinds (hierarchy, data-flow, table-axis,
/// schedule) are exercised from a single project.
fn load_full() -> m1_visualiser::model::GraphModel {
    loader::load(
        &fixture_project(),
        Some(&fixture_config()),
        Some("Synthetic".into()),
    )
    .expect("full fixture project should load")
}

#[test]
fn builds_nonempty_graph_from_fixture() {
    let model = loader::load(&fixture_project(), None, Some("Synthetic".into()))
        .expect("fixture project should load");

    // Nodes and edges are produced.
    assert!(
        !model.nodes.is_empty(),
        "expected nodes, got {}",
        model.nodes.len()
    );
    assert!(
        !model.edges.is_empty(),
        "expected edges, got {}",
        model.edges.len()
    );

    // Hierarchy edges exist (containment from dotted paths).
    assert!(
        model.edge_count(EdgeKind::Hierarchy) > 0,
        "expected hierarchy edges; edges = {:?}",
        model.edges
    );

    // The function symbol surfaced as a node.
    assert!(
        model.nodes.iter().any(|n| n.id == "Root.Engine.Limiter"),
        "expected the Limiter function node; nodes = {:?}",
        model.nodes
    );

    // Data-flow edges are extracted from the Limiter script (reads Speed +
    // MaxSpeed.Value, writes Limited) — no longer a no-op stub.
    assert!(
        model.edge_count(EdgeKind::DataFlow) > 0,
        "expected data-flow edges; edges = {:?}",
        model.edges
    );
}

#[test]
fn dot_and_json_render_from_fixture() {
    let model = loader::load(&fixture_project(), None, Some("Synthetic".into()))
        .expect("fixture project should load");

    // DOT renders a well-formed digraph mentioning a fixture node.
    let dot = dot::render(&model);
    assert!(dot.starts_with("digraph m1 {"), "DOT header: {dot:.40}");
    assert!(
        dot.contains("Root.Engine.Speed"),
        "DOT should mention a node"
    );
    assert!(dot.trim_end().ends_with('}'), "DOT should be closed");

    // JSON renders and round-trips to a value with the right counts.
    let json_str = json::render(&model);
    let value: serde_json::Value = serde_json::from_str(&json_str).expect("JSON should parse");
    assert_eq!(
        value["nodes"].as_array().unwrap().len(),
        model.nodes.len(),
        "JSON node count should match model"
    );
    assert_eq!(
        value["edges"].as_array().unwrap().len(),
        model.edges.len(),
        "JSON edge count should match model"
    );
}

#[test]
fn html_render_is_self_contained() {
    let model = loader::load(&fixture_project(), None, Some("Synthetic".into()))
        .expect("fixture project should load");

    let page = html::render(&model);
    // Self-contained: Cytoscape inlined, no leftover placeholders, graph embedded.
    assert!(page.contains("<!DOCTYPE html>"));
    assert!(page.contains("cytoscape"), "Cytoscape.js should be inlined");
    assert!(!page.contains("/*__GRAPH_JSON__*/"), "no graph placeholder");
    assert!(
        !page.contains("/*__CYTOSCAPE_JS__*/"),
        "no cytoscape placeholder"
    );
    assert!(page.contains("Root.Engine.Speed"), "graph data embedded");
}

#[test]
fn viewer_runs_cone_over_real_dataflow() {
    // Structural check (no DOM): the rendered page must embed the Limiter
    // data-flow edges so the in-browser cone walk has real data to traverse.
    // The fixture's Limiter reads Speed + MaxSpeed.Value and writes Limited,
    // giving a cone with both an upstream and a downstream arm.
    let model = loader::load(&fixture_project(), None, Some("Synthetic".into()))
        .expect("fixture project should load");

    // Sanity: the model itself has the oriented data-flow edges (reads in,
    // writes out) the cone walk relies on.
    assert!(
        model.edges.iter().any(|e| e.kind == EdgeKind::DataFlow
            && e.from == "Root.Engine.Speed"
            && e.to == "Root.Engine.Limiter"),
        "expected upstream read edge Speed -> Limiter; edges = {:?}",
        model.edges
    );
    assert!(
        model.edges.iter().any(|e| e.kind == EdgeKind::DataFlow
            && e.from == "Root.Engine.Limiter"
            && e.to == "Root.Engine.Limited"),
        "expected downstream write edge Limiter -> Limited; edges = {:?}",
        model.edges
    );

    let page = html::render(&model);

    // The cone-highlight machinery is present and self-contained (offline).
    assert!(
        page.contains("highlightCone"),
        "viewer should ship the cone-highlight handler"
    );
    assert!(
        page.contains("incomers") && page.contains("outgoers"),
        "cone BFS should walk both upstream and downstream"
    );

    // The embedded JSON carries the Limiter data-flow edges so the BFS can run.
    assert!(
        page.contains("\"Root.Engine.Limiter\""),
        "embedded graph should name the Limiter function node"
    );
    assert!(
        page.contains("\"data_flow\""),
        "embedded graph should carry data-flow edges for the cone walk"
    );
    // Both arms of the cone are present in the embedded edge list.
    assert!(
        page.contains("\"Root.Engine.Speed\""),
        "embedded graph should carry the upstream read endpoint"
    );
    assert!(
        page.contains("\"Root.Engine.Limited\""),
        "embedded graph should carry the downstream write endpoint"
    );
}

// --- M10: end-to-end integration + determinism --------------------------------

#[test]
fn full_v1_graph_has_all_four_edge_kinds() {
    // A single project must drive every structural relationship the v1 graph
    // models. The full fixture combines: dotted-path nesting (Hierarchy), the
    // Limiter script's reads/writes (DataFlow), a `BuiltIn.Table` with members +
    // sibling `.m1cfg` (TableAxis), and a rated FuncUser via `SelectedTrigger`
    // (Schedule).
    let model = load_full();

    for kind in [
        EdgeKind::Hierarchy,
        EdgeKind::DataFlow,
        EdgeKind::TableAxis,
        EdgeKind::Schedule,
    ] {
        assert!(
            model.edge_count(kind) > 0,
            "expected at least one {kind:?} edge; edges = {:?}",
            model.edges
        );
    }
}

#[test]
fn output_is_deterministic() {
    // Building the model twice from the same inputs must yield byte-identical
    // JSON. This guards the loader's sort/dedup (`sort_for_determinism`) against
    // any reliance on filesystem enumeration or hash-map iteration order.
    let a = json::render(&load_full());
    let b = json::render(&load_full());
    assert_eq!(a, b, "model JSON must be byte-identical across builds");

    // The same must hold for the static and interactive renderers, which embed
    // the (already-sorted) model.
    let model = load_full();
    assert_eq!(
        dot::render(&model),
        dot::render(&model),
        "DOT must be stable"
    );
    assert_eq!(
        html::render(&model),
        html::render(&model),
        "HTML must be stable"
    );
}

#[test]
fn html_dot_json_all_render_from_full_fixture() {
    // All three renderers emit non-trivial output from the full fixture.
    let model = load_full();

    // DOT: a well-formed digraph mentioning real fixture content.
    let dot = dot::render(&model);
    assert!(dot.starts_with("digraph m1 {"), "DOT header: {dot:.40}");
    assert!(dot.trim_end().ends_with('}'), "DOT should be closed");
    assert!(
        dot.contains("Root.Engine.Speed"),
        "DOT should mention a fixture node"
    );

    // JSON: parses and round-trips to matching node/edge counts.
    let json_str = json::render(&model);
    let value: serde_json::Value = serde_json::from_str(&json_str).expect("JSON should parse");
    assert_eq!(
        value["nodes"].as_array().unwrap().len(),
        model.nodes.len(),
        "JSON node count should match model"
    );
    assert_eq!(
        value["edges"].as_array().unwrap().len(),
        model.edges.len(),
        "JSON edge count should match model"
    );
    // Every edge kind from the full fixture survives serialization.
    let edge_kinds: std::collections::BTreeSet<&str> = value["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    for tag in ["hierarchy", "data_flow", "table_axis", "schedule"] {
        assert!(
            edge_kinds.contains(tag),
            "JSON edges should carry a {tag} edge; got {edge_kinds:?}"
        );
    }

    // HTML: self-contained (Cytoscape inlined, no placeholders) and carries the
    // embedded graph data the viewer needs.
    let page = html::render(&model);
    assert!(page.contains("<!DOCTYPE html>"), "HTML doctype");
    assert!(page.contains("cytoscape"), "Cytoscape.js inlined");
    assert!(
        !page.contains("/*__GRAPH_JSON__*/") && !page.contains("/*__CYTOSCAPE_JS__*/"),
        "no leftover placeholders"
    );
    assert!(page.contains("Root.Engine.Speed"), "graph data embedded");
}

// --- O10: end-to-end overlay integration + determinism + back-compat ----------

/// The overlay fixture directory (`tests/fixtures/overlay/`): a `Root.Demo`
/// group with `Speed`/`Gain`/`Output` and an `Update` function computing
/// `Output = Speed * Gain`, plus a mutually-consistent scenario `.toml` and a
/// `.csv` log so an override moves a known cone.
fn overlay_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/overlay")
}

/// The overlay fixture's `.m1prj` and `.m1cfg` paths.
fn overlay_paths() -> (PathBuf, PathBuf) {
    let dir = overlay_dir();
    (dir.join("Project.m1prj"), dir.join("parameters.m1cfg"))
}

/// Build the overlay fixture's structural model exactly as the CLI does, so its
/// node ids are the canonical paths the trace/diff key by.
fn load_overlay_model() -> m1_visualiser::model::GraphModel {
    let (project, cfg) = overlay_paths();
    loader::load(&project, Some(&cfg), Some("Overlay".into()))
        .expect("overlay fixture project should load")
}

/// Read the GraphModel JSON embedded in a rendered HTML page (the renderer
/// substitutes it into the `var GRAPH = <json>;` literal). Mirrors the CLI
/// test's extractor so a smoke test can assert on the overlay the viewer reads.
fn embedded_graph_json(html: &str) -> serde_json::Value {
    let assign = html
        .split("\nvar GRAPH =")
        .nth(1)
        .expect("page embeds a GRAPH assignment");
    let literal = assign
        .split(';')
        .next()
        .expect("GRAPH assignment terminated");
    serde_json::from_str(literal.trim()).expect("embedded GRAPH JSON parses")
}

#[test]
fn value_overlay_round_trips_through_html() {
    // Load the overlay fixture, run the committed scenario through the real
    // engine (Speed=20, Gain=2.5 ⇒ Output=50), attach the VALUE overlay, render
    // HTML, and read it back out of the embedded GraphModel JSON. The page stays
    // self-contained (Cytoscape inlined, placeholders consumed, no network).
    let (project, cfg) = overlay_paths();
    let model = load_overlay_model();
    let scenario_src =
        std::fs::read_to_string(overlay_dir().join("scenario.toml")).expect("scenario fixture");

    let overlay = eval::run_value_scenario(
        &project,
        Some(&cfg),
        &scenario_src,
        ScenarioFormat::Toml,
        &model.nodes,
    )
    .expect("scenario run produces a value overlay");
    let model = model.with_overlay(overlay);

    let page = html::render(&model);

    // Self-contained: inlined Cytoscape, no leftover template placeholders.
    assert!(page.contains("<!DOCTYPE html>"), "HTML doctype");
    assert!(page.contains("cytoscape"), "Cytoscape.js inlined");
    assert!(
        !page.contains("/*__GRAPH_JSON__*/") && !page.contains("/*__CYTOSCAPE_JS__*/"),
        "no leftover placeholders"
    );

    // The embedded JSON carries a value overlay with the known Output value.
    let graph = embedded_graph_json(&page);
    let embedded = &graph["overlay"];
    assert_eq!(embedded["kind"], "value", "scenario yields a value overlay");
    let series = embedded["nodes"]["Root.Demo.Output"]["series"]
        .as_array()
        .expect("Output node carries a series");
    let last = series.last().expect("series is non-empty");
    assert_eq!(
        last["num"], 50.0,
        "Output's last cell is 20 * 2.5 = 50; {last}"
    );
}

#[test]
fn diff_overlay_round_trips_through_html() {
    // Log + override → DIFF overlay → HTML. Overriding the logged Speed (20 → 40)
    // moves its downstream cone (Output), so the embedded overlay is `diff` and
    // names the changed Output channel.
    let (project, cfg) = overlay_paths();
    let model = load_overlay_model();
    let log_path = overlay_dir().join("run.csv");
    let overrides = vec!["Root.Demo.Speed=40.0".to_string()];

    let overlay = eval::run_diff(&project, Some(&cfg), &log_path, &overrides, &model.nodes)
        .expect("override produces a diff overlay");
    let model = model.with_overlay(overlay);

    let page = html::render(&model);

    // Self-contained still holds for the diff page.
    assert!(page.contains("<!DOCTYPE html>"), "HTML doctype");
    assert!(
        !page.contains("/*__GRAPH_JSON__*/") && !page.contains("/*__CYTOSCAPE_JS__*/"),
        "no leftover placeholders"
    );

    let graph = embedded_graph_json(&page);
    let embedded = &graph["overlay"];
    assert_eq!(
        embedded["kind"], "diff",
        "an override yields a diff overlay"
    );
    let changed: Vec<&str> = embedded["changed"]
        .as_array()
        .expect("changed array")
        .iter()
        .map(|v| v.as_str().expect("changed id is a string"))
        .collect();
    assert!(
        changed.contains(&"Root.Demo.Output"),
        "the overridden cone must include Output; got {changed:?}"
    );
}

#[test]
fn overlay_output_is_deterministic() {
    // Building + rendering the same overlay twice must be byte-identical. The
    // overlay's `BTreeMap` node map and sorted `changed` ids make this hold; this
    // guards against any `HashMap` iteration-order leak in `eval.rs`. Exercised
    // for both modes (value via scenario, diff via override) and all three
    // renderers.
    let (project, cfg) = overlay_paths();
    let scenario_src =
        std::fs::read_to_string(overlay_dir().join("scenario.toml")).expect("scenario fixture");
    let log_path = overlay_dir().join("run.csv");
    let overrides = vec!["Root.Demo.Speed=40.0".to_string()];

    // Value mode: two independent build+render passes.
    let value_a = {
        let model = load_overlay_model();
        let overlay = eval::run_value_scenario(
            &project,
            Some(&cfg),
            &scenario_src,
            ScenarioFormat::Toml,
            &model.nodes,
        )
        .expect("value overlay (a)");
        html::render(&model.with_overlay(overlay))
    };
    let value_b = {
        let model = load_overlay_model();
        let overlay = eval::run_value_scenario(
            &project,
            Some(&cfg),
            &scenario_src,
            ScenarioFormat::Toml,
            &model.nodes,
        )
        .expect("value overlay (b)");
        html::render(&model.with_overlay(overlay))
    };
    assert_eq!(
        value_a, value_b,
        "value-overlay HTML must be byte-identical across builds"
    );

    // Diff mode: two independent build+render passes, across all renderers.
    let build_diff = || {
        let model = load_overlay_model();
        let overlay = eval::run_diff(&project, Some(&cfg), &log_path, &overrides, &model.nodes)
            .expect("diff overlay");
        model.with_overlay(overlay)
    };
    let diff_a = build_diff();
    let diff_b = build_diff();
    assert_eq!(
        json::render(&diff_a),
        json::render(&diff_b),
        "diff-overlay JSON must be byte-identical"
    );
    assert_eq!(
        dot::render(&diff_a),
        dot::render(&diff_b),
        "diff-overlay DOT must be byte-identical"
    );
    assert_eq!(
        html::render(&diff_a),
        html::render(&diff_b),
        "diff-overlay HTML must be byte-identical"
    );
}

#[test]
fn no_overlay_output_matches_v1() {
    // Back-compat proof at the integration level: an un-overlaid render of the
    // overlay fixture is byte-identical to the structural (v1) render across all
    // three renderers, and the HTML embeds no `"overlay"` key.
    let structural = load_overlay_model();
    // `with_overlay` is the only overlay seam; never calling it must leave the
    // model — and therefore every renderer's output — exactly as v1.
    assert!(
        structural.overlay.is_none(),
        "a freshly loaded model carries no overlay"
    );

    let json_a = json::render(&load_overlay_model());
    let json_b = json::render(&structural);
    assert_eq!(json_a, json_b, "structural JSON is stable");

    let page = html::render(&structural);
    let graph = embedded_graph_json(&page);
    assert!(
        graph.get("overlay").is_none(),
        "an un-overlaid page must embed no overlay; got {graph}"
    );

    // The renderers' output on an un-overlaid model is identical to a second
    // independently loaded copy — i.e. the v1 path is untouched.
    let reloaded = load_overlay_model();
    assert_eq!(
        dot::render(&reloaded),
        dot::render(&structural),
        "structural DOT is unchanged"
    );
    assert_eq!(
        html::render(&reloaded),
        page,
        "structural HTML is unchanged"
    );
}
