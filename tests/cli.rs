// SPDX-License-Identifier: GPL-3.0-or-later
//! CLI integration tests: drive the `m1-visualiser` binary end-to-end with
//! `assert_cmd`, asserting format wiring (`--format dot|json|html`), the
//! `--config` thread-through, default-config discovery, and exit codes
//! (2 = no project, 1 = load/write error).

use assert_cmd::Command;
use std::path::{Path, PathBuf};

/// Path to the hand-authored synthetic fixture project shipped under
/// `tests/fixtures/`.
fn fixture_project() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/Project.m1prj")
}

/// A 2-D table project + its `.m1cfg` (mirrors the loader's
/// `TABLE_WITH_MEMBERS_PROJECT` / `TABLE_CONFIG`). The cfg supplies the table's
/// 2-D shape so the table node records `table_dims == 2` once threaded through.
const TABLE_PROJECT: &str = r#"<?xml version="1.0"?>
<MoTeCM1BuildSession>
 <Project Name="Demo" TargetHardware="ecu120">
  <ComponentStream><List>
   <Component Classname="BuiltIn.GroupCompound" Name="Root.Demo"/>
   <Component Classname="BuiltIn.Table" Name="Root.Demo.Map"><Props Type="f32"/></Component>
   <Component Classname="BuiltIn.Channel" Name="Root.Demo.Map.Value" Caps="AutoCreated"><Props Type="f32"/></Component>
   <Component Classname="BuiltIn.Channel" Name="Root.Demo.Map.X" Caps="AutoCreated"><Props Type="f32"><Locale><Default Unit="rpm"/></Locale></Props></Component>
   <Component Classname="BuiltIn.Channel" Name="Root.Demo.Map.Y" Caps="AutoCreated"><Props Type="f32"><Locale><Default Unit="%"/></Locale></Props></Component>
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

