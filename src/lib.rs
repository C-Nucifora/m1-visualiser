// SPDX-License-Identifier: GPL-3.0-or-later
//! `m1-visualiser` — an interactive structural graph/visualiser for MoTeC M1
//! projects.
//!
//! Architecture mirrors `m1-doc`: a [`loader`] turns a loaded `m1-typecheck`
//! project into a toolchain-agnostic [`model::GraphModel`], which the renderers
//! ([`dot`], [`json`], [`html`]) consume. Only [`loader`] touches `m1-typecheck`
//! / `m1-core` types.
//!
//! v1 is structural-first and covers all four edge types — data-flow,
//! table-axis, hierarchy and schedule (see [`model::EdgeKind`]) — plus the
//! interactive viewer (search, per-edge-kind filters, collapse/expand,
//! dependency-cone highlight). The only deferred workflow is a numeric value
//! overlay (via `m1-eval`); see the design doc.

pub mod dataflow;
pub mod dot;
pub mod html;
pub mod json;
pub mod loader;
pub mod model;
