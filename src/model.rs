// SPDX-License-Identifier: GPL-3.0-or-later
//! The in-memory graph model — the single source of truth that the renderers
//! (DOT / JSON / HTML) read. No `m1-core` / `m1-typecheck` types leak past this
//! boundary; the loader translates the project's symbol table into this model.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// The kind of structural entity a [`GraphNode`] represents.
///
/// These mirror the subset of `m1-typecheck`'s `SymbolKind` that the visualiser
/// surfaces, plus synthesized [`NodeKind::Group`] ancestors implied by dotted
/// paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// A runtime signal (`SymbolKind::Channel`).
    Channel,
    /// A tunable (`SymbolKind::Parameter`).
    Parameter,
    /// A fixed value (`SymbolKind::Constant`).
    Constant,
    /// A lookup table (`SymbolKind::Table`).
    Table,
    /// A user function or method (`SymbolKind::Function` / `Method`).
    Function,
    /// A subsystem / container — a declared group or a synthesized ancestor
    /// implied by a dotted path. Rendered as a Cytoscape compound (parent) node.
    Group,
}

impl NodeKind {
    /// A short, stable lowercase tag used in DOT/HTML styling and JSON.
    pub fn tag(self) -> &'static str {
        match self {
            NodeKind::Channel => "channel",
            NodeKind::Parameter => "parameter",
            NodeKind::Constant => "constant",
            NodeKind::Table => "table",
            NodeKind::Function => "function",
            NodeKind::Group => "group",
        }
    }
}

/// The kind of relationship a `GraphEdge` represents. The four structural
/// relationship types the visualiser models (see the design doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// A script reads one symbol and writes another (read/write dependency).
    /// Reads point into the backing function; writes point out of it.
    DataFlow,
    /// Links a table's input-axis symbols / output channel to the table node.
    TableAxis,
    /// Subsystem/group containment, derived from dotted-path nesting. Renders as
    /// a parent→child compound relationship.
    Hierarchy,
    /// Execution-rate / scheduling relationship (e.g. a function and its clock).
    Schedule,
}

impl EdgeKind {
    /// A short, stable lowercase tag used in DOT/HTML styling and JSON.
    pub fn tag(self) -> &'static str {
        match self {
            EdgeKind::DataFlow => "data_flow",
            EdgeKind::TableAxis => "table_axis",
            EdgeKind::Hierarchy => "hierarchy",
            EdgeKind::Schedule => "schedule",
        }
    }
}

/// One node in the graph: a channel, parameter, constant, table, function, or
/// group. `id` is a stable identifier (the dotted path) used to wire up edges.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphNode {
    /// Stable node identifier — the symbol's dotted path (e.g. `Root.Engine.Speed`).
    pub id: String,
    /// The dotted path (same as `id` for now; kept distinct so `id` can become a
    /// surrogate key later without breaking labels).
    pub path: String,
    /// What this node represents.
    pub kind: NodeKind,
    /// Execution / logging rate in Hz, when known (functions, scheduled nodes).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_hz: Option<f64>,
    /// Engineering unit for display, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Declared storage / value type label, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_label: Option<String>,
    /// For tables: number of input axes (dimensionality), when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table_dims: Option<usize>,
    /// The id of the containing group, when this node is nested. Drives the
    /// Cytoscape compound `parent` field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

impl GraphNode {
    /// Construct a bare node of the given `kind` at `path`. Optional metadata
    /// (`rate_hz`, `unit`, …) is filled in by the loader as available.
    pub fn new(path: impl Into<String>, kind: NodeKind) -> Self {
        let path = path.into();
        GraphNode {
            id: path.clone(),
            path,
            kind,
            rate_hz: None,
            unit: None,
            type_label: None,
            table_dims: None,
            parent: None,
        }
    }

    /// The label shown in renderers: the final dotted segment of the path, or the
    /// whole path when it has no dots.
    pub fn label(&self) -> &str {
        self.path.rsplit('.').next().unwrap_or(&self.path)
    }
}

/// One directed edge between two nodes, of a given [`EdgeKind`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphEdge {
    /// Source node id.
    pub from: String,
    /// Target node id.
    pub to: String,
    /// The relationship kind.
    pub kind: EdgeKind,
}

impl GraphEdge {
    /// Construct an edge of `kind` from `from` to `to`.
    pub fn new(from: impl Into<String>, to: impl Into<String>, kind: EdgeKind) -> Self {
        GraphEdge {
            from: from.into(),
            to: to.into(),
            kind,
        }
    }
}

