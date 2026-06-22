// SPDX-License-Identifier: GPL-3.0-or-later
//! Self-contained interactive HTML export of a [`GraphModel`].
//!
//! Renders a single HTML document that embeds:
//!
//! 1. the `GraphModel` as inline JSON,
//! 2. the Cytoscape.js library inline (vendored under `templates/`), and
//! 3. a small viewer script that builds the interactive graph.
//!
//! The result opens in any browser with no server and no network access. The
//! HTML shell and the Cytoscape library are baked into the binary at compile
//! time via [`include_str!`], so the tool ships them itself.
//!
//! The viewer ships a node search box, per-edge-kind filter toggles, a layered
//! (breadthfirst, directed) default layout with selectable alternatives,
//! collapse/expand of compound (group) subsystems, and click-to-highlight of a
//! node's dependency cone (upstream + downstream) — all driven entirely by the
//! embedded graph JSON in plain JS over core Cytoscape (no extra vendored
//! asset). TODO(later): vendor the dagre/elk layout extensions the same way as
//! the core library for production-grade edge routing.

use crate::json;
use crate::model::GraphModel;

/// The HTML shell, with `/*__CYTOSCAPE_JS__*/`, `/*__GRAPH_JSON__*/` and
/// `__TITLE__` placeholders.
const VIEWER_HTML: &str = include_str!("../templates/viewer.html");

/// The graph-JSON placeholder *together with* the empty-graph fallback literal
/// that follows it in the template. Replacing this whole token with the real
/// JSON leaves a single valid object literal (`var GRAPH = {…};`); the template
/// on its own stays valid JS because the comment is whitespace and `GRAPH`
/// defaults to the empty fallback.
const GRAPH_JSON_TOKEN: &str = "/*__GRAPH_JSON__*/{\"nodes\":[],\"edges\":[]}";

/// The vendored Cytoscape.js library (MIT). Inlined so the output is offline and
/// self-contained.
const CYTOSCAPE_JS: &str = include_str!("../templates/cytoscape.min.js");

/// Render the model as a single self-contained interactive HTML document.
pub fn render(model: &GraphModel) -> String {
    let title = model.title.as_deref().unwrap_or("M1 Project");
    let graph_json = json::render_compact(model);

    // Substitute the three placeholders. Order matters only in that none of the
    // replacements introduce another placeholder.
    VIEWER_HTML
        .replace("/*__CYTOSCAPE_JS__*/", CYTOSCAPE_JS)
        .replace(GRAPH_JSON_TOKEN, &graph_json)
        .replace("__TITLE__", &html_escape_text(title))
}

