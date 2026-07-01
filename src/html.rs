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
    // Escape the three script-context-significant characters to their JSON
    // \uXXXX unicode escapes. In valid JSON these characters only ever appear
    // inside string values, so the escapes stay valid JSON/JS while ensuring a
    // string field containing `</script>` (e.g. a channel/annotation name)
    // cannot terminate the inline `<script>` element and inject markup into the
    // exported HTML.
    let graph_json = json::render_compact(model)
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026");

    // Substitute the three placeholders. Order matters only in that none of the
    // replacements introduce another placeholder.
    VIEWER_HTML
        .replace("/*__CYTOSCAPE_JS__*/", CYTOSCAPE_JS)
        .replace(GRAPH_JSON_TOKEN, &graph_json)
        .replace("__TITLE__", &html_escape_text(title))
}

/// Minimal text escaping for the title interpolated into element text / the
/// `<title>`. The graph JSON is escaped separately in [`render`]: its
/// script-significant characters (`<`, `>`, `&`) are rewritten to JSON `\uXXXX`
/// unicode escapes so a string field containing `</script>` cannot close the
/// inline `<script>` element early or inject markup.
fn html_escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        EdgeKind, GraphEdge, GraphNode, NodeKind, NodeOverlay, Overlay, OverlayCell, OverlayKind,
    };
    use std::collections::{BTreeMap, BTreeSet};

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

    /// A small VALUE-overlaid model: one numeric node (`Speed`) carrying a
    /// two-tick series, plus an externally-driven `Output` node, so the
    /// value-overlay viewer tests have a concrete page to assert against.
    fn value_overlaid_sample() -> GraphModel {
        let mut g = sample();
        g.nodes
            .push(GraphNode::new("Root.Engine.Output", NodeKind::Channel));

        let mut nodes = BTreeMap::new();
        nodes.insert(
            "Root.Engine.Speed".to_string(),
            NodeOverlay {
                series: vec![OverlayCell::Num(10.0), OverlayCell::Num(50.0)],
                delta: None,
                max_abs_delta: None,
            },
        );
        nodes.insert(
            "Root.Engine.Output".to_string(),
            NodeOverlay {
                series: vec![OverlayCell::Num(20.0), OverlayCell::Num(20.0)],
                delta: None,
                max_abs_delta: None,
            },
        );
        let overlay = Overlay {
            kind: OverlayKind::Value,
            time: vec![0.0, 0.01],
            nodes,
            external: ["Root.Engine.Output".to_string()].into_iter().collect(),
            changed: Vec::new(),
            eps: None,
            start_tick: Some(1),
        };
        g.with_overlay(overlay)
    }

    /// A small DIFF-overlaid model: one changed node (`Speed`) carrying a
    /// counterfactual `series`, a per-tick `delta`, and a `max_abs_delta`, plus an
    /// unchanged `Output` node — so the diff-overlay viewer tests have a concrete
    /// page whose changed cone is `["Root.Engine.Speed"]`.
    fn diff_overlaid_sample() -> GraphModel {
        let mut g = sample();
        g.nodes
            .push(GraphNode::new("Root.Engine.Output", NodeKind::Channel));

        let mut nodes = BTreeMap::new();
        nodes.insert(
            "Root.Engine.Speed".to_string(),
            NodeOverlay {
                series: vec![OverlayCell::Num(10.0), OverlayCell::Num(50.0)],
                delta: Some(vec![0.0, 30.0]),
                max_abs_delta: Some(30.0),
            },
        );
        nodes.insert(
            "Root.Engine.Output".to_string(),
            NodeOverlay {
                series: vec![OverlayCell::Num(20.0), OverlayCell::Num(20.0)],
                delta: Some(vec![0.0, 0.0]),
                max_abs_delta: Some(0.0),
            },
        );
        let overlay = Overlay {
            kind: OverlayKind::Diff,
            time: vec![0.0, 0.01],
            nodes,
            external: BTreeSet::new(),
            changed: vec!["Root.Engine.Speed".to_string()],
            eps: Some(1e-9),
            start_tick: Some(1),
        };
        g.with_overlay(overlay)
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
    fn embedded_json_escapes_script_close() {
        // A node id (a string field that rides verbatim into the embedded GRAPH
        // JSON) containing a `</script>` sequence must not be able to terminate
        // the inline `<script>` element early and inject markup into the exported
        // HTML. The renderer HTML-escapes the script-significant characters to
        // their JSON \uXXXX unicode escapes, which stay valid JSON/JS while never
        // closing the script tag.
        let mut g = GraphModel::new(Some("Demo".into()));
        g.nodes.push(GraphNode::new(
            "Root.</script><img src=x onerror=alert(1)>",
            NodeKind::Channel,
        ));
        let html = render(&g);
        // The raw injection payload must not appear (which would break out of the
        // <script> and inject an <img> into the document).
        assert!(
            !html.contains("</script><img"),
            "raw </script><img injection leaked into the exported HTML"
        );
        // Instead the `<` is embedded as its JSON unicode escape.
        assert!(
            html.contains("\\u003c/script"),
            "expected the script-close to be embedded as its \\u003c escape"
        );
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

    // ---- O8: the VALUE overlay viewer (scrubber, readout, externals, ramp) ----

    #[test]
    fn value_overlay_embeds_overlay_json() {
        // The overlay rides inside the same GRAPH JSON the v1 viewer embeds — no
        // second document, no network — so rendering an overlaid model must put
        // `"overlay"` with `"kind":"value"` into the page's GRAPH literal.
        let html = render(&value_overlaid_sample());
        let assign = html
            .split("\nvar GRAPH =")
            .nth(1)
            .expect("a GRAPH assignment");
        let stmt = assign.split(';').next().expect("a terminated statement");
        assert!(
            stmt.contains("\"overlay\""),
            "the overlay must ride inside the embedded GRAPH JSON:\n{stmt:.200}"
        );
        assert!(
            stmt.contains("\"kind\":\"value\""),
            "the embedded overlay must declare kind=value:\n{stmt:.200}"
        );
        // And a node series is carried so the scrubber has values to read.
        assert!(
            stmt.contains("Root.Engine.Speed"),
            "the overlaid node id must be in the embedded JSON:\n{stmt:.200}"
        );
    }

    #[test]
    fn viewer_has_time_scrubber_when_overlay_present() {
        // The value overlay ships a range input over `overlay.time` and an
        // `applyOverlayAtTick` function wired to it, so the user can step ticks
        // and re-colour. Asserted by id/name presence (cargo test can't drive a
        // browser, so the renderer + template are the unit under test).
        let html = render(&value_overlaid_sample());
        assert!(
            html.contains("type=\"range\"") && html.contains("id=\"scrubber\""),
            "expected a <input type=\"range\" id=\"scrubber\"> time scrubber"
        );
        assert!(
            html.contains("applyOverlayAtTick"),
            "expected an applyOverlayAtTick function wired to the scrubber"
        );
        // The whole overlay block is guarded so the structural page is untouched
        // when no overlay is embedded.
        assert!(
            html.contains("GRAPH.overlay"),
            "the overlay viewer must be guarded by a GRAPH.overlay check"
        );
    }

    #[test]
    fn viewer_has_value_readout() {
        // A `#readout` element plus a node click that writes the focused node's
        // value into it (alongside the existing highlightCone).
        let html = render(&value_overlaid_sample());
        assert!(
            html.contains("id=\"readout\""),
            "expected a #readout span for the value readout"
        );
        // The readout is written from the embedded series — the handler reads a
        // node's overlay cell and shows it.
        assert!(
            html.contains("readout"),
            "expected a click handler writing into the readout"
        );
    }

    #[test]
    fn viewer_distinguishes_external_nodes() {
        // Externally-driven channels (`overlay.external`) get a dashed-border
        // treatment so simulated inputs read distinctly from evaluated outputs.
        let html = render(&value_overlaid_sample());
        assert!(
            html.contains("overlay.external") || html.contains(".external"),
            "the viewer must reference overlay.external / an external class"
        );
        assert!(
            html.contains("dashed"),
            "external nodes must get a dashed border treatment"
        );
    }

    #[test]
    fn viewer_value_ramp_present() {
        // A `valueColor(v)` ramp and a size-by-value rule, so numeric nodes
        // colour and size by magnitude.
        let html = render(&value_overlaid_sample());
        assert!(
            html.contains("valueColor"),
            "expected a valueColor(v) ramp function"
        );
        // The size-by-value rule scales node geometry by magnitude.
        assert!(
            html.contains("valueSize") || html.contains("overlay-num"),
            "expected a size-by-value rule for numeric overlay nodes"
        );
    }

    // ---- O9: the DIFF overlay viewer (changed cone, delta ramp, auto-focus) ----

    #[test]
    fn diff_overlay_embeds_diff_json() {
        // The diff overlay rides inside the same GRAPH JSON, so rendering a
        // diff-overlaid model must embed `"kind":"diff"` and the `changed`
        // node-id list (the override's downstream cone — the headline answer).
        let html = render(&diff_overlaid_sample());
        let assign = html
            .split("\nvar GRAPH =")
            .nth(1)
            .expect("a GRAPH assignment");
        let stmt = assign.split(';').next().expect("a terminated statement");
        assert!(
            stmt.contains("\"overlay\""),
            "the diff overlay must ride inside the embedded GRAPH JSON:\n{stmt:.200}"
        );
        assert!(
            stmt.contains("\"kind\":\"diff\""),
            "the embedded overlay must declare kind=diff:\n{stmt:.200}"
        );
        // The changed node-id list is embedded so the viewer can highlight it.
        assert!(
            stmt.contains("\"changed\""),
            "the embedded overlay must carry a changed list:\n{stmt:.200}"
        );
        assert!(
            stmt.contains("Root.Engine.Speed"),
            "the changed node id must be in the embedded JSON:\n{stmt:.200}"
        );
    }

    #[test]
    fn viewer_highlights_changed_nodes() {
        // The diff viewer references `overlay.changed` and an `applyDiff` function
        // that adds a `changed` highlight class to those nodes (reusing the
        // cone-highlight visual vocabulary).
        let html = render(&diff_overlaid_sample());
        assert!(
            html.contains("overlay.changed"),
            "the diff viewer must reference overlay.changed"
        );
        assert!(
            html.contains("applyDiff"),
            "expected an applyDiff function driving the changed-cone highlight"
        );
        // A `changed` class marks the moved nodes; the unchanged remainder is
        // dimmed with the existing cone-dimmed style.
        assert!(
            html.contains("\"changed\"") || html.contains("addClass(\"changed\""),
            "expected a `changed` highlight class on moved nodes"
        );
        assert!(
            html.contains("cone-dimmed"),
            "expected the unchanged remainder dimmed via the existing cone-dimmed style"
        );
    }

    #[test]
    fn viewer_diff_ramp_by_max_abs_delta() {
        // A `deltaColor(d)` ramp keyed on `max_abs_delta` exists, so nodes are
        // coloured by how much the override moved them (neutral at 0).
        let html = render(&diff_overlaid_sample());
        assert!(
            html.contains("deltaColor"),
            "expected a deltaColor(d) ramp keyed on max_abs_delta"
        );
        assert!(
            html.contains("max_abs_delta"),
            "the diff ramp must read each node's max_abs_delta"
        );
    }

    #[test]
    fn viewer_diff_default_focus_is_changed_cone() {
        // On load, a diff overlay auto-runs applyDiff() so the changed set is
        // highlighted without a click — the headline "what did this override move"
        // answer is visible immediately.
        let html = render(&diff_overlaid_sample());
        // The guard is the diff-kind branch.
        assert!(
            html.contains("overlay.kind === \"diff\"") || html.contains("kind === \"diff\""),
            "the diff block must be guarded by a kind=diff check"
        );
        // applyDiff() is invoked on load (not only from a click handler), so the
        // changed cone is the default focus.
        let invoked = html.matches("applyDiff()").count();
        assert!(
            invoked >= 1,
            "expected applyDiff() to be auto-run on load so the changed cone is the default focus"
        );
    }
}
