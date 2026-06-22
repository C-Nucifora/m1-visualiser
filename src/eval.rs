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

use m1_eval::{Counterfactual, Trace, Value};

use crate::model::{GraphNode, NodeOverlay, Overlay, OverlayCell, OverlayKind};

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
}