/// Minimal text escaping for the title interpolated into element text / the
/// `<title>`. The graph JSON is *not* escaped this way — it is valid JS embedded
/// in a `<script>` and contains no `</script>` because node ids are dotted
/// component paths.
fn html_escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EdgeKind, GraphEdge, GraphNode, NodeKind};

    fn sample() -> GraphModel {
        let mut g = GraphModel::new(Some("Demo".into()));
        g.nodes.push(GraphNode::new("Root.Engine", NodeKind::Group));
        g.nodes
            .push(GraphNode::new("Root.Engine.Speed", NodeKind::Channel));
        g.edges.push(GraphEdge::new(
            "Root.Engine",
            "Root.Engine.Speed",
            EdgeKind::Hierarchy,
        ));
        g
    }

    #[test]
    fn embeds_cytoscape_and_graph_and_has_no_placeholders_left() {
        let html = render(&sample());
        // Placeholders all consumed.
        assert!(!html.contains("/*__CYTOSCAPE_JS__*/"));
        assert!(!html.contains("/*__GRAPH_JSON__*/"));
        assert!(!html.contains("__TITLE__"));
        // Cytoscape present (its UMD factory ends with a version string).
        assert!(html.contains("cytoscape"));
        assert!(html.contains("3.30.2"));
    }

    #[test]
    fn embeds_node_ids_from_the_model() {
        let html = render(&sample());
        assert!(html.contains("Root.Engine.Speed"));
        assert!(html.contains("hierarchy"));
    }

    #[test]
    fn title_is_escaped() {
        let mut g = GraphModel::new(Some("A<b>&c".into()));
        g.nodes.push(GraphNode::new("X", NodeKind::Group));
        let html = render(&g);
        assert!(html.contains("A&lt;b&gt;&amp;c"));
    }

    #[test]
    fn viewer_has_search_and_filter_controls() {
        let html = render(&sample());
        // A search input the viewer wires to dim/undim nodes by label/id.
        assert!(
            html.contains("id=\"search\""),
            "expected a search input with id=\"search\""
        );
        // One filter checkbox per edge kind, keyed by the EdgeKind tag so the
        // viewer can show/hide edges of that kind. Lock every tag from the model.
        for kind in [
            EdgeKind::DataFlow,
            EdgeKind::TableAxis,
            EdgeKind::Hierarchy,
            EdgeKind::Schedule,
        ] {
            let tag = kind.tag();
            let checkbox_id = format!("id=\"filter-{tag}\"");
            assert!(
                html.contains(&checkbox_id),
                "expected a filter checkbox with {checkbox_id} for edge kind {tag}"
            );
            // And it must be a checkbox input, not just a stray id.
            assert!(
                html.contains("type=\"checkbox\""),
                "expected checkbox inputs for the edge-kind filters"
            );
        }
    }

    #[test]
    fn embedded_graph_assignment_is_valid_js() {
        // Guard the GRAPH embedding: the renderer replaces the JSON placeholder
        // in place, and the template's `|| {…}` fallback must keep the assignment
        // a single valid expression. A regression here (two juxtaposed object
        // literals) would make the *whole* viewer script a syntax error, so none
        // of the interactive features would run despite string-presence tests.
        let html = render(&sample());
        // After substitution the GRAPH assignment is a *single* object literal:
        // the placeholder+fallback token is gone, replaced by the real JSON.
        // Line-anchored so an explanatory comment that mentions GRAPH cannot be
        // mistaken for the assignment itself.
        let assign = html
            .split("\nvar GRAPH =")
            .nth(1)
            .expect("a GRAPH assignment");
        let stmt = assign.split(';').next().expect("a terminated statement");
        assert!(
            stmt.contains("Root.Engine.Speed"),
            "expected the embedded JSON in the GRAPH assignment"
        );
        // No leftover placeholder and no dangling empty-graph fallback that would
        // turn the assignment into two juxtaposed object literals (a syntax error
        // that would break the entire viewer script).
        assert!(
            !stmt.contains("/*__GRAPH_JSON__*/"),
            "the graph placeholder must be consumed"
        );
        assert!(
            !stmt.contains("\"nodes\":[],\"edges\":[]"),
            "the empty-graph fallback must be consumed, not left dangling"
        );
        // Exactly one object literal: a single opening brace run, then a closing.
        // The statement starts with whitespace + `{` (the JSON object).
        assert!(
            stmt.trim_start().starts_with('{'),
            "GRAPH should be assigned a single object literal, got: {:.40}",
            stmt.trim_start()
        );
    }

    #[test]
    fn viewer_has_collapse_and_cone_handlers() {
        let html = render(&sample());
        // A collapse/expand toggle control: a toolbar button the viewer wires to
        // collapse compound (group) subsystems, plus a dblclick handler that does
        // the same on a single compound node.
        assert!(
            html.contains("id=\"toggle-collapse\""),
            "expected a collapse/expand toggle button with id=\"toggle-collapse\""
        );
        assert!(
            html.contains("dblclick"),
            "expected a dblclick handler so compound nodes collapse on double-click"
        );
        // Collapsed-state is tracked in a Set keyed by node id (authored name,
        // distinct from anything in the minified library).
        assert!(
            html.contains("collapsedNodes"),
            "expected a collapsedNodes set driving collapse/expand"
        );
        // The cone-highlight click handler: clicking a node highlights its
        // dependency cone (upstream + downstream) and dims the rest.
        assert!(
            html.contains("highlightCone"),
            "expected a highlightCone handler for click-to-highlight dependency cones"
        );
        // The BFS walks edges into the node (upstream — "what feeds this") and
        // edges out of it (downstream), so both directions must be referenced.
        assert!(
            html.contains("incomers") && html.contains("outgoers"),
            "expected the cone BFS to follow incomers (upstream) and outgoers (downstream)"
        );
        // Background-click clears the highlight.
        assert!(
            html.contains("clearCone"),
            "expected a clearCone handler wired to background clicks"
        );
    }

    #[test]
    fn viewer_layout_is_layered() {
        let html = render(&sample());
        // The default layout is the layered DAG view that ships with core
        // Cytoscape (breadthfirst, directed) — no second vendored asset. Anchor
        // on the exact authored layout config so we are asserting the viewer's
        // choice, not a `breadthfirst` substring that exists inside the minified
        // library. `cose` stays selectable via the authored layout option list.
        assert!(
            html.contains("name: \"breadthfirst\""),
            "expected the default layout config to name the layered `breadthfirst` layout"
        );
        assert!(
            html.contains("LAYOUTS"),
            "expected an authored LAYOUTS table offering selectable layouts"
        );
        assert!(
            html.contains("\"cose\""),
            "expected `cose` to remain a selectable layout option"
        );
    }

    #[test]
    fn viewer_embeds_all_four_edge_kinds_legend() {
        let html = render(&sample());
        // The legend (locked from M-prior) lists every edge kind by its CSS tag.
        for kind in [
            EdgeKind::DataFlow,
            EdgeKind::TableAxis,
            EdgeKind::Hierarchy,
            EdgeKind::Schedule,
        ] {
            let legend_class = format!("e-{}", kind.tag());
            assert!(
                html.contains(&legend_class),
                "expected the legend to mention {legend_class}"
            );
        }
    }
}
