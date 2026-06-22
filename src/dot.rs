// SPDX-License-Identifier: GPL-3.0-or-later
//! Graphviz DOT export of a [`GraphModel`].
//!
//! Produces a `digraph` with one node per [`GraphNode`] and one edge per
//! `GraphEdge`. Node shape/colour is keyed off [`NodeKind`] and edge style off
//! [`EdgeKind`] so the four relationship types read distinctly. Render with e.g.
//! `dot -Tsvg graph.dot -o graph.svg`.
//!
//! Subsystem groups render as nested `subgraph cluster_<id>` blocks so the
//! hierarchy reads as boxes, with each non-group node placed inside its nearest
//! group cluster.
//!
//! When a [`GraphModel`] carries a value/diff [`Overlay`], DOT degrades
//! gracefully: each node's *at-time* cell (its `start_tick`, else its last tick)
//! is looked up by node id, and a numeric value is appended to that node's label
//! (e.g. `Speed\n50`). Non-numeric cells and nodes the overlay never touched are
//! left untouched, so an un-overlaid model renders byte-identically to v1.

use crate::model::{EdgeKind, GraphModel, GraphNode, NodeKind, Overlay, OverlayCell};
use std::collections::{BTreeMap, HashMap};

/// Render the model as a Graphviz DOT document.
///
/// Subsystem nesting is emitted as nested `subgraph cluster_<id>` blocks: every
/// [`NodeKind::Group`] becomes a cluster (labelled by its leaf segment), and
/// every non-group node is declared inside its *nearest* enclosing group
/// cluster. Top-level groups and any node with no group ancestor are declared at
/// the digraph root. Per-kind node/edge styling and `rankdir=LR` are preserved.
///
/// If `model.overlay` is set, each node's at-time numeric value is appended to
/// its label (see the module docs); absent an overlay the output is unchanged.
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

    // Index every node by id, and bucket each node under the nearest group
    // ancestor that exists in the model (or `None` for the digraph root).
    let by_id: HashMap<&str, &GraphNode> = model.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let cluster_of: HashMap<&str, Option<&str>> = model
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), nearest_group_ancestor(n, &by_id)))
        .collect();

    // Group ids whose parent (nearest group ancestor) is some other group, vs.
    // the top-level groups whose cluster is the digraph root. We render the tree
    // by recursing from the roots.
    let mut children_groups: BTreeMap<Option<&str>, Vec<&str>> = BTreeMap::new();
    let mut members: BTreeMap<Option<&str>, Vec<&GraphNode>> = BTreeMap::new();
    for node in &model.nodes {
        let bucket = cluster_of[node.id.as_str()];
        if node.kind == NodeKind::Group {
            children_groups.entry(bucket).or_default().push(&node.id);
        } else {
            members.entry(bucket).or_default().push(node);
        }
    }

    // Emit non-group members that belong directly to the digraph root, then each
    // top-level cluster recursively.
    if let Some(top_members) = members.get(&None) {
        for node in top_members {
            out.push_str(&node_decl(node, 1, model.overlay.as_ref()));
        }
    }
    if let Some(roots) = children_groups.get(&None) {
        for group_id in roots {
            emit_cluster(
                group_id,
                &by_id,
                &children_groups,
                &members,
                1,
                model.overlay.as_ref(),
                &mut out,
            );
        }
    }

    // Edges. Suppress Hierarchy edges that the cluster nesting already conveys,
    // to cut clutter:
    //  - the child (`to`) is drawn directly inside `from`'s own cluster
    //    (`cluster_of[to] == from`): the box already shows the containment; or
    //  - both endpoints land in the same nearest cluster (e.g. a parameter and
    //    its `.Value` member sitting together in one group box).
    // Cross-cluster Hierarchy edges (and every other edge kind) are still drawn.
    for edge in &model.edges {
        if edge.kind == EdgeKind::Hierarchy {
            let from_cluster = cluster_of.get(edge.from.as_str()).copied().flatten();
            let to_cluster = cluster_of.get(edge.to.as_str()).copied().flatten();
            let child_nested_in_from = to_cluster == Some(edge.from.as_str());
            if child_nested_in_from || from_cluster == to_cluster {
                continue;
            }
        }
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

/// The nearest ancestor of `node` (walking `.`-separated path segments) that is
/// a [`NodeKind::Group`] present in the model. `None` means the node sits at the
/// digraph root. M1 identifiers may contain spaces, so we split only on `.`.
fn nearest_group_ancestor<'a>(
    node: &'a GraphNode,
    by_id: &HashMap<&'a str, &'a GraphNode>,
) -> Option<&'a str> {
    let mut current = node.parent.as_deref();
    while let Some(path) = current {
        if let Some(parent_node) = by_id.get(path)
            && parent_node.kind == NodeKind::Group
        {
            // Return the id slice owned by the indexed node so the borrow lives
            // as long as the model, not the local `node`.
            return Some(parent_node.id.as_str());
        }
        current = parent_path(path);
    }
    None
}

