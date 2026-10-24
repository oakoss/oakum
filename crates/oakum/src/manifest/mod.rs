//! In-memory manifest and lockfile edits. Filesystem writes belong to
//! `version` (ADR-0003).

mod cargo_lock;
mod catalog;
mod inherited;
mod json;
mod key_path;
mod pnpm_lock;
mod rewrite;
mod toml;
mod yaml_scalar;

#[cfg(test)]
mod roundtrip;

pub use cargo_lock::{retarget_cargo_lock, CargoLockBump, CargoLockError};
pub use catalog::{
    rewrite_catalog_json, rewrite_catalog_yaml, yaml_has_catalog_table, AnchorNote,
    CatalogYamlError,
};
pub use inherited::{
    collect_inherited_pins, inheriting_cargo_dependents, rewrite_collected_pins,
    rewrite_inherited_pins, CatalogRewrite, CatalogText, InheritedError, InheritedPins,
    InheritedRewrites, InheritedSources,
};
pub use json::{json_has_catalog_table, set_json_string, JsonEditError};
pub use key_path::{
    parse_key_path, parse_write_key_path, replace_json_at_key, KeyPathError, KeySegment,
};
pub use pnpm_lock::{
    retarget_pnpm_lock, PnpmCatalogChange, PnpmCatalogRef, PnpmImporterChange, PnpmLockLeft,
    PnpmLockLeftReason, PnpmLockRetarget,
};
pub use rewrite::{
    rewrite_dependencies, rewrite_dependency, rewrite_workspace_dependency, RewriteError,
};
pub use toml::{cargo_package_version_inherits_workspace, set_toml_string, TomlEditError};
