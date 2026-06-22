// SPDX-License-Identifier: GPL-3.0-or-later
//! `m1-visualiser` CLI: load a MoTeC M1 project, build its structural graph, and
//! write it out as Graphviz DOT, JSON, or a self-contained interactive HTML
//! file.

use clap::{Parser, ValueEnum};
use m1_visualiser::eval::{self, ScenarioFormat};
use m1_visualiser::model::Overlay;
use m1_visualiser::{dot, html, json, loader};
use std::path::{Path, PathBuf};
use std::process;

#[derive(Parser, Debug)]
#[command(
    name = "m1-visualiser",
    version,
    about = "Interactive structural graph/visualiser for MoTeC M1 projects"
)]
struct Args {
    /// Project.m1prj (defaults to nearest upward, or $M1_PROJECT).
    #[arg(long)]
    project: Option<PathBuf>,
    /// Parameters `.m1cfg` (table/parameter shape; drives table dimensions).
    /// When omitted, a single sibling `*.m1cfg` next to the project is used;
    /// ambiguity (zero or several) means none.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Output file. Defaults to `m1-graph.<ext>` for the chosen format.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Html)]
    format: Format,
    /// Graph title (defaults to the project file's directory name).
    #[arg(long)]
    title: Option<String>,
    /// Overlay computed VALUES from a scenario file (`.toml`/`.json`), run
    /// through `m1-eval`. Mutually exclusive with `--overlay-log`. Off by
    /// default — without any overlay flag the output is the structural v1 graph.
    #[arg(long, value_name = "FILE")]
    overlay_scenario: Option<PathBuf>,
    /// Overlay computed VALUES from a recorded log (`.csv`, or `.ld` with
    /// `--features ld`). With one or more `--override`, switches to a DIFF
    /// overlay instead. Mutually exclusive with `--overlay-scenario`.
    #[arg(long, value_name = "FILE")]
    overlay_log: Option<PathBuf>,
    /// Register a counterfactual override `CH=value-or-expr` (repeatable).
    /// Requires `--overlay-log`; switches it from a value overlay to a DIFF
    /// overlay of the override's downstream cone.
    #[arg(long = "override", value_name = "SPEC")]
    overrides: Vec<String>,
    /// The scrubber's initial time in seconds — the viewer opens on the nearest
    /// tick. Defaults to the last tick (the design's "last-tick value").
    #[arg(long, value_name = "SECONDS")]
    at_time: Option<f64>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Format {
    Dot,
    Json,
    Html,
}

impl Format {
    /// Default output file extension for this format.
    fn ext(self) -> &'static str {
        match self {
            Format::Dot => "dot",
            Format::Json => "json",
            Format::Html => "html",
        }
    }
}

/// Resolve the project path: explicit `--project`, then `$M1_PROJECT`, then the
/// nearest `Project.m1prj` upward from the cwd.
fn resolve_project(arg: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(p) = arg {
        return Some(p);
    }
    if let Ok(p) = std::env::var("M1_PROJECT") {
        return Some(PathBuf::from(p));
    }
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join("Project.m1prj");
        if candidate.is_file() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Resolve the config path: explicit `--config` wins. Otherwise discover a
/// single sibling `*.m1cfg` next to the project; if zero or several exist the
/// result is `None` (conservative — ambiguity never guesses).
fn resolve_config(arg: Option<PathBuf>, project_path: &std::path::Path) -> Option<PathBuf> {
    if let Some(p) = arg {
        return Some(p);
    }
    let dir = project_path.parent()?;
    let mut found: Option<PathBuf> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("m1cfg") {
            if found.is_some() {
                // More than one sibling cfg: ambiguous, so use none.
                return None;
            }
            found = Some(path);
        }
    }
    found
}

/// The overlay-flag inputs, grouped so the guard + dispatch logic lives in one
/// well-named place rather than inline in `main`.
struct OverlayRequest<'a> {
    /// `--overlay-scenario` (VALUE overlay from a scenario file).
    scenario: Option<&'a Path>,
    /// `--overlay-log` (VALUE overlay from a log; DIFF when `overrides` present).
    log: Option<&'a Path>,
    /// `--override` specs (repeatable; require `log`).
    overrides: &'a [String],
    /// `--at-time` seconds (the scrubber's initial tick; default = last tick).
    at_time: Option<f64>,
}