/// Recursively emit one `subgraph cluster_<id>` block at the given indent depth.
fn emit_cluster<'a>(
    group_id: &'a str,
    by_id: &HashMap<&'a str, &'a GraphNode>,
    children_groups: &BTreeMap<Option<&'a str>, Vec<&'a str>>,
    members: &BTreeMap<Option<&'a str>, Vec<&'a GraphNode>>,
    depth: usize,
    overlay: Option<&Overlay>,
    out: &mut String,
) {
    let indent = "  ".repeat(depth);
    let label = by_id.get(group_id).map(|n| n.label()).unwrap_or(group_id);
    out.push_str(&format!(
        "{indent}subgraph {} {{\n",
        quote(&format!("cluster_{group_id}"))
    ));
    out.push_str(&format!("{indent}  label={};\n", quote(label)));
    out.push_str(&format!("{indent}  color=orange;\n"));

    // Direct non-group members of this cluster.
    if let Some(mems) = members.get(&Some(group_id)) {
        for node in mems {
            out.push_str(&node_decl(node, depth + 1, overlay));
        }
    }
    // Nested sub-clusters.
    if let Some(subs) = children_groups.get(&Some(group_id)) {
        for sub in subs {
            emit_cluster(
                sub,
                by_id,
                children_groups,
                members,
                depth + 1,
                overlay,
                out,
            );
        }
    }
    out.push_str(&format!("{indent}}}\n"));
}

/// The DOT declaration line for a single non-group node at the given indent.
///
/// When `overlay` is `Some` and carries a numeric at-time cell for this node
/// (see [`at_time_value`]), the value is appended to the label as a final
/// `\n<value>` segment, so the static export reads the value alongside the
/// structural metadata. Absent such a cell the label is exactly the v1 label.
fn node_decl(node: &GraphNode, depth: usize, overlay: Option<&Overlay>) -> String {
    let (shape, color) = node_style(node.kind);
    let mut label = node.label().to_string();
    if let Some(rate) = node.rate_hz {
        label.push_str(&format!("\\n{rate} Hz"));
    } else if let Some(unit) = &node.unit {
        label.push_str(&format!("\\n[{unit}]"));
    }
    if let Some(value) = overlay.and_then(|o| at_time_value(o, &node.id)) {
        label.push_str(&format!("\\n{value}"));
    }
    format!(
        "{}{} [label={}, shape={}, color={}];\n",
        "  ".repeat(depth),
        quote(&node.id),
        quote(&label),
        shape,
        color,
    )
}

/// The node's *at-time* numeric value from an [`Overlay`], or `None`.
///
/// The at-time tick is the overlay's [`Overlay::start_tick`] hint when set, else
/// the node series' last tick (the design's "last-tick value"). Only a numeric
/// ([`OverlayCell::Num`]) cell yields a value to append; boolean/string cells are
/// label-only and nodes the overlay never touched return `None`. The number is
/// formatted like Rust's default `f64` display (e.g. `50`, not `50.0`).
fn at_time_value(overlay: &Overlay, node_id: &str) -> Option<f64> {
    let series = &overlay.nodes.get(node_id)?.series;
    let tick = overlay
        .start_tick
        .filter(|&i| i < series.len())
        .unwrap_or(series.len().checked_sub(1)?);
    match series.get(tick)? {
        OverlayCell::Num(v) => Some(*v),
        OverlayCell::Bool(_) | OverlayCell::Str(_) => None,
    }
}

