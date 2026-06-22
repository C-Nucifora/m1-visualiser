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

/// Path to the overlay fixture project (a `Root.Demo` group with
/// `Speed`/`Gain`/`Output` and an `Update` function computing
/// `Output = Speed * Gain`) used by the value/diff overlay CLI tests.
fn overlay_project() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/overlay/Project.m1prj")
}

/// A `function` scenario over `Demo.Update` with Speed=20, Gain=2.5, so
/// `Output = 50.0` every tick — a known value for the overlay assertions.
const OVERLAY_SCENARIO_TOML: &str = r#"
mode = "function"
target = "Demo.Update"
duration_s = 0.03
base_rate_hz = 100.0

[[inputs]]
channel = "Root.Demo.Speed"
const = 20.0

[[inputs]]
channel = "Root.Demo.Gain"
const = 2.5
"#;

/// A `time`-first CSV log consistent with Gain=2.5 (`Output = Speed * 2.5`).
const OVERLAY_LOG_CSV: &str = "time,Root.Demo.Speed,Root.Demo.Output\n\
                               0.00,20,50\n\
                               0.01,20,50\n";

/// Read the GraphModel JSON embedded in a rendered HTML page (the renderer
/// substitutes it into the `var GRAPH = <json>;` literal), so a test can assert
/// on the overlay the viewer will read. Returns the parsed JSON document.
fn embedded_graph_json(html: &str) -> serde_json::Value {
    // The template embeds the model as `var GRAPH = <compact json>;`. The compact
    // JSON has no internal `;`, so slicing the assignment to its terminating `;`
    // recovers exactly the document (matches `html.rs`'s own embedding guard).
    let assign = html
        .split("\nvar GRAPH =")
        .nth(1)
        .expect("page embeds a GRAPH assignment");
    let literal = assign
        .split(';')
        .next()
        .expect("GRAPH assignment terminated");
    serde_json::from_str(literal.trim()).expect("embedded GRAPH JSON parses")
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

// ---- O6: opt-in overlay flags, back-compatible ----

#[test]
fn cli_without_overlay_is_unchanged() {
    // With no overlay flag the binary is the v1 tool: the HTML it writes carries
    // no `"overlay"` in its embedded GraphModel JSON (the back-compat invariant).
    let dir = tempfile::tempdir().expect("temp dir");
    let html = dir.path().join("graph.html");

    Command::cargo_bin("m1-visualiser")
        .expect("binary builds")
        .arg("--project")
        .arg(overlay_project())
        .arg("--out")
        .arg(&html)
        .assert()
        .success();

    let page = std::fs::read_to_string(&html).expect("html written");
    let graph = embedded_graph_json(&page);
    assert!(
        graph.get("overlay").is_none(),
        "no overlay flag must embed no overlay; got {graph}"
    );
}

#[test]
fn cli_overlay_scenario_embeds_values() {
    // `--overlay-scenario <scn.toml>` runs a VALUE overlay and embeds it: the
    // page's GraphModel JSON gains `"overlay"` with `"kind":"value"` and a node
    // series carrying the known computed value (Output = 20 * 2.5 = 50).
    let dir = tempfile::tempdir().expect("temp dir");
    let scn = dir.path().join("scenario.toml");
    std::fs::write(&scn, OVERLAY_SCENARIO_TOML).expect("write scenario");
    let html = dir.path().join("graph.html");

    Command::cargo_bin("m1-visualiser")
        .expect("binary builds")
        .arg("--project")
        .arg(overlay_project())
        .arg("--overlay-scenario")
        .arg(&scn)
        .arg("--out")
        .arg(&html)
        .assert()
        .success();

    let page = std::fs::read_to_string(&html).expect("html written");
    let graph = embedded_graph_json(&page);
    let overlay = &graph["overlay"];
    assert_eq!(overlay["kind"], "value", "scenario yields a value overlay");
    // The Output node carries a numeric series ending at 50.0.
    let series = overlay["nodes"]["Root.Demo.Output"]["series"]
        .as_array()
        .expect("Output node has a series");
    let last = series.last().expect("series is non-empty");
    assert_eq!(last["num"], 50.0, "Output's last cell is 50.0; got {last}");
}

#[test]
fn cli_overlay_log_embeds_values() {
    // `--overlay-log <log.csv>` (no override) produces a VALUE overlay (the
    // identity replay): `"kind":"value"`, the logged Speed channel present, and
    // no changed cone.
    let dir = tempfile::tempdir().expect("temp dir");
    let log = dir.path().join("run.csv");
    std::fs::write(&log, OVERLAY_LOG_CSV).expect("write log");
    let html = dir.path().join("graph.html");

    Command::cargo_bin("m1-visualiser")
        .expect("binary builds")
        .arg("--project")
        .arg(overlay_project())
        .arg("--overlay-log")
        .arg(&log)
        .arg("--out")
        .arg(&html)
        .assert()
        .success();

    let page = std::fs::read_to_string(&html).expect("html written");
    let graph = embedded_graph_json(&page);
    let overlay = &graph["overlay"];
    assert_eq!(
        overlay["kind"], "value",
        "a log without override is value mode"
    );
    assert!(
        overlay["nodes"].get("Root.Demo.Speed").is_some(),
        "the logged Speed channel rides into the overlay; got {overlay}"
    );
    assert!(
        overlay["changed"]
            .as_array()
            .expect("changed array")
            .is_empty(),
        "identity replay flags nothing changed; got {}",
        overlay["changed"]
    );
}

#[test]
fn cli_overlay_diff_marks_changed() {
    // `--overlay-log <log.csv> --override "Root.Demo.Speed=40.0"` recomputes the
    // override's downstream cone: the overlay is `"kind":"diff"` and its `changed`
    // set names the moved Output channel.
    let dir = tempfile::tempdir().expect("temp dir");
    let log = dir.path().join("run.csv");
    std::fs::write(&log, OVERLAY_LOG_CSV).expect("write log");
    let html = dir.path().join("graph.html");

    Command::cargo_bin("m1-visualiser")
        .expect("binary builds")
        .arg("--project")
        .arg(overlay_project())
        .arg("--overlay-log")
        .arg(&log)
        .arg("--override")
        .arg("Root.Demo.Speed=40.0")
        .arg("--out")
        .arg(&html)
        .assert()
        .success();

    let page = std::fs::read_to_string(&html).expect("html written");
    let graph = embedded_graph_json(&page);
    let overlay = &graph["overlay"];
    assert_eq!(overlay["kind"], "diff", "an override switches to diff mode");
    let changed: Vec<&str> = overlay["changed"]
        .as_array()
        .expect("changed array")
        .iter()
        .map(|v| v.as_str().expect("changed id is a string"))
        .collect();
    assert!(
        changed.contains(&"Root.Demo.Output"),
        "the overridden cone must include Output; got {changed:?}"
    );
}

#[test]
fn cli_at_time_selects_tick() {
    // `--at-time 0.0` records the chosen default scrubber tick (nearest to 0.0,
    // i.e. the first tick = index 0) in the overlay JSON as `start_tick`, so the
    // viewer opens there. The full series is still embedded.
    let dir = tempfile::tempdir().expect("temp dir");
    let log = dir.path().join("run.csv");
    std::fs::write(&log, OVERLAY_LOG_CSV).expect("write log");
    let html = dir.path().join("graph.html");

    Command::cargo_bin("m1-visualiser")
        .expect("binary builds")
        .arg("--project")
        .arg(overlay_project())
        .arg("--overlay-log")
        .arg(&log)
        .arg("--at-time")
        .arg("0.0")
        .arg("--out")
        .arg(&html)
        .assert()
        .success();

    let page = std::fs::read_to_string(&html).expect("html written");
    let graph = embedded_graph_json(&page);
    let overlay = &graph["overlay"];
    assert_eq!(
        overlay["start_tick"], 0,
        "--at-time 0.0 selects the first tick; got {}",
        overlay["start_tick"]
    );
}

#[test]
fn cli_overlay_requires_log_for_override() {
    // `--override` without `--overlay-log` is a usage error: fail loud (non-zero),
    // never silently produce a value overlay.
    let dir = tempfile::tempdir().expect("temp dir");
    let html = dir.path().join("graph.html");

    Command::cargo_bin("m1-visualiser")
        .expect("binary builds")
        .arg("--project")
        .arg(overlay_project())
        .arg("--override")
        .arg("Root.Demo.Speed=40.0")
        .arg("--out")
        .arg(&html)
        .assert()
        .failure();
}

#[test]
fn cli_overlay_scenario_and_log_are_mutually_exclusive() {
    // The two overlay sources are xor: requesting both must fail loud, never
    // silently pick one.
    let dir = tempfile::tempdir().expect("temp dir");
    let scn = dir.path().join("scenario.toml");
    std::fs::write(&scn, OVERLAY_SCENARIO_TOML).expect("write scenario");
    let log = dir.path().join("run.csv");
    std::fs::write(&log, OVERLAY_LOG_CSV).expect("write log");
    let html = dir.path().join("graph.html");

    Command::cargo_bin("m1-visualiser")
        .expect("binary builds")
        .arg("--project")
        .arg(overlay_project())
        .arg("--overlay-scenario")
        .arg(&scn)
        .arg("--overlay-log")
        .arg(&log)
        .arg("--out")
        .arg(&html)
        .assert()
        .failure();
}

#[test]
fn cli_ld_log_without_feature_fails_loud() {
    // A `.ld` overlay log built without `--features ld` must exit non-zero (the
    // engine fails loud, naming the feature). This test runs against whatever the
    // default build is; under `--features ld` the `.ld` path is enabled and the
    // engine would instead fail on the file's contents, so the assertion is just
    // "non-zero exit" either way (a bogus `.ld` never succeeds).
    let dir = tempfile::tempdir().expect("temp dir");
    let log = dir.path().join("run.ld");
    std::fs::write(&log, "not a real ld file").expect("write bogus ld");
    let html = dir.path().join("graph.html");

    Command::cargo_bin("m1-visualiser")
        .expect("binary builds")
        .arg("--project")
        .arg(overlay_project())
        .arg("--overlay-log")
        .arg(&log)
        .arg("--out")
        .arg(&html)
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
