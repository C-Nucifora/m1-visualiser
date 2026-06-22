// SPDX-License-Identifier: GPL-3.0-or-later
//! Per-script read/write extraction — the data-flow walker.
//!
//! For every function-backing `.m1scr`, this derives the set of project symbols
//! the script *writes* and the set it *reads*, so the loader can emit data-flow
//! edges (`read -> function`, `function -> write`).
//!
//! This is a port of `m1-eval/src/summary.rs` (`io_sets`) and the small helpers
//! it depends on — `classify` (from `m1-eval/src/ident.rs`), `flatten_member`
//! and `rewrite_this` (from `m1-eval/src/expr.rs`). Both crates are ours and
//! GPL-3.0-or-later, so copying our own code is fine; we copy rather than depend
//! on `m1-eval` so the structural crate does **not** take an `m1-eval`
//! dependency (the design doc forbids that until the value-overlay workflow).
//!
//! Divergences from the originals, all behaviour-preserving for read/write
//! extraction:
//!
//! - `m1-eval`'s `Walker.locals` is a `HashMap<String, Value>`; the values are
//!   only ever used as a *set of local names*, never read, so here it is a
//!   `HashSet<String>`. `classify` builds its `Scope.locals` with
//!   `ValueType::Unknown` for each name, exactly as `ident.rs` does.
//! - `flatten_member` / `rewrite_this` returned `Result<_, EvalError>` /
//!   `Option` in `expr.rs`; here both collapse to `Option` since the walker only
//!   needs the happy path (a malformed member just yields no read).
//!
//! The extraction rules (verified against `summary.rs`):
//!
//! - the left-hand side of an `AssignmentStatement` is a **write**;
//! - a compound assignment (`+=`, `*=`, …) reads its target first, so a compound
//!   target is **both** a read and a write;
//! - every other identifier/member reference in a value position is a **read**;
//! - a `CallExpression` callee (`Calculate.Max`) is **not** a read, but its
//!   arguments are walked for reads.
//!
//! Only *project symbols* land in the sets — function-locals, builtin library
//! objects, and the `In`/`Out`/`This` anchors are excluded. Names are
//! canonicalised through `classify` (and a leading `This` is rewritten to the
//! enclosing group first), so `Speed`, `This.Speed`, and `Root.Demo.Speed` all
//! collapse to one path.
//!
//! Identifiers may contain spaces; paths are only ever split on `.`.

use m1_core::{Field, Kind, Node};
use m1_typecheck::Project;
use m1_typecheck::ValueType;
use m1_typecheck::parsed::ParsedScript;
use m1_typecheck::resolve::{Resolution, Scope, resolve};
use std::collections::{BTreeSet, HashSet};

/// The canonical read/write sets of one function's body.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IoSets {
    /// Canonical paths of project symbols this function assigns to.
    pub writes: BTreeSet<String>,
    /// Canonical paths of project symbols this function reads.
    pub reads: BTreeSet<String>,
}

/// Collect the read/write sets of `script`'s body.
///
/// `group` is the enclosing group's canonical path (for group-relative name
/// resolution); the function symbol the script backs is looked up from the
/// project by the script's file name, so `In.*` references canonicalise too.
pub fn io_sets(script: &ParsedScript, project: &Project, group: Option<&str>) -> IoSets {
    let fn_symbol = project.function_symbol_for_script(&script.name);
    let mut walker = Walker {
        project,
        group,
        fn_symbol: fn_symbol.as_deref(),
        // Local variable names in scope; a declared local shadows project lookup,
        // so we track them to exclude from the dependency sets.
        locals: HashSet::new(),
        sets: IoSets::default(),
    };
    walker.walk(&script.cst.root());
    walker.sets
}

/// Carries the resolution context while walking one function body.
struct Walker<'a> {
    project: &'a Project,
    group: Option<&'a str>,
    fn_symbol: Option<&'a str>,
    locals: HashSet<String>,
    sets: IoSets,
}

