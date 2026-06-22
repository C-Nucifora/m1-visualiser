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
//! TODO(later): collapse/expand controls for compound (group) nodes, a
//! dagre/elk layered layout (those layout extensions still need vendoring), and
//! filtering by edge kind.

use crate::json;
use crate::model::GraphModel;

/// The HTML shell, with `/*__CYTOSCAPE_JS__*/`, `/*__GRAPH_JSON__*/` and
/// `__TITLE__` placeholders.
const VIEWER_HTML: &str = include_str!("../templates/viewer.html");

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
        .replace("/*__GRAPH_JSON__*/", &graph_json)
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
}