/// True if the JSON graph has a `Root.Demo.Map` table node with `table_dims`.
fn json_has_table_dims(value: &serde_json::Value, id: &str) -> bool {
    value["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|n| n["id"] == id && n["table_dims"].is_number())
}

#[test]
fn cli_writes_html_by_default() {
    let out = tempfile::tempdir().expect("temp dir");
    let html = out.path().join("graph.html");

    Command::cargo_bin("m1-visualiser")
        .expect("binary builds")
        .arg("--project")
        .arg(fixture_project())
        .arg("--out")
        .arg(&html)
        .assert()
        .success();

    let page = std::fs::read_to_string(&html).expect("html written");
    assert!(
        page.contains("cytoscape"),
        "default-format HTML should inline Cytoscape"
    );
    assert!(page.contains("<!DOCTYPE html>"), "HTML preamble present");
}

#[test]
fn cli_dot_and_json_formats() {
    let dir = tempfile::tempdir().expect("temp dir");

    // DOT: writes a well-formed digraph.
    let dot = dir.path().join("graph.dot");
    Command::cargo_bin("m1-visualiser")
        .expect("binary builds")
        .args(["--format", "dot"])
        .arg("--project")
        .arg(fixture_project())
        .arg("--out")
        .arg(&dot)
        .assert()
        .success();
    let dot_text = std::fs::read_to_string(&dot).expect("dot written");
    assert!(
        dot_text.starts_with("digraph m1 {"),
        "DOT header: {:.40}",
        dot_text
    );
    assert!(dot_text.trim_end().ends_with('}'), "DOT closed");

    // JSON: writes a parseable graph that round-trips with nodes + edges.
    let json = dir.path().join("graph.json");
    Command::cargo_bin("m1-visualiser")
        .expect("binary builds")
        .args(["--format", "json"])
        .arg("--project")
        .arg(fixture_project())
        .arg("--out")
        .arg(&json)
        .assert()
        .success();
    let json_text = std::fs::read_to_string(&json).expect("json written");
    let value: serde_json::Value = serde_json::from_str(&json_text).expect("json parses");
    assert!(
        !value["nodes"].as_array().expect("nodes array").is_empty(),
        "JSON graph has nodes"
    );
    assert!(
        !value["edges"].as_array().expect("edges array").is_empty(),
        "JSON graph has edges"
    );
}

#[test]
fn cli_with_config_threads_table_meta() {
    let dir = tempfile::tempdir().expect("temp dir");
    let prj = dir.path().join("Project.m1prj");
    let cfg = dir.path().join("parameters.m1cfg");
    std::fs::write(&prj, TABLE_PROJECT).expect("write project");
    std::fs::write(&cfg, TABLE_CONFIG).expect("write config");

    let json = dir.path().join("graph.json");
    Command::cargo_bin("m1-visualiser")
        .expect("binary builds")
        .args(["--format", "json"])
        .arg("--project")
        .arg(&prj)
        .arg("--config")
        .arg(&cfg)
        .arg("--out")
        .arg(&json)
        .assert()
        .success();

    let json_text = std::fs::read_to_string(&json).expect("json written");
    let value: serde_json::Value = serde_json::from_str(&json_text).expect("json parses");
    assert!(
        json_has_table_dims(&value, "Root.Demo.Map"),
        "explicit --config should thread table_meta so the table node carries \
         table_dims; nodes = {}",
        value["nodes"]
    );
}

#[test]
fn cli_discovers_sibling_config_by_default() {
    // With exactly one sibling `*.m1cfg` next to the project and no explicit
    // `--config`, the CLI discovers it and threads its table shape through.
    let dir = tempfile::tempdir().expect("temp dir");
    let prj = dir.path().join("Project.m1prj");
    let cfg = dir.path().join("parameters.m1cfg");
    std::fs::write(&prj, TABLE_PROJECT).expect("write project");
    std::fs::write(&cfg, TABLE_CONFIG).expect("write config");

    let json = dir.path().join("graph.json");
    Command::cargo_bin("m1-visualiser")
        .expect("binary builds")
        .args(["--format", "json"])
        .arg("--project")
        .arg(&prj)
        .arg("--out")
        .arg(&json)
        .assert()
        .success();

    let json_text = std::fs::read_to_string(&json).expect("json written");
    let value: serde_json::Value = serde_json::from_str(&json_text).expect("json parses");
    assert!(
        json_has_table_dims(&value, "Root.Demo.Map"),
        "a single sibling .m1cfg should be auto-discovered; nodes = {}",
        value["nodes"]
    );
}

#[test]
fn cli_ambiguous_sibling_configs_are_ignored() {
    // Two sibling `.m1cfg` files are ambiguous: discovery must back off to none
    // rather than guess, so the run still succeeds but no table_meta is applied.
    let dir = tempfile::tempdir().expect("temp dir");
    let prj = dir.path().join("Project.m1prj");
    std::fs::write(&prj, TABLE_PROJECT).expect("write project");
    std::fs::write(dir.path().join("a.m1cfg"), TABLE_CONFIG).expect("write config a");
    std::fs::write(dir.path().join("b.m1cfg"), TABLE_CONFIG).expect("write config b");

    let json = dir.path().join("graph.json");
    Command::cargo_bin("m1-visualiser")
        .expect("binary builds")
        .args(["--format", "json"])
        .arg("--project")
        .arg(&prj)
        .arg("--out")
        .arg(&json)
        .assert()
        .success();

    let json_text = std::fs::read_to_string(&json).expect("json written");
    let value: serde_json::Value = serde_json::from_str(&json_text).expect("json parses");
    assert!(
        !json_has_table_dims(&value, "Root.Demo.Map"),
        "ambiguous sibling configs must not be guessed; nodes = {}",
        value["nodes"]
    );
}

#[test]
fn cli_missing_project_exits_nonzero() {
    // A non-existent project path that the resolver cannot find anywhere must
    // exit 2 (no project) with a diagnostic, and must not auto-discover an
    // ambient project from the cwd.
    let empty = tempfile::tempdir().expect("temp dir");

    Command::cargo_bin("m1-visualiser")
        .expect("binary builds")
        .current_dir(empty.path())
        .env_remove("M1_PROJECT")
        .assert()
        .failure()
        .code(2);
}

#[test]
fn cli_unreadable_project_exits_one() {
    // A `--project` that points at a path which is not a loadable project must
    // exit 1 (load error), distinct from the "no project" exit 2.
    let dir = tempfile::tempdir().expect("temp dir");
    let bogus = dir.path().join("Project.m1prj");
    std::fs::write(&bogus, "not valid m1 project xml").expect("write bogus project");

    Command::cargo_bin("m1-visualiser")
        .expect("binary builds")
        .arg("--project")
        .arg(&bogus)
        .assert()
        .failure()
        .code(1);
}

/// Sanity guard: the fixture project the simple tests rely on exists.
#[test]
fn fixture_project_exists() {
    assert!(
        Path::new(&fixture_project()).is_file(),
        "expected fixture at {}",
        fixture_project().display()
    );
}
