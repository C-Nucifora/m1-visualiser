// SPDX-License-Identifier: GPL-3.0-or-later
//! The in-memory graph model — the single source of truth that the renderers
//! (DOT / JSON / HTML) read. No `m1-core` / `m1-typecheck` types leak past this
//! boundary; the loader translates the project's symbol table into this model.

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

/// The whole project's structural graph. Nodes and edges are kept in
/// deterministic order by the loader so output is stable.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GraphModel {
    /// Optional human title (e.g. the project directory name).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

impl GraphModel {
    /// An empty graph with an optional title.
    pub fn new(title: Option<String>) -> Self {
        GraphModel {
            title,
            nodes: Vec::new(),
            edges: Vec::new(),
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
}