impl Walker<'_> {
    /// Walk a node, dispatching assignments specially and recursing elsewhere.
    fn walk(&mut self, node: &Node) {
        match node.kind() {
            Kind::LocalDeclaration => self.walk_local_decl(node),
            Kind::AssignmentStatement => self.walk_assignment(node),
            _ => {
                for child in node.named_children() {
                    self.walk(&child);
                }
            }
        }
    }

    /// A `local`/`static local` declaration introduces a local name (shadowing
    /// project symbols) and reads its initialiser, if any.
    fn walk_local_decl(&mut self, node: &Node) {
        if let Some(name) = node.child_by_field(Field::Name) {
            // Register the local so later references to it are not mistaken for a
            // project channel read.
            self.locals.insert(name.text().to_string());
        }
        if let Some(init) = node.child_by_field(Field::Value) {
            self.walk_reads(&init);
        }
    }

    /// An assignment: the target is a write (and also a read for a compound
    /// assignment), and the value expression is read.
    fn walk_assignment(&mut self, node: &Node) {
        let target = node.child_by_field(Field::Target);
        let value = node.child_by_field(Field::Value);
        let op = node.child_by_field(Field::Operator);
        let compound = op
            .map(|o| m1_core::is_compound_assign(o.kind()))
            .unwrap_or(false);

        if let Some(target) = &target {
            // Resolve the target path to a canonical symbol; locals are not deps.
            if let Some(path) = self.canonical_symbol(target) {
                self.sets.writes.insert(path.clone());
                if compound {
                    // A compound assignment reads the target before writing it.
                    self.sets.reads.insert(path);
                }
            }
        }
        if let Some(value) = &value {
            self.walk_reads(value);
        }
    }

    /// Walk an expression position, recording each project-symbol reference as a
    /// read. Member expressions are flattened to a path and resolved as a unit;
    /// other nodes recurse so nested calls/operands are covered.
    fn walk_reads(&mut self, node: &Node) {
        match node.kind() {
            Kind::Identifier => {
                if let Some(path) = self.canonical_symbol(node) {
                    self.sets.reads.insert(path);
                }
            }
            Kind::MemberExpression => {
                // A member chain like `A.B.C` is one reference. If its head is a
                // builtin object (e.g. `Calculate.PI`) or it does not resolve to a
                // project symbol, `canonical_symbol` returns None and we skip it.
                if let Some(path) = self.canonical_symbol(node) {
                    self.sets.reads.insert(path);
                }
                // Do not recurse into the member's segments — they are not
                // independent references. (A `MemberExpression` used as a call
                // *callee* is handled by the CallExpression arm below, which only
                // walks the argument list.)
            }
            Kind::CallExpression => {
                // The callee is `Object.Method` (a builtin or table); we do not
                // count it as a channel read. The arguments, however, are reads.
                if let Some(args) = node.child_by_field(Field::Arguments) {
                    self.walk_reads(&args);
                }
            }
            _ => {
                for child in node.named_children() {
                    self.walk_reads(&child);
                }
            }
        }
    }

    /// Canonicalise an identifier/member node to a project-symbol path, or `None`
    /// when it is a local, a builtin object, or unresolved (none of which is a
    /// cross-function channel dependency).
    fn canonical_symbol(&self, node: &Node) -> Option<String> {
        let raw = match node.kind() {
            Kind::Identifier => node.text().to_string(),
            Kind::MemberExpression => flatten_member(node)?,
            _ => return None,
        };
        // Expand a `This` anchor to the enclosing group before resolution, exactly
        // as the evaluator does.
        let rewritten = rewrite_this(&raw, self.group);
        let path = rewritten.as_deref().unwrap_or(&raw);
        match classify(path, self.group, self.fn_symbol, self.project, &self.locals) {
            Target::Symbol(p) => Some(p),
            // Locals, builtins, and unresolved anchors are not project deps.
            Target::Local(_) | Target::Builtin { .. } | Target::Unresolved => None,
        }
    }
}

/// What an identifier (or dotted path) denotes. A `Value`-free port of
/// `m1-eval/src/ident.rs`'s `Target` — the walker only distinguishes "is it a
/// project symbol" from "is it a local/builtin/miss", so no runtime value is
/// carried.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    /// A project channel/parameter/constant/table/function, by its single
    /// canonical symbol-table path (e.g. `"Root.Demo.Speed"`).
    Symbol(String),
    /// A function-local variable, by its name.
    Local(String),
    /// A builtin library object (e.g. `Calculate`). `object` is the library
    /// object name.
    Builtin { object: String },
    /// Nothing in the project, locals, or the builtin library matches.
    Unresolved,
}

