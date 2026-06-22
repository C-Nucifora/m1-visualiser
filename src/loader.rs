// SPDX-License-Identifier: GPL-3.0-or-later
//! Builds a [`GraphModel`] from a loaded `m1-typecheck` project.
//!
//! This is the only module that touches `m1-typecheck` / `m1-core` types: it
//! translates the project's symbol table into the toolchain-agnostic
//! [`GraphModel`] that the DOT / JSON / HTML renderers consume. This mirrors
//! `m1-doc`'s `loader.rs`, which builds a `DocModel` the same way.
//!
//! What the structural-first v1 derives from the symbol table:
//!
//! - **Nodes** from every channel / parameter / constant / table / function /
//!   group symbol.
//! - **Hierarchy edges** from dotted-path containment (`Root.Engine.Speed` is a
//!   child of `Root.Engine`), with missing ancestor groups synthesized.
//! - **TableAxis edges** linking a table's auto-created members (its `.Value`
//!   output channel and axis channels nested under the table's path) to the
//!   table node, using `Symbol.table_meta` for dimensionality.
//! - **Schedule edges** linking a function/scheduled node to a synthetic clock
//!   node for its `call_rate_hz`.
//!
//! `load` also ingests the optional `.m1cfg` (for `table_meta`) and discovers
//! the project's `*.m1scr` scripts (parsed once via `parse_all`), threading them
//! into the model for the data-flow pass.
//!
//! `DataFlow` edges require per-script CST read/write analysis and are stubbed —
//! see [`add_data_flow_edges`].

use crate::model::{EdgeKind, GraphEdge, GraphModel, GraphNode, NodeKind};
use m1_typecheck::Project;
use m1_typecheck::parsed::{ParsedScript, parse_all};
use m1_typecheck::symbols::{Symbol, SymbolKind};
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;

/// Map an `m1-typecheck` symbol kind to a [`NodeKind`]. Returns `None` for kinds
/// the structural graph does not surface as nodes (references, opaque objects,
/// `Other`).
fn node_kind(kind: SymbolKind) -> Option<NodeKind> {
    match kind {
        SymbolKind::Channel => Some(NodeKind::Channel),
        SymbolKind::Parameter => Some(NodeKind::Parameter),
        SymbolKind::Constant => Some(NodeKind::Constant),
        SymbolKind::Table => Some(NodeKind::Table),
        SymbolKind::Function | SymbolKind::Method => Some(NodeKind::Function),
        SymbolKind::Group => Some(NodeKind::Group),
        // Objects, references and `Other` are not surfaced as their own nodes in
        // v1; their members still appear via their dotted paths.
        SymbolKind::Object | SymbolKind::Reference | SymbolKind::Other => None,
    }
}

/// The parent (containing) path of a dotted path: everything before the final
/// `.` segment. `None` when the path has no dot (a top-level node).
fn parent_path(path: &str) -> Option<&str> {
    path.rfind('.').map(|i| &path[..i])
}

/// Human-readable type label for a symbol: the declared type verbatim when
/// present, else the resolved value type's display string.
fn type_label(sym: &Symbol) -> String {
    sym.declared_type
        .clone()
        .unwrap_or_else(|| value_type_label(sym.value_type).to_string())
}

/// Render a `ValueType` as a short label. Mirrors the labels used elsewhere in
/// the toolchain (e.g. `m1-doc`).
fn value_type_label(vt: m1_typecheck::ValueType) -> &'static str {
    use m1_typecheck::ValueType;
    match vt {
        ValueType::Boolean => "bool",
        ValueType::Integer => "integer",
        ValueType::Unsigned => "unsigned",
        ValueType::Float => "float",
        ValueType::Enum(_) => "enum",
        ValueType::String => "string",
        ValueType::Unknown => "unknown",
    }
}

/// Build a [`GraphNode`] from a symbol of a known [`NodeKind`], filling in
/// optional structural metadata (unit, type, rate, table dims, parent).
fn build_node(sym: &Symbol, kind: NodeKind) -> GraphNode {
    let mut node = GraphNode::new(sym.path.clone(), kind);
    node.unit = sym.display_unit.clone().or_else(|| sym.unit.clone());
    node.type_label = Some(type_label(sym));
    node.rate_hz = sym.call_rate_hz.or(sym.log_rate_hz);
    node.table_dims = sym.table_meta.as_ref().map(|m| m.axes.len());
    node.parent = parent_path(&sym.path).map(str::to_string);
    node
}