/// Which overlay workflow produced an [`Overlay`].
///
/// This is a toolchain-agnostic mirror of the two `m1-eval` sources (a [`Trace`]
/// for [`OverlayKind::Value`], a `Counterfactual` for [`OverlayKind::Diff`]); the
/// model deliberately knows nothing of `m1-eval` — the `eval` boundary module
/// builds the [`Overlay`] and the model just carries it.
///
/// [`Trace`]: https://docs.rs/m1-eval
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayKind {
    /// Computed values at each tick (from a scenario, whole-project run, or a
    /// replayed log). Drives the colour/size-by-value viewer.
    Value,
    /// Counterfactual deltas (a logged run vs. an override). Drives the
    /// changed-cone highlight and the colour-by-magnitude-of-change viewer.
    Diff,
}

/// One cell of a node's per-tick series — a single computed value, rendered in a
/// faithful, ramp-aware tagged form.
///
/// The tag lets the viewer tell a numeric value (which gets a colour/size ramp)
/// from a label-only value (a boolean or an enum/string, which is shown verbatim
/// but never ramped). This preserves the bool/enum/string distinction that a
/// bare scalar would lose.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayCell {
    /// A numeric value — colour-by-magnitude applies.
    Num(f64),
    /// A boolean value — label-only.
    Bool(bool),
    /// A non-numeric display value (enum member, string) — label-only.
    Str(String),
}

/// The per-node overlay payload: a value series aligned to [`Overlay::time`],
/// plus optional diff data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeOverlay {
    /// One [`OverlayCell`] per tick, aligned to [`Overlay::time`].
    pub series: Vec<OverlayCell>,
    /// Per-tick delta (counterfactual − logged) in [`OverlayKind::Diff`] mode;
    /// `None` for a [`OverlayKind::Value`] overlay.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub delta: Option<Vec<f64>>,
    /// The diff summary used by the colour ramp (`max |delta|`) in
    /// [`OverlayKind::Diff`] mode; `None` for a [`OverlayKind::Value`] overlay.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub max_abs_delta: Option<f64>,
}

/// A value/diff overlay keyed by graph-node id.
///
/// Built by the `eval` boundary module from an `m1-eval` `Trace` (value mode) or
/// `Counterfactual` (diff mode), then folded onto a [`GraphModel`]. It is pure
/// data — no `m1-eval` type appears here, so the model stays toolchain-agnostic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Overlay {
    /// Which workflow produced this overlay.
    pub kind: OverlayKind,
    /// The shared tick axis (seconds); every [`NodeOverlay::series`] aligns to it.
    pub time: Vec<f64>,
    /// Per-node overlay payloads, keyed by node id. Only nodes the run touched
    /// appear; others render neutral. `BTreeMap` keeps output deterministic.
    pub nodes: BTreeMap<String, NodeOverlay>,
    /// Externally-driven node ids (simulated inputs, not evaluated outputs).
    /// Populated in [`OverlayKind::Value`] mode; the viewer renders these
    /// distinctly. `BTreeSet` keeps output deterministic.
    pub external: BTreeSet<String>,
    /// Changed node ids (the override's downstream cone) in
    /// [`OverlayKind::Diff`] mode; empty in [`OverlayKind::Value`] mode and for a
    /// no-op override (the load-bearing invariant).
    pub changed: Vec<String>,
    /// The diff threshold (`eps`) in [`OverlayKind::Diff`] mode; `None` otherwise.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub eps: Option<f64>,
}

/// The whole project's structural graph. Nodes and edges are kept in
/// deterministic order by the loader so output is stable.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GraphModel {
    /// Optional human title (e.g. the project directory name).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    /// An optional value/diff overlay. Absent for the structural v1 render, so an
    /// un-overlaid model serialises byte-identically to v1 (the `skip` below).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub overlay: Option<Overlay>,
}

impl GraphModel {
    /// An empty graph with an optional title.
    pub fn new(title: Option<String>) -> Self {
        GraphModel {
            title,
            nodes: Vec::new(),
            edges: Vec::new(),
            overlay: None,
        }
    }