/// The parent path of a dotted id: everything before the final `.` segment, or
/// `None` if the id has no `.`. M1 identifiers may contain spaces, so this
/// splits only on `.`.
fn parent_path(path: &str) -> Option<&str> {
    path.rsplit_once('.').map(|(parent, _)| parent)
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
    use crate::model::{GraphEdge, GraphNode, NodeOverlay, Overlay, OverlayCell, OverlayKind};
    use std::collections::{BTreeMap, BTreeSet};

    fn sample() -> GraphModel {
        let mut g = GraphModel::new(Some("Demo".into()));
        let mut root = GraphNode::new("Root", NodeKind::Group);
        root.parent = None;
        g.nodes.push(root);
        let mut engine = GraphNode::new("Root.Engine", NodeKind::Group);
        engine.parent = Some("Root".into());
        g.nodes.push(engine);
        let mut ch = GraphNode::new("Root.Engine.Speed", NodeKind::Channel);
        ch.unit = Some("rpm".into());
        ch.parent = Some("Root.Engine".into());
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
        assert!(dot.trim_end().ends_with('}'));
    }

    #[test]
    fn quoting_escapes_quotes_and_backslashes() {
        assert_eq!(quote("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }

    #[test]
    fn emits_cluster_for_group_nodes() {
        let dot = render(&sample());
        // Each group becomes a `subgraph cluster_<id>` block, labelled by its
        // leaf segment.
        assert!(
            dot.contains("subgraph \"cluster_Root.Engine\" {"),
            "expected a cluster for Root.Engine:\n{dot}"
        );
        assert!(
            dot.contains("subgraph \"cluster_Root\" {"),
            "expected a cluster for Root:\n{dot}"
        );
        assert!(
            dot.contains("label=\"Engine\""),
            "cluster should be labelled by group leaf:\n{dot}"
        );

        // The nested channel is declared exactly once — inside its cluster, not
        // also at top level. Its declaration line is indented deeper than a
        // top-level declaration would be.
        let decls: Vec<&str> = dot
            .lines()
            .filter(|l| l.contains("\"Root.Engine.Speed\" [label="))
            .collect();
        assert_eq!(
            decls.len(),
            1,
            "channel should be declared exactly once:\n{dot}"
        );
        assert!(
            decls[0].starts_with("      "),
            "channel decl should be nested inside its cluster (indented):\n{}",
            decls[0]
        );
    }

    #[test]
    fn intra_cluster_hierarchy_edges_are_suppressed() {
        // The Hierarchy edge Root.Engine -> Root.Engine.Speed is redundant once
        // the channel is drawn inside cluster_Root.Engine, so it is suppressed.
        let dot = render(&sample());
        assert!(
            !dot.contains("\"Root.Engine\" -> \"Root.Engine.Speed\""),
            "intra-cluster hierarchy edge should be suppressed:\n{dot}"
        );
    }

    #[test]
    fn dot_renders_without_overlay_unchanged() {
        // DOT degrades gracefully: attaching an overlay must not perturb the
        // structural output unless a node has an at-time numeric cell. With no
        // overlay at all, the document is byte-identical to v1.
        let g = sample();
        assert!(g.overlay.is_none());
        let bare = render(&g);
        assert!(bare.starts_with("digraph m1 {"));
        assert!(bare.trim_end().ends_with('}'));

        // Attaching an overlay whose node ids do not match any graph node leaves
        // the DOT byte-identical to the un-overlaid render (no spurious labels).
        let mut nodes = BTreeMap::new();
        nodes.insert(
            "Root.Other.Thing".to_string(),
            NodeOverlay {
                series: vec![OverlayCell::Num(99.0)],
                delta: None,
                max_abs_delta: None,
            },
        );
        let unmatched = Overlay {
            kind: OverlayKind::Value,
            time: vec![0.0],
            nodes,
            external: BTreeSet::new(),
            changed: Vec::new(),
            eps: None,
            start_tick: None,
        };
        let overlaid = render(&sample().with_overlay(unmatched));
        assert_eq!(
            bare, overlaid,
            "an overlay with no matching node must not change the DOT:\n{overlaid}"
        );
    }

    #[test]
    fn dot_appends_at_time_value_to_node_label() {
        // When a node carries a numeric at-time cell, DOT appends it to the node
        // label (e.g. `Speed\n50`) so the static export reads the value too. The
        // at-time cell defaults to the last tick; `start_tick` overrides it.
        let mut nodes = BTreeMap::new();
        nodes.insert(
            "Root.Engine.Speed".to_string(),
            NodeOverlay {
                series: vec![OverlayCell::Num(10.0), OverlayCell::Num(50.0)],
                delta: None,
                max_abs_delta: None,
            },
        );
        let overlay = Overlay {
            kind: OverlayKind::Value,
            time: vec![0.0, 0.01],
            nodes,
            external: BTreeSet::new(),
            changed: Vec::new(),
            eps: None,
            start_tick: None,
        };
        let dot = render(&sample().with_overlay(overlay));
        let decl = dot
            .lines()
            .find(|l| l.contains("\"Root.Engine.Speed\" [label="))
            .expect("Speed node should be declared");
        // Last tick (50) is the default at-time value, appended after the unit.
        // The label's newline separators are DOT-escaped (`\\n`) by `quote`.
        assert!(
            decl.contains("Speed\\\\n[rpm]\\\\n50"),
            "expected the at-time value appended to the label:\n{decl}"
        );
    }

    #[test]
    fn dot_at_time_value_honours_start_tick() {
        // `start_tick` selects which cell is the "at-time" value, not the last.
        let mut nodes = BTreeMap::new();
        nodes.insert(
            "Root.Engine.Speed".to_string(),
            NodeOverlay {
                series: vec![OverlayCell::Num(10.0), OverlayCell::Num(50.0)],
                delta: None,
                max_abs_delta: None,
            },
        );
        let overlay = Overlay {
            kind: OverlayKind::Value,
            time: vec![0.0, 0.01],
            nodes,
            external: BTreeSet::new(),
            changed: Vec::new(),
            eps: None,
            start_tick: Some(0),
        };
        let dot = render(&sample().with_overlay(overlay));
        let decl = dot
            .lines()
            .find(|l| l.contains("\"Root.Engine.Speed\" [label="))
            .expect("Speed node should be declared");
        assert!(
            decl.contains("Speed\\\\n[rpm]\\\\n10"),
            "start_tick=0 should select the first cell (10):\n{decl}"
        );
    }

    #[test]
    fn dot_does_not_append_non_numeric_cells() {
        // A label-only cell (bool/str) carries no colour ramp and no numeric
        // value to append, so the label is unchanged from the structural render.
        let mut nodes = BTreeMap::new();
        nodes.insert(
            "Root.Engine.Speed".to_string(),
            NodeOverlay {
                series: vec![OverlayCell::Str("Idle".into())],
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
        let bare = render(&sample());
        let overlaid = render(&sample().with_overlay(overlay));
        assert_eq!(
            bare, overlaid,
            "a non-numeric at-time cell must not change the DOT:\n{overlaid}"
        );
    }

    #[test]
    fn per_kind_edge_styles_are_distinct() {
        // Lock the per-kind (style, color) contract: all four EdgeKinds must map
        // to a unique (style, color) pair so the relationship types read apart.
        let kinds = [
            EdgeKind::DataFlow,
            EdgeKind::TableAxis,
            EdgeKind::Hierarchy,
            EdgeKind::Schedule,
        ];
        let mut seen = std::collections::HashSet::new();
        for k in kinds {
            let style = edge_style(k);
            assert!(
                seen.insert(style),
                "edge style {style:?} for {k:?} collides with another kind"
            );
        }
        assert_eq!(seen.len(), 4, "all four edge kinds must be distinct");
    }
}