/// Load a project file and build its graph model. Keeps all `m1-typecheck` I/O
/// inside the loader so the rest of the crate stays toolchain-agnostic.
///
/// `project_path` points at the `.m1prj`. `config_path`, when given, is loaded
/// into the project via [`Project::with_config`] so the `.m1cfg`'s table /
/// parameter **shape** (notably `table_meta`, which drives `table_dims`) reaches
/// the graph. Scripts are discovered by walking the project file's parent
/// directory recursively for `*.m1scr` and parsed once via `parse_all`; they are
/// threaded into the model for the (currently no-op) data-flow pass.
pub fn load(
    project_path: &Path,
    config_path: Option<&Path>,
    title: Option<String>,
) -> Result<GraphModel, m1_typecheck::project::LoadError> {
    let mut project = Project::load(project_path)?;

    // Augment the project with the cfg's table/parameter shape if provided. This
    // is what populates `Symbol::table_meta`, used for `table_dims`.
    if let Some(cfg) = config_path {
        project = project.with_config(cfg)?;
    }

    // Discover scripts relative to the project file's directory (mirrors m1-doc
    // and m1-eval's loader), parsing each `.m1scr` exactly once.
    let project_dir = project_path.parent().unwrap_or_else(|| Path::new("."));
    let pairs = collect_scripts(project_dir);
    let scripts = parse_all(&pairs);

    Ok(build_model_with_scripts(&project, &scripts, title))
}

