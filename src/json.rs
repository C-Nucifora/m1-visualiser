// SPDX-License-Identifier: GPL-3.0-or-later
//! JSON export of a [`GraphModel`].
//!
//! This is the canonical machine-readable form of the graph and the same payload
//! that [`crate::html`] embeds into the interactive viewer. Serialization is via
//! `serde`; see the `Serialize` derives on the model types.

use crate::model::GraphModel;

/// Render the model as pretty-printed JSON.
///
/// Serialization of [`GraphModel`] cannot fail (it contains only strings,
/// numbers, enums and vecs), so this returns a plain `String`; we fall back to
/// `"{}"` defensively rather than panicking if `serde_json` ever errored.
pub fn render(model: &GraphModel) -> String {
    serde_json::to_string_pretty(model).unwrap_or_else(|_| "{}".to_string())
}

/// Render the model as compact (single-line) JSON, used when embedding into the
/// HTML viewer.
pub fn render_compact(model: &GraphModel) -> String {
    serde_json::to_string(model).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EdgeKind, GraphEdge, GraphNode, NodeKind};

    #[test]
    fn renders_parseable_json_with_nodes_and_edges() {
        let mut g = GraphModel::new(Some("Demo".into()));
        g.nodes
            .push(GraphNode::new("Root.Engine.Speed", NodeKind::Channel));
        g.edges.push(GraphEdge::new(
            "Root.Engine",
            "Root.Engine.Speed",
            EdgeKind::Hierarchy,
        ));
        let json = render(&g);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["title"], "Demo");
        assert_eq!(parsed["nodes"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["nodes"][0]["kind"], "channel");
        assert_eq!(parsed["edges"][0]["kind"], "hierarchy");
    }

    #[test]
    fn compact_has_no_newlines() {
        let g = GraphModel::new(None);
        assert!(!render_compact(&g).contains('\n'));
    }
}
