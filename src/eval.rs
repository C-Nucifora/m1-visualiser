// SPDX-License-Identifier: GPL-3.0-or-later
//! The `m1-eval` airlock — the **only** module that depends on `m1-eval`.
//!
//! Mirroring how [`crate::loader`] is the sole module that touches
//! `m1-typecheck` / `m1-core`, this module is the sole module that touches
//! `m1-eval`. It translates an `m1-eval` [`Trace`] (the value-overlay source)
//! into a toolchain-agnostic [`Overlay`] keyed by graph-node id, so that
//! [`crate::model`] / [`crate::json`] / [`crate::dot`] / [`crate::html`] never
//! see an `m1-eval` type.
//!
//! The join is a direct one: a [`GraphNode::id`] is a symbol's dotted path, and
//! [`Trace::channels`] is keyed by the same canonical paths the `m1-eval` runner
//! emits (both crates pin the same `m1-typecheck`, loading the same `Project`),
//! so `node.id == channel_path` lines up with no fuzzy matching. A trace channel
//! with no matching node is ignored; a node with no trace column renders neutral.

use std::collections::BTreeSet;
use std::path::Path;

use m1_eval::{Counterfactual, Engine, InputKind, Log, Scenario, Trace, Value};

use crate::model::{GraphNode, NodeOverlay, Overlay, OverlayCell, OverlayKind};

/// The engine's fail-loud error, re-exported so the CLI (`main.rs`) can match on
/// a run failure without importing `m1-eval` directly — this module stays the
/// only one that names the toolchain. Mirrors how the loader's errors surface to
/// `main` without `m1-typecheck` appearing there.
pub use m1_eval::EvalError;

/// Which textual scenario format a scenario source string is in, so
/// [`run_value_scenario`] dispatches to the matching `m1-eval` constructor
/// ([`Scenario::from_toml_str`] / [`Scenario::from_json_str`]) without guessing
/// from the bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScenarioFormat {
    /// A TOML scenario document (the primary form).
    Toml,
    /// A JSON scenario document (the same shape as the TOML).
    Json,
}

/// Build a [`OverlayKind::Value`] overlay from an `m1-eval` [`Trace`], keyed by
/// graph-node id.
///
/// Every [`Trace::channels`] entry whose path matches a [`GraphNode::id`] in
/// `nodes` becomes a [`NodeOverlay`] whose `series` is the column mapped through
/// `value_cell`; channels with no matching node are dropped. Channels in
/// [`Trace::external`] that are also nodes are collected into
/// [`Overlay::external`]. The result carries `kind = Value`, an empty `changed`
/// set, and `eps = None` (those are diff-mode concerns).
pub fn value_overlay_from_trace(trace: &Trace, nodes: &[GraphNode]) -> Overlay {
    let node_ids: BTreeSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();

    let overlay_nodes = trace
        .channels
        .iter()
        .filter(|(path, _)| node_ids.contains(path.as_str()))
        .map(|(path, column)| {
            let series = column.iter().map(value_cell).collect();
            (
                path.clone(),
                NodeOverlay {
                    series,
                    delta: None,
                    max_abs_delta: None,
                },
            )
        })
        .collect();

    let external = trace
        .external
        .iter()
        .filter(|path| node_ids.contains(path.as_str()))
        .cloned()
        .collect();

    Overlay {
        kind: OverlayKind::Value,
        time: trace.time.clone(),
        nodes: overlay_nodes,
        external,
        changed: Vec::new(),
        eps: None,
        start_tick: None,
    }
}