/// The leading dot-segment of a path (`"Calculate"` of `"Calculate.Max"`). Never
/// splits on whitespace — only on `.`.
fn root_segment(path: &str) -> &str {
    match path.find('.') {
        Some(i) => &path[..i],
        None => path,
    }
}

/// Classify an identifier/path against the project, the enclosing group, the
/// backing function symbol, and the current function locals. Ported from
/// `m1-eval/src/ident.rs::classify`, with `locals` as a name set (their runtime
/// values are irrelevant to resolution, so they resolve as `ValueType::Unknown`).
fn classify(
    name: &str,
    group: Option<&str>,
    fn_symbol: Option<&str>,
    project: &Project,
    locals: &HashSet<String>,
) -> Target {
    let scope = Scope {
        locals: locals
            .iter()
            .map(|k| (k.clone(), ValueType::Unknown))
            .collect(),
        group: group.map(str::to_string),
        project: Some(project),
        fn_symbol: fn_symbol.map(str::to_string),
    };

    match resolve(name, &scope) {
        Resolution::Symbol(sym) => Target::Symbol(sym.path.clone()),
        Resolution::Local(_) => Target::Local(name.to_string()),
        Resolution::BuiltinObject(obj) => Target::Builtin {
            object: obj.to_string(),
        },
        // A builtin function/method call (`Calculate.Max`). Its object is the
        // leading segment.
        Resolution::BuiltinFn(_) => Target::Builtin {
            object: root_segment(name).to_string(),
        },
        // `Opaque` covers `In`/`Out`/`Parent`/`This`/`Library`/`Root` anchors and
        // accessor calls; `This` is rewritten before this call, so reaching here
        // means "no canonical symbol": treat as unresolved (not a project dep).
        Resolution::Opaque | Resolution::Unresolved => Target::Unresolved,
    }
}

/// Flatten a `MemberExpression` to its dotted source path. The `object` may
/// itself be a member expression (`A.B.C`), so recurse; each segment is taken
/// verbatim (it may contain spaces). Only `.` joins segments — never whitespace.
/// Ported from `m1-eval/src/expr.rs::flatten_member` with its `Result` collapsed
/// to `Option` (a malformed member yields `None`).
fn flatten_member(node: &Node) -> Option<String> {
    let object = node.child_by_field(Field::Object)?;
    let property = node.child_by_field(Field::Property)?;

    let head = match object.kind() {
        Kind::MemberExpression => flatten_member(&object)?,
        // Identifier (or any leaf) — its text is the segment verbatim.
        _ => object.text().to_string(),
    };
    Some(format!("{head}.{}", property.text()))
}

