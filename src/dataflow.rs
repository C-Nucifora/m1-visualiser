// SPDX-License-Identifier: GPL-3.0-or-later
//! Per-script read/write extraction — the data-flow walker.
//!
//! For every function-backing `.m1scr`, this derives the set of project symbols
//! the script *writes* and the set it *reads*, so the loader can emit data-flow
//! edges (`read -> function`, `function -> write`).
//!
//! Names resolve through `m1-typecheck`'s project resolver, including locals,
//! library precedence and This/Parent anchors. Assignment targets are writes;
//! compound targets are also reads. Call arguments remain reads, and a resolved
//! project Function/Method contributes a read when its return value is used.
//! Builtin callees and discarded call results do not add return dependencies.
//! This does not model the activation or ordering of side-effect-only calls.
//!
//! Identifiers may contain spaces; paths are only ever split on `.`.

use m1_core::{Field, Kind, Node};
use m1_typecheck::Project;
use m1_typecheck::ValueType;
use m1_typecheck::parsed::ParsedScript;
use m1_typecheck::resolve::{Resolution, Scope, resolve};
use m1_typecheck::symbols::SymbolKind;
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
    /// Walk statements in source order without recursing over script nesting.
    fn walk(&mut self, node: &Node) {
        let mut pending = vec![*node];
        while let Some(node) = pending.pop() {
            match node.kind() {
                Kind::LocalDeclaration => self.walk_local_decl(&node),
                Kind::AssignmentStatement => self.walk_assignment(&node),
                Kind::Identifier | Kind::MemberExpression | Kind::CallExpression => {
                    self.walk_reads(&node)
                }
                _ => pending.extend(node.named_children().into_iter().rev()),
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
    /// other nodes add their children to the work stack for nested operands.
    fn walk_reads(&mut self, node: &Node) {
        let mut pending = vec![*node];
        while let Some(node) = pending.pop() {
            match node.kind() {
                Kind::Identifier | Kind::MemberExpression => {
                    // Resolve the whole member chain, not its individual segments.
                    if let Some(path) = self.canonical_symbol(&node) {
                        self.sets.reads.insert(path);
                    }
                }
                Kind::CallExpression => {
                    // A used project return value depends on its callee. Standalone
                    // calls need an effects model, which this read/write pass lacks.
                    if return_value_is_used(&node)
                        && let Some(callee) = node.child_by_field(Field::Function)
                        && let Some(path) = self.canonical_symbol(&callee)
                        && self.project.symbols().get(&path).is_some_and(|sym| {
                            matches!(sym.kind, SymbolKind::Function | SymbolKind::Method)
                        })
                    {
                        self.sets.reads.insert(path);
                    }
                    if let Some(args) = node.child_by_field(Field::Arguments) {
                        pending.push(args);
                    }
                }
                _ => pending.extend(node.named_children().into_iter().rev()),
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
        let scope = Scope {
            locals: self
                .locals
                .iter()
                .map(|name| (name.clone(), ValueType::Unknown))
                .collect(),
            group: self.group.map(str::to_string),
            project: Some(self.project),
            fn_symbol: self.fn_symbol.map(str::to_string),
        };
        match resolve(&raw, &scope) {
            Resolution::Symbol(sym) => Some(sym.path.clone()),
            _ => None,
        }
    }
}

fn return_value_is_used(node: &Node) -> bool {
    let mut parent = node.parent();
    while let Some(node) = parent {
        if node.kind() != Kind::ParenthesizedExpression {
            return node.kind() != Kind::ExpressionStatement;
        }
        parent = node.parent();
    }
    false
}

/// Flatten a member chain without recursing over its depth. Each segment is
/// taken verbatim, including spaces; only `.` joins segments.
fn flatten_member(node: &Node) -> Option<String> {
    let mut current = *node;
    let mut tail = Vec::new();
    while current.kind() == Kind::MemberExpression {
        tail.push(current.child_by_field(Field::Property)?.text());
        current = current.child_by_field(Field::Object)?;
    }
    let mut path = current.text().to_string();
    for segment in tail.into_iter().rev() {
        path.push('.');
        path.push_str(segment);
    }
    Some(path)
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
    fn project_return_calls_resolve_without_builtin_or_effect_guesses() {
        let extra = r#"
   <Component Classname="BuiltIn.FuncUserParam" Filename="Demo.Helper.m1scr" Name="Root.Demo.Helper"><Signature ReturnType="f32"><Params/></Signature></Component>
   <Component Classname="BuiltIn.MethodUser" Filename="Demo.With Space.m1scr" Name="Root.Demo.With Space"><Signature ReturnType="f32"><Params/></Signature></Component>
   <Component Classname="BuiltIn.FuncUserParam" Filename="Demo.Calculate.Max.m1scr" Name="Root.Demo.Calculate.Max"><Signature ReturnType="f32"><Params/></Signature></Component>
"#;
        let project =
            Project::from_xml(&MINI_PROJECT.replace("</List>", &format!("{extra}</List>")))
                .unwrap();
        for name in [
            "Helper",
            "This.Helper",
            "Root.Demo.Helper",
            "Demo.Helper",
            "Parent.Demo.Helper",
        ] {
            let sets = io_sets(
                &script_from(&format!("Output = {name}(Speed);")),
                &project,
                Some("Root.Demo"),
            );
            assert_eq!(
                sets.reads,
                BTreeSet::from(["Root.Demo.Helper".into(), "Root.Demo.Speed".into()]),
                "{name}"
            );
        }
        let sets = io_sets(
            &script_from("Output = This.With Space(Speed);"),
            &project,
            Some("Root.Demo"),
        );
        assert!(sets.reads.contains("Root.Demo.With Space"));
        for body in [
            "Helper(Speed);",
            "(Helper(Speed));",
            "Output = Calculate.Max(Speed, Gain);",
            "Output = Library.Calculate.Max(Speed, Gain);",
            "local Helper = 0; Output = Helper(Speed);",
        ] {
            let sets = io_sets(&script_from(body), &project, Some("Root.Demo"));
            assert_eq!(
                sets.reads
                    .iter()
                    .filter(|p| p.contains("Helper") || p.contains("Calculate"))
                    .count(),
                0,
                "{body}: {sets:?}"
            );
            assert!(sets.reads.contains("Root.Demo.Speed"));
        }
        // The project anchor disambiguates a project member from the library.
        let sets = io_sets(
            &script_from("Output = This.Calculate.Max(Speed);"),
            &project,
            Some("Root.Demo"),
        );
        assert!(sets.reads.contains("Root.Demo.Calculate.Max"));
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