/// Build a [`OverlayKind::Value`] overlay from a recorded [`Log`], keyed by
/// graph-node id.
///
/// The engine has no no-override counterfactual, so a value overlay of a logged
/// run is built straight from the [`Log`]: the shared `time` axis is the sorted
/// union of every channel's keyframe times, and each logged channel that is also
/// a graph node becomes a [`NodeOverlay`] whose `series` is the channel
/// zero-order-hold-sampled at each grid time (via [`m1_eval::InputSeries::sample`]).
/// Channels with no matching node are dropped, and a constant-kind channel (no
/// keyframes) contributes no grid times but is still sampled if it is a node.
/// The result carries `kind = Value`, an empty `changed` set, no `eps`, and an
/// empty `external` set (a log records observed channels, with no
/// externally-driven distinction).
pub fn value_overlay_from_log(log: &Log, nodes: &[GraphNode]) -> Overlay {
    let node_ids: BTreeSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();

    // The shared tick axis is the sorted, de-duplicated union of every channel's
    // keyframe times, so the overlay preserves the log's own sample points.
    let mut times: Vec<f64> = log
        .channels
        .iter()
        .filter_map(|series| match &series.kind {
            InputKind::Series(points) => Some(points.iter().map(|(t, _)| *t)),
            InputKind::Const(_) => None,
        })
        .flatten()
        .collect();
    times.sort_by(|a, b| a.total_cmp(b));
    times.dedup();

    let overlay_nodes = log
        .channels
        .iter()
        .filter(|series| node_ids.contains(series.channel.as_str()))
        .map(|series| {
            let cells = times
                .iter()
                .map(|t| value_cell(&series.sample(*t)))
                .collect();
            (
                series.channel.clone(),
                NodeOverlay {
                    series: cells,
                    delta: None,
                    max_abs_delta: None,
                },
            )
        })
        .collect();

    Overlay {
        kind: OverlayKind::Value,
        time: times,
        nodes: overlay_nodes,
        external: BTreeSet::new(),
        changed: Vec::new(),
        eps: None,
        start_tick: None,
    }
}

/// Build a [`OverlayKind::Diff`] overlay from an `m1-eval` [`Counterfactual`],
/// keyed by graph-node id.
///
/// Starts from the [`OverlayKind::Value`] overlay of the counterfactual
/// [`Trace`] (so each node's `series` is the recomputed column and
/// [`Overlay::external`] is populated), then switches `kind` to `Diff` and
/// layers the per-channel diff on top: for every `cf.diff.channels` entry whose
/// path matches a node already in the overlay, the [`NodeOverlay`] gains its
/// per-tick `delta` and `max_abs_delta`. [`Overlay::changed`] is
/// [`Diff::changed_channels`](m1_eval::Diff::changed_channels) filtered to node
/// ids, and [`Overlay::eps`] is the diff's threshold.
///
/// The **no-op ⇒ no change** invariant is preserved: a diff with no changed
/// channels yields an empty `changed` set (the engine's identity counterfactual
/// reports none, so nothing is highlighted).
pub fn diff_overlay(cf: &Counterfactual, nodes: &[GraphNode]) -> Overlay {
    let mut overlay = value_overlay_from_trace(&cf.trace, nodes);
    overlay.kind = OverlayKind::Diff;

    // Attach per-tick delta + summary to the nodes that have a diff column. A
    // `ChannelDiff` exists only for channels shared (numerically) by trace and
    // log, so this is a subset of the value-overlay nodes; the rest stay neutral.
    for (path, channel_diff) in &cf.diff.channels {
        if let Some(node_overlay) = overlay.nodes.get_mut(path) {
            node_overlay.delta = Some(channel_diff.delta.clone());
            node_overlay.max_abs_delta = Some(channel_diff.max_abs_delta);
        }
    }

    // The changed cone, restricted to graph nodes. `changed_channels` is already
    // sorted and empty for a no-op override (the load-bearing invariant).
    overlay.changed = cf
        .diff
        .changed_channels()
        .into_iter()
        .filter(|path| overlay.nodes.contains_key(*path))
        .map(str::to_string)
        .collect();

    overlay.eps = Some(cf.diff.eps);
    overlay
}