/// Rewrite a leading `This` anchor to the enclosing group's canonical path
/// (`This.Output` from group `Root.Demo` → `Root.Demo.Output`; bare `This` →
/// `Root.Demo`). `resolve` handles the `In`/`Out`/`Parent`/`Root` anchors itself
/// but not `This`, so we expand it here before classification. Only `.` splits
/// segments, never whitespace. Non-`This` paths return `None` (caller falls back
/// to the original). Ported verbatim from `m1-eval/src/expr.rs::rewrite_this`.
fn rewrite_this(path: &str, group: Option<&str>) -> Option<String> {
    let group = group?;
    if path == "This" {
        return Some(group.to_string());
    }
    path.strip_prefix("This.")
        .map(|rest| format!("{group}.{rest}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use m1_typecheck::parsed::parse_all;

    /// A synthetic mini project mirroring the shape of `m1-eval`'s `mini`
    /// fixture: a `Root.Demo` group with `Speed`/`Gain`/`Output` channels and a
    /// `FuncUser` backed by `Demo.Update.m1scr`. Loaded via `from_xml` for speed.
    /// No proprietary content.
    const MINI_PROJECT: &str = r#"<?xml version="1.0"?>
<MoTeCM1BuildSession>
 <Project Name="Mini" TargetHardware="ecu120">
  <ComponentStream><List>
   <Component Classname="BuiltIn.Channel" Name="Root.Sibling"><Props Type="f32"><Locale><Default Unit="rpm"/></Locale></Props></Component>
   <Component Classname="BuiltIn.GroupCompound" Name="Root.Demo"/>
   <Component Classname="BuiltIn.Channel" Name="Root.Demo.Speed"><Props Type="f32"><Locale><Default Unit="rpm"/></Locale></Props></Component>
   <Component Classname="BuiltIn.Channel" Name="Root.Demo.Gain"><Props Type="f32"/></Component>
   <Component Classname="BuiltIn.Channel" Name="Root.Demo.Output"><Props Type="f32"/></Component>
   <Component Classname="BuiltIn.FuncUser" Filename="Demo.Update.m1scr" Name="Root.Demo.Update"/>
  </List></ComponentStream>
 </Project>
</MoTeCM1BuildSession>"#;

    fn mini_project() -> Project {
        Project::from_xml(MINI_PROJECT).expect("mini fixture loads")
    }

    /// Parse a synthetic script body under the `Demo.Update.m1scr` name so it
    /// canonicalises against the fixture's `Root.Demo` group.
    fn script_from(src: &str) -> ParsedScript {
        let pairs = vec![("Demo.Update.m1scr".to_string(), src.to_string())];
        parse_all(&pairs).into_iter().next().unwrap()
    }

    #[test]
    fn assignment_target_is_write_rhs_idents_are_reads() {
        let project = mini_project();
        let script = script_from("Output = Speed * Gain;\n");
        let sets = io_sets(&script, &project, Some("Root.Demo"));

        assert!(sets.writes.contains("Root.Demo.Output"), "{sets:?}");
        assert!(sets.reads.contains("Root.Demo.Speed"), "{sets:?}");
        assert!(sets.reads.contains("Root.Demo.Gain"), "{sets:?}");
        // The write target is not also a read here (plain assignment).
        assert!(!sets.reads.contains("Root.Demo.Output"), "{sets:?}");
    }

    #[test]
    fn compound_assignment_target_is_both_read_and_write() {
        let project = mini_project();
        let script = script_from("Output += Speed;\n");
        let sets = io_sets(&script, &project, Some("Root.Demo"));

        assert!(sets.writes.contains("Root.Demo.Output"), "{sets:?}");
        assert!(sets.reads.contains("Root.Demo.Output"), "{sets:?}");
        assert!(sets.reads.contains("Root.Demo.Speed"), "{sets:?}");
    }

    #[test]
    fn locals_are_not_dependencies() {
        let project = mini_project();
        // `scaled` is a local; only Speed/Gain (reads) and Output (write) are deps.
        let script = script_from("local scaled = Speed * Gain;\nOutput = scaled;\n");
        let sets = io_sets(&script, &project, Some("Root.Demo"));

        assert!(sets.writes.contains("Root.Demo.Output"), "{sets:?}");
        assert!(sets.reads.contains("Root.Demo.Speed"), "{sets:?}");
        assert!(sets.reads.contains("Root.Demo.Gain"), "{sets:?}");
        // `scaled` must not appear as a channel.
        assert!(!sets.reads.iter().any(|r| r.contains("scaled")), "{sets:?}");
        assert!(
            !sets.writes.iter().any(|w| w.contains("scaled")),
            "{sets:?}"
        );
    }

    #[test]
    fn builtin_callee_is_not_a_read_but_args_are() {
        let project = mini_project();
        let script = script_from("Output = Calculate.Max(Speed, Gain);\n");
        let sets = io_sets(&script, &project, Some("Root.Demo"));

        // The Calculate object/method is not a channel.
        assert!(
            !sets.reads.iter().any(|r| r.starts_with("Calculate")),
            "{sets:?}"
        );
        // But the call arguments are reads.
        assert!(sets.reads.contains("Root.Demo.Speed"), "{sets:?}");
        assert!(sets.reads.contains("Root.Demo.Gain"), "{sets:?}");
        assert!(sets.writes.contains("Root.Demo.Output"), "{sets:?}");
    }

    #[test]
    fn this_anchor_rewrites_to_group() {
        let project = mini_project();
        // `This.Speed` read from group `Root.Demo` canonicalises to
        // `Root.Demo.Speed` — the same path a bare `Speed` resolves to.
        let script = script_from("Output = This.Speed;\n");
        let sets = io_sets(&script, &project, Some("Root.Demo"));

        assert!(sets.reads.contains("Root.Demo.Speed"), "{sets:?}");
        assert!(sets.writes.contains("Root.Demo.Output"), "{sets:?}");
    }
}
