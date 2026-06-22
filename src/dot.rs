// SPDX-License-Identifier: GPL-3.0-or-later
//! Graphviz DOT export of a [`GraphModel`].
//!
//! Produces a `digraph` with one node per [`GraphNode`] and one edge per
//! [`GraphEdge`]. Node shape/colour is keyed off [`NodeKind`] and edge style off
//! [`EdgeKind`] so the four relationship types read distinctly. Render with e.g.
//! `dot -Tsvg graph.dot -o graph.svg`.
//!
//! TODO(later): emit hierarchy groups as DOT `subgraph cluster_*` blocks so the
//! subsystem nesting renders as boxes rather than only via hierarchy edges.

use crate::model::{EdgeKind, GraphModel, NodeKind};

/// Render the model as a Graphviz DOT document.
pub fn render(model: &GraphModel) -> String {
    let mut out = String::new();
    out.push_str("digraph m1 {\n");
    out.push_str("  rankdir=LR;\n");
    out.push_str("  node [fontname=\"Helvetica\"];\n");
    out.push_str("  edge [fontname=\"Helvetica\"];\n");
    if let Some(title) = &model.title {
        out.push_str(&format!("  label={};\n", quote(title)));
        out.push_str("  labelloc=t;\n");
    }

    for node in &model.nodes {
        let (shape, color) = node_style(node.kind);
        let mut label = node.label().to_string();
        if let Some(rate) = node.rate_hz {
            label.push_str(&format!("\\n{rate} Hz"));
        } else if let Some(unit) = &node.unit {
            label.push_str(&format!("\\n[{unit}]"));
        }
        out.push_str(&format!(
            "  {} [label={}, shape={}, color={}];\n",
            quote(&node.id),
            quote(&label),
            shape,
            color,
        ));
    }

    for edge in &model.edges {
        let (style, color) = edge_style(edge.kind);
        out.push_str(&format!(
            "  {} -> {} [style={}, color={}];\n",
            quote(&edge.from),
            quote(&edge.to),
            style,
            color,
        ));
    }

    out.push_str("}\n");
    out
}

/// DOT `(shape, color)` for a node kind.
fn node_style(kind: NodeKind) -> (&'static str, &'static str) {
    match kind {
        NodeKind::Channel => ("ellipse", "black"),
        NodeKind::Parameter => ("box", "blue"),
        NodeKind::Constant => ("box", "gray"),
        NodeKind::Table => ("box3d", "purple"),
        NodeKind::Function => ("component", "darkgreen"),
        NodeKind::Group => ("folder", "orange"),
    }
}

/// DOT `(style, color)` for an edge kind.
fn edge_style(kind: EdgeKind) -> (&'static str, &'static str) {
    match kind {
        EdgeKind::DataFlow => ("solid", "black"),
        EdgeKind::TableAxis => ("dashed", "purple"),
        EdgeKind::Hierarchy => ("dotted", "orange"),
        EdgeKind::Schedule => ("dashed", "red"),
    }
}

/// Quote and escape a string as a DOT-safe double-quoted identifier/label.
fn quote(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{GraphEdge, GraphNode};

    fn sample() -> GraphModel {
        let mut g = GraphModel::new(Some("Demo".into()));
        g.nodes.push(GraphNode::new("Root.Engine", NodeKind::Group));
        let mut ch = GraphNode::new("Root.Engine.Speed", NodeKind::Channel);
        ch.unit = Some("rpm".into());
        g.nodes.push(ch);
        g.edges.push(GraphEdge::new(
            "Root.Engine",
            "Root.Engine.Speed",
            EdgeKind::Hierarchy,
        ));
        g
    }

    #[test]
    fn renders_digraph_with_nodes_and_edges() {
        let dot = render(&sample());
        assert!(dot.starts_with("digraph m1 {"));
        assert!(dot.contains("\"Root.Engine.Speed\""));
        assert!(dot.contains("\"Root.Engine\" -> \"Root.Engine.Speed\""));
        assert!(dot.trim_end().ends_with('}'));
    }

    #[test]
    fn quoting_escapes_quotes_and_backslashes() {
        assert_eq!(quote("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }
}
