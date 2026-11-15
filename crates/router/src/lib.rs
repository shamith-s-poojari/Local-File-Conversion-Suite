//! Adapter manifests, the generated registry, and the route planner
//! (spec sections 8.2 and 11.1, blueprint section 3).
//!
//! - [`manifest`] loads `adapters/*/manifest.yaml`, checks each against
//!   `adapters/manifest.schema.json` and the cross-file rules, and builds
//!   the [`Registry`] that `registry-gen` writes to `registry.generated.json`.
//! - [`matrix`] is the product's compatibility matrix.
//! - [`planner`] finds and ranks conversion paths, and lists the targets
//!   for a selection of files.

pub mod manifest;
pub mod matrix;
mod model;
pub mod planner;

pub use manifest::{
    build_registry, AdapterEntry, Catalog, Manifest, OutputFormat, Problem, Registry,
};
pub use model::{Format, Level, Platform, Tier};
pub use planner::{targets, Availability, Plan, Planner, Targets};