/// Map one `m1-eval` [`Value`] to a faithful, ramp-aware [`OverlayCell`].
///
/// Numeric values (`Float`/`Int`/`Uint`) become [`OverlayCell::Num`] via
/// [`Value::as_f64`], so the viewer applies a colour/size ramp. A `Bool` becomes
/// [`OverlayCell::Bool`]; an `Enum` or `Str` becomes [`OverlayCell::Str`] of its
/// display form (the enum's member name, matching the trace's own rendering) so
/// it is shown verbatim but never ramped.
fn value_cell(value: &Value) -> OverlayCell {
    match value {
        Value::Bool(b) => OverlayCell::Bool(*b),
        Value::Enum { member, .. } => OverlayCell::Str(member.clone()),
        Value::Str(s) => OverlayCell::Str(s.clone()),
        // `Float`/`Int`/`Uint` are exactly the variants `as_f64` accepts, so the
        // coercion cannot fail here; fall back to a string cell defensively
        // rather than panicking if `m1-eval` ever broadens the numeric set.
        numeric => match numeric.as_f64() {
            Ok(x) => OverlayCell::Num(x),
            Err(_) => OverlayCell::Str(format!("{numeric:?}")),
        },
    }
}

/// Run a scenario through the real [`Engine`] and build a [`OverlayKind::Value`]
/// overlay from the resulting [`Trace`], keyed by the structural model's
/// `nodes`.
///
/// `project` is the `.m1prj` (the same path the visualiser's loader takes),
/// `cfg` the optional `.m1cfg`. `scenario_src` is the *contents* of a scenario
/// file (`.toml`/`.json`) the CLI read; `format` says which constructor to use,
/// so the bytes are never sniffed. The pipeline is
/// [`Engine::load`] → [`Scenario::from_toml_str`] / [`Scenario::from_json_str`]
/// → [`Engine::run`] → [`value_overlay_from_trace`]. Any load / parse / run
/// failure propagates as an [`EvalError`] — never swallowed.
pub fn run_value_scenario(
    project: &Path,
    cfg: Option<&Path>,
    scenario_src: &str,
    format: ScenarioFormat,
    nodes: &[GraphNode],
) -> Result<Overlay, EvalError> {
    let engine = Engine::load(project, cfg)?;
    let scenario = match format {
        ScenarioFormat::Toml => Scenario::from_toml_str(scenario_src)?,
        ScenarioFormat::Json => Scenario::from_json_str(scenario_src)?,
    };
    let trace = engine.run(&scenario)?;
    Ok(value_overlay_from_trace(&trace, nodes))
}

/// Load a recorded log through the real [`Engine`] and build a
/// [`OverlayKind::Value`] overlay of its logged channels, keyed by the
/// structural model's `nodes`.
///
/// The pipeline is [`Engine::load`] → [`Engine::load_log`] → read back the
/// attached [`Log`] ([`Engine::log`]) → [`value_overlay_from_log`]. The overlay
/// is the log replayed onto its own keyframe grid; with no override there is no
/// counterfactual cone, so `changed` is empty by construction (the load-bearing
/// invariant a value overlay must keep). A missing / unreadable / unsupported
/// log fails loud as an [`EvalError`].
///
/// Note on the source choice: the engine has no `run`-from-log entry point, and
/// its counterfactual run requires at least one override channel that a function
/// reads (an empty-override `run_counterfactual_diff` fails loud), so the
/// faithful value-from-log source is the attached [`Log`] itself, sampled onto
/// its keyframe grid here in the airlock — never a guessed or empty trace.
pub fn run_value_log(
    project: &Path,
    cfg: Option<&Path>,
    log_path: &Path,
    nodes: &[GraphNode],
) -> Result<Overlay, EvalError> {
    let mut engine = Engine::load(project, cfg)?;
    engine.load_log(log_path)?;
    let log = engine
        .log()
        .expect("load_log succeeded, so a log is attached");
    Ok(value_overlay_from_log(log, nodes))
}