/// Collect every `.m1scr` under `dir` (recursively) as `(basename, source)`
/// pairs, sorted deterministically by basename. Sources are lossy-UTF-8 decoded
/// so Windows-1252 exports do not abort discovery. Ported from
/// `m1-eval/src/loader.rs` (same project, GPL-3.0-or-later).
fn collect_scripts(dir: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    collect_scripts_rec(dir, &mut out);
    // Deterministic order: sort by basename so the graph (and any traces) are
    // reproducible regardless of filesystem enumeration order.
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn collect_scripts_rec(dir: &Path, out: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_scripts_rec(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("m1scr") {
            let Some(name) = path.file_name().and_then(|n| n.to_str()).map(str::to_string) else {
                continue;
            };
            let bytes = std::fs::read(&path).unwrap_or_default();
            let source = String::from_utf8_lossy(&bytes).into_owned();
            out.push((name, source));
        }
    }
}

/// Build the structural graph model from an already-loaded project, with no
/// discovered scripts. Retained for callers/tests that only have a `Project`;
/// delegates to [`build_model_with_scripts`] with an empty slice.
pub fn build_model(project: &Project, title: Option<String>) -> GraphModel {
    build_model_with_scripts(project, &[], title)
}

/// Build the structural graph model from an already-loaded project and its
/// parsed scripts. The scripts feed the data-flow pass (currently a no-op).
pub fn build_model_with_scripts(
    project: &Project,
    scripts: &[ParsedScript],
    title: Option<String>,
) -> GraphModel {
    let mut model = GraphModel::new(title);

    // 1. Nodes from symbols, plus the set of group paths we've seen explicitly.
    let mut node_ids: BTreeSet<String> = BTreeSet::new();
    let mut group_paths: BTreeSet<String> = BTreeSet::new();
    for sym in project.symbols().iter() {
        if let Some(kind) = node_kind(sym.kind) {
            if kind == NodeKind::Group {
                group_paths.insert(sym.path.clone());
            }
            node_ids.insert(sym.path.clone());
            model.nodes.push(build_node(sym, kind));
        }
    }

    // 2. Synthesize any ancestor groups implied by dotted paths that were not
    //    declared explicitly, so the hierarchy is connected up to the roots.
    synthesize_ancestor_groups(&mut model, &mut node_ids, &mut group_paths);

    // 3. Hierarchy edges from dotted-path containment (parent -> child), only
    //    where both endpoints exist as nodes.
    add_hierarchy_edges(&mut model, &node_ids);

    // 4. Table-axis edges: link a table's members (nested under its path) to the
    //    table node.
    add_table_axis_edges(project, &mut model);

    // 5. Schedule edges: connect rated functions/channels to a synthetic clock
    //    node for their rate.
    add_schedule_edges(&mut model);

    // 6. Data-flow edges (stubbed — see the function).
    add_data_flow_edges(project, scripts, &mut model);

    sort_for_determinism(&mut model);
    model
}

/// Add `Group` nodes for any ancestor path implied by an existing node's dotted
/// path but not declared as its own symbol. Records them in `node_ids` /
/// `group_paths` so later edge passes can wire them up.
fn synthesize_ancestor_groups(
    model: &mut GraphModel,
    node_ids: &mut BTreeSet<String>,
    group_paths: &mut BTreeSet<String>,
) {
    // Collect every ancestor path of every current node id.
    let mut needed: BTreeSet<String> = BTreeSet::new();
    for id in node_ids.iter() {
        let mut p = id.as_str();
        while let Some(parent) = parent_path(p) {
            needed.insert(parent.to_string());
            p = parent;
        }
    }
    for path in needed {
        if node_ids.contains(&path) {
            continue;
        }
        let mut node = GraphNode::new(path.clone(), NodeKind::Group);
        node.parent = parent_path(&path).map(str::to_string);
        model.nodes.push(node);
        group_paths.insert(path.clone());
        node_ids.insert(path);
    }
}

/// Emit a [`EdgeKind::Hierarchy`] edge from each node's parent to the node,
/// where both endpoints are present in the graph.
fn add_hierarchy_edges(model: &mut GraphModel, node_ids: &BTreeSet<String>) {
    let mut edges = Vec::new();
    for node in &model.nodes {
        if let Some(parent) = &node.parent
            && node_ids.contains(parent)
        {
            edges.push(GraphEdge::new(
                parent.clone(),
                node.id.clone(),
                EdgeKind::Hierarchy,
            ));
        }
    }
    model.edges.extend(edges);
}

/// Emit [`EdgeKind::TableAxis`] edges linking a table's auto-created members
/// (any node whose path is nested directly or indirectly under the table's path,
/// e.g. its `.Value` output channel and axis channels) to the table node.
///
/// `Symbol.table_meta` carries the axis count/units; we use its presence to mark
/// the table and record dimensionality on the node (done in [`build_node`]). The
/// concrete axis *channels* live as members under the table path in the symbol
/// table, so containment gives us the links without needing the raw breakpoints.
fn add_table_axis_edges(project: &Project, model: &mut GraphModel) {
    // Paths of all table nodes.
    let table_paths: BTreeSet<String> = project
        .symbols()
        .iter()
        .filter(|s| matches!(s.kind, SymbolKind::Table))
        .map(|s| s.path.clone())
        .collect();
    if table_paths.is_empty() {
        return;
    }
    let node_ids: BTreeSet<String> = model.nodes.iter().map(|n| n.id.clone()).collect();
    let mut edges = Vec::new();
    for table in &table_paths {
        let prefix = format!("{table}.");
        for node in &model.nodes {
            // A member of this table (nested under it) that is itself a node.
            if node.id.starts_with(&prefix) && node.id != *table && node_ids.contains(&node.id) {
                edges.push(GraphEdge::new(
                    table.clone(),
                    node.id.clone(),
                    EdgeKind::TableAxis,
                ));
            }
        }
    }
    model.edges.extend(edges);
}

/// Emit [`EdgeKind::Schedule`] edges. For every node that carries a rate
/// (`rate_hz`), synthesize a clock node `Clock@<rate>Hz` and connect the clock
/// to the rated node. This models "this node runs at this rate".
fn add_schedule_edges(model: &mut GraphModel) {
    // Gather (clock_id, rate) for every rated node.
    let mut clocks: BTreeMap<String, f64> = BTreeMap::new();
    let mut links: Vec<(String, String)> = Vec::new();
    for node in &model.nodes {
        if let Some(rate) = node.rate_hz {
            let clock_id = format!("Clock@{}Hz", format_rate(rate));
            clocks.entry(clock_id.clone()).or_insert(rate);
            links.push((clock_id, node.id.clone()));
        }
    }
    // Add synthetic clock nodes.
    for (clock_id, rate) in clocks {
        let mut node = GraphNode::new(clock_id, NodeKind::Group);
        node.rate_hz = Some(rate);
        node.parent = None;
        model.nodes.push(node);
    }
    for (clock_id, target) in links {
        model
            .edges
            .push(GraphEdge::new(clock_id, target, EdgeKind::Schedule));
    }
}

/// Format a rate for a clock-node id without a trailing `.0` on whole numbers.
fn format_rate(rate: f64) -> String {
    if rate.fract() == 0.0 {
        format!("{}", rate as i64)
    } else {
        format!("{rate}")
    }
}

/// **STUB.** Add [`EdgeKind::DataFlow`] edges (a script reads one symbol and
/// writes another).
///
/// TODO(workflow-3): Implement real data-flow extraction. Accurate edges need
/// per-script CST read/write analysis — for each function's `.m1scr`, parse the
/// body with `m1_core::parse`, collect the set of symbols read and the set
/// written, and emit `read -> function` and `function -> written` edges. The
/// full version will reuse `m1-eval`'s read/write *summary* module (built in
/// Workflow 3) rather than re-deriving the analysis here.
///
/// For now this is intentionally a no-op so the structural scaffold compiles and
/// the other three edge types are exercised end to end. The parsed `scripts`
/// slice is already threaded in (M1) ready for the real walker (M2/M3).
fn add_data_flow_edges(_project: &Project, _scripts: &[ParsedScript], _model: &mut GraphModel) {
    // Intentionally empty in the scaffold. See the doc comment above.
}

/// Sort nodes and edges into a deterministic order so DOT / JSON / HTML output
/// is stable across runs.
fn sort_for_determinism(model: &mut GraphModel) {
    model.nodes.sort_by(|a, b| a.id.cmp(&b.id));
    model
        .edges
        .sort_by(|a, b| (a.kind.tag(), &a.from, &a.to).cmp(&(b.kind.tag(), &b.from, &b.to)));
    model.edges.dedup();
}

#[cfg(test)]
mod tests {
    use super::*;

    // A small synthetic project: a group with a channel and a parameter, a
    // function (FuncUser), and a table.
    const PROJECT: &str = r#"<?xml version="1.0"?>
<MoTeCM1BuildSession>
 <Project Name="Demo" TargetHardware="ecu120">
  <ComponentStream><List>
   <Component Classname="BuiltIn.GroupCompound" Name="Root.Engine"/>
   <Component Classname="BuiltIn.Channel" Name="Root.Engine.Speed"><Props Type="f32"><Locale><Default Unit="rpm"/></Locale></Props></Component>
   <Component Classname="BuiltIn.Parameter" Name="Root.Engine.Gain.Value"><Props Type="u16" Security="Tune"/></Component>
   <Component Classname="BuiltIn.Constant" Name="Root.Engine.MaxRpm"><Props Type="u16"/></Component>
  </List></ComponentStream>
 </Project>
</MoTeCM1BuildSession>"#;

    #[test]
    fn parent_path_strips_final_segment() {
        assert_eq!(parent_path("Root.Engine.Speed"), Some("Root.Engine"));
        assert_eq!(parent_path("Root"), None);
    }

    #[test]
    fn builds_nodes_for_each_symbol_kind() {
        let project = Project::from_xml(PROJECT).unwrap();
        let model = build_model(&project, Some("Demo".into()));
        assert!(
            model.nodes.iter().any(|n| n.id == "Root.Engine.Speed"
                && n.kind == NodeKind::Channel
                && n.unit.as_deref() == Some("rpm")),
            "channel node with unit; got {:?}",
            model.nodes
        );
        assert!(
            model.nodes.iter().any(|n| n.kind == NodeKind::Constant),
            "constant node; got {:?}",
            model.nodes
        );
        // The Engine group node exists (declared).
        assert!(
            model
                .nodes
                .iter()
                .any(|n| n.id == "Root.Engine" && n.kind == NodeKind::Group),
            "engine group node; got {:?}",
            model.nodes
        );
    }

    #[test]
    fn synthesizes_root_ancestor_group() {
        // `Root` is never declared, but `Root.Engine` implies it — it should be
        // synthesized as a Group node.
        let project = Project::from_xml(PROJECT).unwrap();
        let model = build_model(&project, None);
        assert!(
            model
                .nodes
                .iter()
                .any(|n| n.id == "Root" && n.kind == NodeKind::Group),
            "synthesized Root group; got {:?}",
            model.nodes
        );
    }

    #[test]
    fn hierarchy_edges_connect_parent_to_child() {
        let project = Project::from_xml(PROJECT).unwrap();
        let model = build_model(&project, None);
        assert!(
            model.edges.iter().any(|e| e.from == "Root.Engine"
                && e.to == "Root.Engine.Speed"
                && e.kind == EdgeKind::Hierarchy),
            "Root.Engine -> Root.Engine.Speed hierarchy edge; got {:?}",
            model.edges
        );
        assert!(
            model.edges.iter().any(|e| e.from == "Root"
                && e.to == "Root.Engine"
                && e.kind == EdgeKind::Hierarchy),
            "Root -> Root.Engine hierarchy edge; got {:?}",
            model.edges
        );
    }

    #[test]
    fn nonempty_graph_has_nodes_and_edges() {
        let project = Project::from_xml(PROJECT).unwrap();
        let model = build_model(&project, None);
        assert!(model.nodes.len() > 1, "expected several nodes");
        assert!(
            model.edge_count(EdgeKind::Hierarchy) > 0,
            "expected hierarchy edges"
        );
    }

    // A project declaring a table symbol, plus a `.m1cfg` giving it a 2-D shape.
    // Real `.m1cfg` exports drop the implicit `Root.` prefix the symbol table
    // keys use, so the cfg names the table `Demo.Map` and m1-typecheck resolves
    // it back onto `Root.Demo.Map`.
    const TABLE_PROJECT: &str = r#"<?xml version="1.0"?>
<MoTeCM1BuildSession>
 <Project Name="Demo" TargetHardware="ecu120">
  <ComponentStream><List>
   <Component Classname="BuiltIn.GroupCompound" Name="Root.Demo"/>
   <Component Classname="BuiltIn.Table" Name="Root.Demo.Map"><Props Type="f32"/></Component>
  </List></ComponentStream>
 </Project>
</MoTeCM1BuildSession>"#;

    const TABLE_CONFIG: &str = r#"<?xml version="1.0"?>
<Configuration Locale="English_Australia.1252" DefaultLocale="C">
 <Group Name="">
  <Table Name="Demo.Map">
   <X><Cells Type="f32" Unit="rpm"><Cell>0</Cell><Cell>100</Cell></Cells></X>
   <Y><Cells Type="f32" Unit="%"><Cell>0</Cell><Cell>1</Cell></Cells></Y>
   <Body><Cells Type="f32"><Cell>10</Cell><Cell>20</Cell><Cell>30</Cell><Cell>40</Cell></Cells></Body>
  </Table>
 </Group>
</Configuration>"#;

    #[test]
    fn load_with_config_populates_table_meta() {
        // Write the project + cfg to a temp dir and load via the public `load`
        // entry point, which threads the cfg into the project's symbol table.
        let dir = tempfile::tempdir().expect("temp dir");
        let prj = dir.path().join("Project.m1prj");
        let cfg = dir.path().join("parameters.m1cfg");
        std::fs::write(&prj, TABLE_PROJECT).expect("write project");
        std::fs::write(&cfg, TABLE_CONFIG).expect("write config");

        let model = load(&prj, Some(&cfg), Some("Demo".into())).expect("project should load");

        let table = model
            .nodes
            .iter()
            .find(|n| n.id == "Root.Demo.Map")
            .expect("table node present");
        assert_eq!(
            table.kind,
            NodeKind::Table,
            "Root.Demo.Map should be a table node"
        );
        // The 2-D cfg shape (X + Y axes) must reach the node as table_dims == 2.
        assert_eq!(
            table.table_dims,
            Some(2),
            "2-D table from .m1cfg should give table_dims == Some(2); got {:?}",
            table.table_dims
        );
    }

    #[test]
    fn collect_scripts_finds_m1scr() {
        // A project dir with one `.m1scr` should yield exactly one parsed script
        // with the right name and a non-empty source.
        let dir = tempfile::tempdir().expect("temp dir");
        let prj = dir.path().join("Project.m1prj");
        std::fs::write(&prj, TABLE_PROJECT).expect("write project");
        let scr = dir.path().join("Update.m1scr");
        std::fs::write(&scr, "Output = 1;\n").expect("write script");

        let pairs = collect_scripts(dir.path());
        assert_eq!(pairs.len(), 1, "exactly one .m1scr discovered; got {pairs:?}");
        assert_eq!(pairs[0].0, "Update.m1scr", "discovered by basename");
        assert!(!pairs[0].1.is_empty(), "source should be non-empty");

        // And the same discovery, parsed, surfaces through the public loader.
        let scripts = parse_all(&pairs);
        assert_eq!(scripts.len(), 1, "one parsed script");
        assert_eq!(scripts[0].name, "Update.m1scr");
        assert!(
            !scripts[0].cst.source().is_empty(),
            "parsed CST should retain its source"
        );
    }
}
