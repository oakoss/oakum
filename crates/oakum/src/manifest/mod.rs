//! In-memory manifest and lockfile edits. Filesystem writes belong to
//! `version` (ADR-0003).

mod cargo_lock;
mod catalog;
mod inherited;
mod json;
mod rewrite;
mod toml;

#[cfg(test)]
mod roundtrip;

pub use cargo_lock::{retarget_cargo_lock, CargoLockBump, CargoLockError};
pub use catalog::{
    rewrite_catalog_json, rewrite_catalog_yaml, yaml_has_catalog_table, CatalogYamlError,
};
pub use inherited::{
    inheriting_cargo_dependents, rewrite_inherited_pins, InheritedError, InheritedRewrites,
    InheritedSources,
};
pub use json::{json_has_catalog_table, set_json_string, JsonEditError};
pub use rewrite::{
    rewrite_dependencies, rewrite_dependency, rewrite_workspace_dependency, RewriteError,
};
pub use toml::{set_toml_string, TomlEditError};
