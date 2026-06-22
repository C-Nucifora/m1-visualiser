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
    use crate::model::{
        EdgeKind, GraphEdge, GraphNode, NodeKind, NodeOverlay, Overlay, OverlayCell, OverlayKind,
    };
    use std::collections::{BTreeMap, BTreeSet};

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

    #[test]
    fn overlay_is_present_in_json_when_attached() {
        // The JSON export is the canonical machine-readable form and the HTML
        // payload, so an attached overlay must ride in verbatim — `"overlay"`
        // with the node series — while an un-overlaid model carries no such key
        // (re-asserting O1's `skip_serializing_if` invariant at the renderer).
        let mut g = GraphModel::new(Some("Demo".into()));
        g.nodes
            .push(GraphNode::new("Root.Demo.Output", NodeKind::Channel));

        // Un-overlaid: no `"overlay"` key.
        let bare = render(&g);
        assert!(
            !bare.contains("\"overlay\""),
            "un-overlaid model leaked an `overlay` key:\n{bare}"
        );

        // Overlaid: `"overlay"` present, carrying the node series.
        let mut nodes = BTreeMap::new();
        nodes.insert(
            "Root.Demo.Output".to_string(),
            NodeOverlay {
                series: vec![OverlayCell::Num(50.0)],
                delta: None,
                max_abs_delta: None,
            },
        );
        let overlay = Overlay {
            kind: OverlayKind::Value,
            time: vec![0.0],
            nodes,
            external: BTreeSet::new(),
            changed: Vec::new(),
            eps: None,
            start_tick: None,
        };
        let g = g.with_overlay(overlay);
        let json = render(&g);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["overlay"]["kind"], "value");
        assert_eq!(
            parsed["overlay"]["nodes"]["Root.Demo.Output"]["series"][0]["num"],
            50.0
        );
    }
}