    /// Count of edges of a given kind — handy in tests and summaries.
    pub fn edge_count(&self, kind: EdgeKind) -> usize {
        self.edges.iter().filter(|e| e.kind == kind).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_label_is_final_segment() {
        let n = GraphNode::new("Root.Engine.Speed", NodeKind::Channel);
        assert_eq!(n.label(), "Speed");
    }

    #[test]
    fn node_label_without_dots_is_whole_path() {
        let n = GraphNode::new("Root", NodeKind::Group);
        assert_eq!(n.label(), "Root");
    }

    #[test]
    fn kind_tags_are_stable() {
        assert_eq!(NodeKind::Channel.tag(), "channel");
        assert_eq!(NodeKind::Table.tag(), "table");
        assert_eq!(EdgeKind::DataFlow.tag(), "data_flow");
        assert_eq!(EdgeKind::Hierarchy.tag(), "hierarchy");
    }

    #[test]
    fn edge_count_filters_by_kind() {
        let mut g = GraphModel::new(Some("T".into()));
        g.edges.push(GraphEdge::new("a", "b", EdgeKind::Hierarchy));
        g.edges.push(GraphEdge::new("b", "c", EdgeKind::Hierarchy));
        g.edges.push(GraphEdge::new("c", "d", EdgeKind::Schedule));
        assert_eq!(g.edge_count(EdgeKind::Hierarchy), 2);
        assert_eq!(g.edge_count(EdgeKind::Schedule), 1);
        assert_eq!(g.edge_count(EdgeKind::DataFlow), 0);
    }

    #[test]
    fn model_round_trips_through_json() {
        let mut g = GraphModel::new(Some("Demo".into()));
        let mut node = GraphNode::new("Root.Engine.Speed", NodeKind::Channel);
        node.unit = Some("rpm".into());
        node.parent = Some("Root.Engine".into());
        g.nodes.push(node);
        g.edges.push(GraphEdge::new(
            "Root.Engine",
            "Root.Engine.Speed",
            EdgeKind::Hierarchy,
        ));
        let json = serde_json::to_string(&g).unwrap();
        let back: GraphModel = serde_json::from_str(&json).unwrap();
        assert_eq!(g, back);
    }

    #[test]
    fn model_without_overlay_serialises_unchanged() {
        // Back-compat lock: a model with no overlay must produce JSON with no
        // `"overlay"` key, so every existing JSON/HTML/DOT test stays valid.
        let mut g = GraphModel::new(Some("Demo".into()));
        g.nodes
            .push(GraphNode::new("Root.Engine.Speed", NodeKind::Channel));
        assert!(g.overlay.is_none());
        let json = serde_json::to_string(&g).unwrap();
        assert!(
            !json.contains("overlay"),
            "un-overlaid model leaked an `overlay` key: {json}"
        );
    }

    #[test]
    fn overlay_round_trips_through_json() {
        let mut nodes = BTreeMap::new();
        nodes.insert(
            "Root.Demo.Output".to_string(),
            NodeOverlay {
                series: vec![OverlayCell::Num(50.0), OverlayCell::Num(50.0)],
                delta: Some(vec![0.0, 0.0]),
                max_abs_delta: Some(0.0),
            },
        );
        nodes.insert(
            "Root.Demo.Mode".to_string(),
            NodeOverlay {
                series: vec![OverlayCell::Str("Idle".into())],
                delta: None,
                max_abs_delta: None,
            },
        );
        let overlay = Overlay {
            kind: OverlayKind::Diff,
            time: vec![0.0, 0.01],
            nodes,
            external: ["Root.Demo.Speed".to_string()].into_iter().collect(),
            changed: vec!["Root.Demo.Output".to_string()],
            eps: Some(1e-9),
        };
        let json = serde_json::to_string(&overlay).unwrap();
        let back: Overlay = serde_json::from_str(&json).unwrap();
        assert_eq!(overlay, back);
    }

    #[test]
    fn overlay_cell_serialises_tagged() {
        // Each variant must round-trip carrying its tag, so the viewer can tell
        // numeric-rampable (`num`) from label-only (`bool`/`str`).
        for (cell, tag) in [
            (OverlayCell::Num(50.0), "num"),
            (OverlayCell::Bool(true), "bool"),
            (OverlayCell::Str("Idle".into()), "str"),
        ] {
            let json = serde_json::to_string(&cell).unwrap();
            assert!(
                json.contains(tag),
                "cell {cell:?} did not carry tag `{tag}`: {json}"
            );
            let back: OverlayCell = serde_json::from_str(&json).unwrap();
            assert_eq!(cell, back);
        }
    }

    #[test]
    fn overlaid_model_round_trips_through_json() {
        let mut g = GraphModel::new(Some("Demo".into()));
        g.nodes
            .push(GraphNode::new("Root.Demo.Output", NodeKind::Channel));
        let mut nodes = BTreeMap::new();
        nodes.insert(
            "Root.Demo.Output".to_string(),
            NodeOverlay {
                series: vec![OverlayCell::Num(50.0)],
                delta: None,
                max_abs_delta: None,
            },
        );
        g.overlay = Some(Overlay {
            kind: OverlayKind::Value,
            time: vec![0.0],
            nodes,
            external: BTreeSet::new(),
            changed: Vec::new(),
            eps: None,
        });
        let json = serde_json::to_string(&g).unwrap();
        assert!(json.contains("overlay"));
        let back: GraphModel = serde_json::from_str(&json).unwrap();
        assert_eq!(g, back);
    }
}