/// Run a counterfactual (a logged run plus channel overrides) through the real
/// [`Engine`] and build a [`OverlayKind::Diff`] overlay, keyed by the structural
/// model's `nodes`.
///
/// The pipeline is [`Engine::load`] → [`Engine::load_log`] →
/// [`Engine::override_channel`] for each `overrides` spec (`CH=value-or-expr`) →
/// [`Engine::run_counterfactual_diff`] → [`diff_overlay`]. The override cone the
/// run moved lands in [`Overlay::changed`]; with no overrides the diff is the
/// identity counterfactual and `changed` is empty (the load-bearing invariant).
/// A missing log, a malformed override spec, or a run failure all propagate as
/// an [`EvalError`] — the caller decides how to report it.
pub fn run_diff(
    project: &Path,
    cfg: Option<&Path>,
    log_path: &Path,
    overrides: &[String],
    nodes: &[GraphNode],
) -> Result<Overlay, EvalError> {
    let mut engine = Engine::load(project, cfg)?;
    engine.load_log(log_path)?;
    for spec in overrides {
        engine.override_channel(spec)?;
    }
    let cf = engine.run_counterfactual_diff()?;
    Ok(diff_overlay(&cf, nodes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NodeKind;
    use m1_eval::{ChannelDiff, Counterfactual, Diff};

    /// A `Trace` with one tick and the given channels (no externals).
    fn trace_with(channels: &[(&str, Vec<Value>)], time: Vec<f64>) -> Trace {
        let mut trace = Trace::new();
        trace.time = time;
        for (path, column) in channels {
            trace.channels.insert((*path).to_string(), column.clone());
        }
        trace
    }

    fn node(id: &str) -> GraphNode {
        GraphNode::new(id, NodeKind::Channel)
    }

    /// A `ChannelDiff` from a per-tick `delta`, deriving `max_abs_delta` and the
    /// `changed` flag against `eps` (mirrors `Diff::between_eps`'s arithmetic so a
    /// hand-built diff is self-consistent).
    fn channel_diff(counterfactual: &[f64], logged: &[f64], eps: f64) -> ChannelDiff {
        let delta: Vec<f64> = counterfactual
            .iter()
            .zip(logged)
            .map(|(cf, lg)| cf - lg)
            .collect();
        let max_abs_delta = delta
            .iter()
            .copied()
            .map(f64::abs)
            .filter(|d| d.is_finite())
            .fold(0.0_f64, f64::max);
        let changed = max_abs_delta > eps;
        ChannelDiff {
            logged: logged.to_vec(),
            counterfactual: counterfactual.to_vec(),
            delta,
            max_abs_delta,
            changed,
        }
    }

    /// Hand-build a `Counterfactual`: a trace from `counterfactual` columns and a
    /// `Diff` whose `ChannelDiff`s compare those columns against `logged` ones.
    fn counterfactual_with(
        cols: &[(&str, Vec<f64>, Vec<f64>)],
        time: Vec<f64>,
        eps: f64,
    ) -> Counterfactual {
        let trace = trace_with(
            &cols
                .iter()
                .map(|(path, cf, _)| {
                    (
                        *path,
                        cf.iter().map(|v| Value::Float(*v)).collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>(),
            time.clone(),
        );
        let mut channels = std::collections::BTreeMap::new();
        for (path, cf, logged) in cols {
            channels.insert((*path).to_string(), channel_diff(cf, logged, eps));
        }
        Counterfactual {
            trace,
            diff: Diff {
                time,
                channels,
                eps,
            },
        }
    }

    #[test]
    fn value_overlay_keys_by_node_id() {
        let trace = trace_with(
            &[(
                "Root.Demo.Output",
                vec![Value::Float(50.0), Value::Float(50.0)],
            )],
            vec![0.0, 0.01],
        );
        let nodes = [node("Root.Demo.Output")];

        let overlay = value_overlay_from_trace(&trace, &nodes);

        assert_eq!(overlay.kind, OverlayKind::Value);
        assert_eq!(overlay.time, vec![0.0, 0.01]);
        assert_eq!(
            overlay.nodes["Root.Demo.Output"].series,
            vec![OverlayCell::Num(50.0), OverlayCell::Num(50.0)]
        );
        assert!(overlay.changed.is_empty());
        assert_eq!(overlay.eps, None);
    }

    #[test]
    fn trace_channels_without_a_node_are_dropped() {
        // `Root.Builtin.Hidden` has no matching node, so it produces no overlay
        // entry — no spurious keys leak into the overlay.
        let trace = trace_with(
            &[
                ("Root.Demo.Output", vec![Value::Float(1.0)]),
                ("Root.Builtin.Hidden", vec![Value::Float(2.0)]),
            ],
            vec![0.0],
        );
        let nodes = [node("Root.Demo.Output")];

        let overlay = value_overlay_from_trace(&trace, &nodes);

        assert!(overlay.nodes.contains_key("Root.Demo.Output"));
        assert!(!overlay.nodes.contains_key("Root.Builtin.Hidden"));
        assert_eq!(overlay.nodes.len(), 1);
    }

    #[test]
    fn external_channels_are_flagged() {
        let mut trace = trace_with(
            &[
                ("Root.Demo.Speed", vec![Value::Float(20.0)]),
                ("Root.Demo.Output", vec![Value::Float(50.0)]),
            ],
            vec![0.0],
        );
        // `Speed` is externally driven and a node; `CanIn` is external but NOT a
        // node, so it must not leak into `overlay.external`.
        trace.mark_external("Root.Demo.Speed");
        trace.mark_external("Root.Demo.CanIn");
        let nodes = [node("Root.Demo.Speed"), node("Root.Demo.Output")];

        let overlay = value_overlay_from_trace(&trace, &nodes);

        assert!(overlay.external.contains("Root.Demo.Speed"));
        assert!(!overlay.external.contains("Root.Demo.CanIn"));
        assert_eq!(overlay.external.len(), 1);
    }

    #[test]
    fn non_numeric_values_become_string_cells() {
        let trace = trace_with(
            &[
                ("Root.Demo.Mode", vec![Value::Str("Idle".to_string())]),
                (
                    "Root.Demo.State",
                    vec![Value::Enum {
                        id: 1,
                        member: "Precharging".to_string(),
                    }],
                ),
                ("Root.Demo.Armed", vec![Value::Bool(true)]),
                ("Root.Demo.Output", vec![Value::Float(50.0)]),
            ],
            vec![0.0],
        );
        let nodes = [
            node("Root.Demo.Mode"),
            node("Root.Demo.State"),
            node("Root.Demo.Armed"),
            node("Root.Demo.Output"),
        ];

        let overlay = value_overlay_from_trace(&trace, &nodes);

        assert_eq!(
            overlay.nodes["Root.Demo.Mode"].series,
            vec![OverlayCell::Str("Idle".to_string())]
        );
        assert_eq!(
            overlay.nodes["Root.Demo.State"].series,
            vec![OverlayCell::Str("Precharging".to_string())]
        );
        assert_eq!(
            overlay.nodes["Root.Demo.Armed"].series,
            vec![OverlayCell::Bool(true)]
        );
        // The numeric one still rampable.
        assert_eq!(
            overlay.nodes["Root.Demo.Output"].series,
            vec![OverlayCell::Num(50.0)]
        );
    }

    #[test]
    fn diff_overlay_marks_changed_nodes() {
        // `Mid` moved by +5 under the override; `Sensor` held its logged value.
        let cf = counterfactual_with(
            &[
                ("Root.CF.Sensor", vec![10.0, 10.0], vec![10.0, 10.0]),
                ("Root.CF.Mid", vec![30.0, 30.0], vec![25.0, 25.0]),
            ],
            vec![0.0, 1.0],
            1e-9,
        );
        let nodes = [node("Root.CF.Sensor"), node("Root.CF.Mid")];

        let overlay = diff_overlay(&cf, &nodes);

        assert_eq!(overlay.kind, OverlayKind::Diff);
        assert_eq!(overlay.changed, vec!["Root.CF.Mid".to_string()]);
        assert_eq!(overlay.nodes["Root.CF.Mid"].max_abs_delta, Some(5.0));
        // The unchanged channel still carries its (zero) summary, never `changed`.
        assert_eq!(overlay.nodes["Root.CF.Sensor"].max_abs_delta, Some(0.0));
        assert_eq!(overlay.eps, Some(1e-9));
    }

    #[test]
    fn diff_overlay_carries_per_tick_delta() {
        let cf = counterfactual_with(
            &[("Root.CF.Mid", vec![30.0, 31.0], vec![25.0, 25.0])],
            vec![0.0, 1.0],
            1e-9,
        );
        let nodes = [node("Root.CF.Mid")];

        let overlay = diff_overlay(&cf, &nodes);

        // The per-node `delta` is the `ChannelDiff.delta`, aligned to `time`.
        assert_eq!(overlay.time, cf.diff.time);
        assert_eq!(
            overlay.nodes["Root.CF.Mid"].delta,
            Some(cf.diff.channels["Root.CF.Mid"].delta.clone())
        );
        assert_eq!(overlay.nodes["Root.CF.Mid"].delta, Some(vec![5.0, 6.0]));
    }

    #[test]
    fn noop_diff_has_no_changed_nodes() {
        // The load-bearing invariant: a no-op override (counterfactual == logged)
        // leaves the changed set empty.
        let cf = counterfactual_with(
            &[
                ("Root.CF.Sensor", vec![10.0, 10.0], vec![10.0, 10.0]),
                ("Root.CF.Mid", vec![25.0, 25.0], vec![25.0, 25.0]),
            ],
            vec![0.0, 1.0],
            1e-9,
        );
        let nodes = [node("Root.CF.Sensor"), node("Root.CF.Mid")];

        let overlay = diff_overlay(&cf, &nodes);

        assert!(
            overlay.changed.is_empty(),
            "no-op override must not flag changes: {:?}",
            overlay.changed
        );
        assert_eq!(overlay.kind, OverlayKind::Diff);
    }

    #[test]
    fn diff_overlay_series_is_the_counterfactual_trace() {
        // `series` reads the counterfactual trace column (so the scrubber still
        // shows values), while `delta`/`changed` drive the highlight/ramp.
        let cf = counterfactual_with(
            &[("Root.CF.Mid", vec![30.0, 31.0], vec![25.0, 25.0])],
            vec![0.0, 1.0],
            1e-9,
        );
        let nodes = [node("Root.CF.Mid")];

        let overlay = diff_overlay(&cf, &nodes);

        assert_eq!(
            overlay.nodes["Root.CF.Mid"].series,
            vec![OverlayCell::Num(30.0), OverlayCell::Num(31.0)]
        );
    }

    // ---- O4: the run helpers driven by the real `m1-eval` Engine ----
    //
    // These use a small fixture project under `tests/fixtures/overlay/` (a
    // `Root.Demo` group with `Speed`/`Gain`/`Output` channels and an `Update`
    // `FuncUser` computing `Output = Speed * Gain`, mirroring `m1-eval`'s `mini`
    // fixture) loaded through the *real* engine, so the helpers' wiring (load →
    // run → map) is exercised end-to-end behind the airlock.

    use std::path::PathBuf;

    /// The `tests/fixtures/overlay` directory.
    fn overlay_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/overlay")
    }

    /// The fixture's `.m1prj` and `.m1cfg` paths.
    fn overlay_paths() -> (PathBuf, PathBuf) {
        let dir = overlay_dir();
        (dir.join("Project.m1prj"), dir.join("parameters.m1cfg"))
    }

    /// The structural graph nodes of the overlay fixture, built by the real
    /// loader, so node ids are the canonical paths the trace/diff key by.
    fn overlay_nodes() -> Vec<GraphNode> {
        let (project, cfg) = overlay_paths();
        crate::loader::load(&project, Some(&cfg), None)
            .expect("overlay fixture loads through the structural loader")
            .nodes
    }

    /// Write `contents` to a uniquely-named `ext` file under a fresh temp dir and
    /// return both (the dir must outlive the path, so it is returned too).
    fn temp_log(ext: &str, contents: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(format!("run.{ext}"));
        std::fs::write(&path, contents).expect("write temp log");
        (dir, path)
    }

    #[test]
    fn scenario_run_produces_value_overlay() {
        // A `function` scenario over `Demo.Update` with Speed=20, Gain=2.5 makes
        // `Output = 50.0` every tick. The run helper loads the fixture, runs the
        // scenario, and maps the trace to a VALUE overlay keyed by node id.
        let (project, cfg) = overlay_paths();
        let nodes = overlay_nodes();
        let scenario = r#"
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

        let overlay =
            run_value_scenario(&project, Some(&cfg), scenario, ScenarioFormat::Toml, &nodes)
                .expect("scenario run produces a value overlay");

        assert_eq!(overlay.kind, OverlayKind::Value);
        let output = &overlay.nodes["Root.Demo.Output"];
        assert_eq!(
            output.series.last(),
            Some(&OverlayCell::Num(50.0)),
            "Output's last cell is 20 * 2.5 = 50: {:?}",
            output.series
        );
        // A value overlay carries no changed set and no diff threshold.
        assert!(overlay.changed.is_empty());
        assert_eq!(overlay.eps, None);
    }

    #[test]
    fn log_replay_produces_value_overlay() {
        // Replaying a `time`-first CSV log through the identity counterfactual (no
        // overrides) yields a VALUE overlay: the logged channels are present and
        // nothing is flagged changed (the no-op invariant the helper relies on).
        let (project, cfg) = overlay_paths();
        let nodes = overlay_nodes();
        // Mutually consistent with Gain=2.5: Output = Speed * 2.5.
        let csv = "time,Root.Demo.Speed,Root.Demo.Output\n\
                   0.00,20,50\n\
                   0.01,20,50\n";
        let (_dir, log_path) = temp_log("csv", csv);

        let overlay = run_value_log(&project, Some(&cfg), &log_path, &nodes)
            .expect("log replay produces a value overlay");

        assert_eq!(overlay.kind, OverlayKind::Value);
        // The logged Speed channel's series rode through to the overlay.
        let speed = &overlay.nodes["Root.Demo.Speed"];
        assert_eq!(speed.series.first(), Some(&OverlayCell::Num(20.0)));
        // No override ⇒ nothing changed (a value overlay never carries a cone).
        assert!(
            overlay.changed.is_empty(),
            "identity replay must flag no changes: {:?}",
            overlay.changed
        );
    }

    #[test]
    fn override_produces_diff_overlay() {
        // Overriding the logged `Speed` recomputes its downstream cone: `Output`
        // (= Speed * Gain) moves, so the DIFF overlay flags it as changed.
        let (project, cfg) = overlay_paths();
        let nodes = overlay_nodes();
        let csv = "time,Root.Demo.Speed,Root.Demo.Output\n\
                   0.00,20,50\n\
                   0.01,20,50\n";
        let (_dir, log_path) = temp_log("csv", csv);
        let overrides = vec!["Root.Demo.Speed=40.0".to_string()];

        let overlay = run_diff(&project, Some(&cfg), &log_path, &overrides, &nodes)
            .expect("override produces a diff overlay");

        assert_eq!(overlay.kind, OverlayKind::Diff);
        assert!(
            overlay.changed.contains(&"Root.Demo.Output".to_string()),
            "the overridden cone must include Output: {:?}",
            overlay.changed
        );
        // The moved node carries a positive max-abs-delta and a per-tick delta.
        let output = &overlay.nodes["Root.Demo.Output"];
        assert!(
            output.max_abs_delta.is_some_and(|d| d > 0.0),
            "Output moved, so max_abs_delta > 0: {:?}",
            output.max_abs_delta
        );
        assert!(output.delta.is_some());
        // A diff overlay records the diff threshold it was computed against.
        assert!(overlay.eps.is_some(), "diff overlay carries its eps");
    }

    #[test]
    fn missing_log_for_diff_fails_loud() {
        // A diff request with no log surfaces the engine's fail-loud `EvalError`
        // (we propagate it, never swallow it into an empty overlay).
        let (project, cfg) = overlay_paths();
        let nodes = overlay_nodes();
        let missing = overlay_dir().join("does-not-exist.csv");
        let overrides = vec!["Root.Demo.Speed=40.0".to_string()];

        let err = run_diff(&project, Some(&cfg), &missing, &overrides, &nodes)
            .expect_err("a missing log must fail loud, not produce an empty overlay");
        // A `.csv` path that does not exist is a `MissingInput` from `load_log`.
        assert!(
            matches!(err, EvalError::MissingInput { .. }),
            "expected a fail-loud MissingInput, got {err:?}"
        );
    }
}