impl OverlayRequest<'_> {
    /// Build the overlay this request asks for, or `Ok(None)` when no overlay
    /// flag is set (the v1 path).
    ///
    /// Guards, all fail-loud:
    /// - `--overlay-scenario` and `--overlay-log` are mutually exclusive.
    /// - `--override` requires `--overlay-log` (never silently a value overlay).
    ///
    /// On success the chosen `m1-eval` run produces an [`Overlay`], onto which the
    /// scrubber's start tick is recorded: the nearest tick to `--at-time`, or the
    /// last tick by default. Usage errors and engine failures both surface as a
    /// `String` the caller prints before exiting 1.
    fn build(
        &self,
        project: &Path,
        cfg: Option<&Path>,
        model: &m1_visualiser::model::GraphModel,
    ) -> Result<Option<Overlay>, String> {
        // Mutually-exclusive sources.
        if self.scenario.is_some() && self.log.is_some() {
            return Err(
                "--overlay-scenario and --overlay-log are mutually exclusive (pick one)".into(),
            );
        }
        // An override only makes sense against a log (the counterfactual ground
        // truth); without one it is a usage error, never a silent value overlay.
        if !self.overrides.is_empty() && self.log.is_none() {
            return Err("--override requires --overlay-log".into());
        }

        let nodes = &model.nodes;
        let overlay = if let Some(scenario_path) = self.scenario {
            let src = std::fs::read_to_string(scenario_path)
                .map_err(|e| format!("{}: {e}", scenario_path.display()))?;
            let format = scenario_format(scenario_path)?;
            eval::run_value_scenario(project, cfg, &src, format, nodes)
                .map_err(|e| e.to_string())?
        } else if let Some(log_path) = self.log {
            if self.overrides.is_empty() {
                eval::run_value_log(project, cfg, log_path, nodes).map_err(|e| e.to_string())?
            } else {
                eval::run_diff(project, cfg, log_path, self.overrides, nodes)
                    .map_err(|e| e.to_string())?
            }
        } else {
            // No overlay flag: the structural v1 path.
            return Ok(None);
        };

        Ok(Some(self.apply_start_tick(overlay)))
    }

    /// Record the scrubber's initial tick on `overlay`: the nearest tick to
    /// `--at-time`, or the last tick by default. An empty time axis leaves the
    /// hint unset (the viewer falls back to its own default).
    fn apply_start_tick(&self, overlay: Overlay) -> Overlay {
        let tick = match self.at_time {
            Some(t) => overlay.nearest_tick(t),
            None => overlay.time.len().checked_sub(1),
        };
        match tick {
            Some(i) => overlay.with_start_tick(i),
            None => overlay,
        }
    }
}

/// Pick the scenario constructor from a scenario file's extension: `.toml` or
/// `.json`. Any other (or missing) extension is a fail-loud usage error rather
/// than a guess at the bytes.
fn scenario_format(path: &Path) -> Result<ScenarioFormat, String> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("toml") => Ok(ScenarioFormat::Toml),
        Some("json") => Ok(ScenarioFormat::Json),
        _ => Err(format!(
            "--overlay-scenario must be a .toml or .json file: {}",
            path.display()
        )),
    }
}

fn main() {
    let args = Args::parse();

    let Some(project_path) = resolve_project(args.project) else {
        eprintln!("m1-visualiser: no Project.m1prj found (pass --project or set $M1_PROJECT)");
        process::exit(2);
    };

    let config_path = resolve_config(args.config, &project_path);

    let title = args.title.or_else(|| {
        project_path
            .parent()
            .and_then(|d| d.file_name())
            .map(|n| n.to_string_lossy().into_owned())
    });

    let model = match loader::load(&project_path, config_path.as_deref(), title) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("m1-visualiser: {}: {e}", project_path.display());
            process::exit(1);
        }
    };

    // Optionally compute and fold an overlay onto the structural model. With no
    // overlay flag this is a no-op and the render below is byte-for-byte the v1
    // tool. A usage error (bad flag combination) or an engine `EvalError` both
    // exit 1, mirroring a load/write failure's shape.
    let overlay_request = OverlayRequest {
        scenario: args.overlay_scenario.as_deref(),
        log: args.overlay_log.as_deref(),
        overrides: &args.overrides,
        at_time: args.at_time,
    };
    let model = match overlay_request.build(&project_path, config_path.as_deref(), &model) {
        Ok(Some(overlay)) => model.with_overlay(overlay),
        Ok(None) => model,
        Err(e) => {
            eprintln!("m1-visualiser: {}: {e}", project_path.display());
            process::exit(1);
        }
    };

    let rendered = match args.format {
        Format::Dot => dot::render(&model),
        Format::Json => json::render(&model),
        Format::Html => html::render(&model),
    };

    let out = args
        .out
        .unwrap_or_else(|| PathBuf::from(format!("m1-graph.{}", args.format.ext())));

    if let Err(e) = std::fs::write(&out, rendered) {
        eprintln!("m1-visualiser: {}: {e}", out.display());
        process::exit(1);
    }

    eprintln!(
        "m1-visualiser: wrote {} ({} nodes, {} edges)",
        out.display(),
        model.nodes.len(),
        model.edges.len()
    );
}
