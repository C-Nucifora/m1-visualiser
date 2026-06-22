// SPDX-License-Identifier: GPL-3.0-or-later
//! `m1-visualiser` CLI: load a MoTeC M1 project, build its structural graph, and
//! write it out as Graphviz DOT, JSON, or a self-contained interactive HTML
//! file.

use clap::{Parser, ValueEnum};
use m1_visualiser::{dot, html, json, loader};
use std::path::PathBuf;
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
    /// Output file. Defaults to `m1-graph.<ext>` for the chosen format.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Html)]
    format: Format,
    /// Graph title (defaults to the project file's directory name).
    #[arg(long)]
    title: Option<String>,
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

fn main() {
    let args = Args::parse();

    let Some(project_path) = resolve_project(args.project) else {
        eprintln!("m1-visualiser: no Project.m1prj found (pass --project or set $M1_PROJECT)");
        process::exit(2);
    };

    let title = args.title.or_else(|| {
        project_path
            .parent()
            .and_then(|d| d.file_name())
            .map(|n| n.to_string_lossy().into_owned())
    });

    let model = match loader::load(&project_path, title) {
        Ok(m) => m,
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
