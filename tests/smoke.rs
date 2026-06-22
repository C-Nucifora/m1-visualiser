// SPDX-License-Identifier: GPL-3.0-or-later
//! Smoke / integration test: build a [`GraphModel`] from a tiny synthetic
//! fixture project and assert that nodes/edges are produced and that the DOT,
//! JSON and HTML renderers all emit non-trivial output.

use m1_visualiser::model::EdgeKind;
use m1_visualiser::{dot, html, json, loader};
use std::path::PathBuf;

/// Path to the hand-authored synthetic fixture project shipped under
/// `tests/fixtures/`.
fn fixture_project() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/Project.m1prj")
}

#[test]
fn builds_nonempty_graph_from_fixture() {
    let model = loader::load(&fixture_project(), Some("Synthetic".into()))
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
}

#[test]
fn dot_and_json_render_from_fixture() {
    let model = loader::load(&fixture_project(), Some("Synthetic".into()))
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
    let model = loader::load(&fixture_project(), Some("Synthetic".into()))
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
