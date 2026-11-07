//! In-memory manifest and lockfile edits. Filesystem writes belong to
//! `version` (ADR-0003).

mod cargo_declared;
mod cargo_dep;
mod cargo_lock;
mod catalog;
mod catalog_file;
mod inherited;
mod json;
mod key_path;
mod npm_spec;
mod pnpm_lock;
mod rewrite;
mod toml;
mod workspace_root;
mod yaml_scalar;

#[cfg(test)]
mod roundtrip;

pub use cargo_declared::{declared_members, CargoConfig, Declarations, NotToml};
pub(crate) use cargo_dep::{
    dependency_entry, item_workspace_true, member_version, workspace_entry,
    workspace_entry_declares_version, EntryMiss, MemberVersion, WorkspaceMiss,
};
pub use cargo_lock::{retarget_cargo_lock, CargoLockBump, CargoLockError};
pub use catalog::{rewrite_catalog_yaml, AnchorNote, CatalogYamlError};
pub(crate) use catalog_file::CatalogFile;
pub use inherited::{
    collect_inherited_pins, needs_cargo_members, rewrite_collected_pins, InheritedError,
    InheritedPins, InheritedRewrites, InheritedSources,
};
pub(crate) use json::package_json_version;
pub use json::{set_json_string, JsonEditError};
pub use key_path::{
    parse_key_path, parse_write_key_path, replace_json_at_key, KeyPathError, KeySegment,
};
pub(crate) use npm_spec::split_name_version;
pub use pnpm_lock::{
    pnpm_lock_drift, retarget_pnpm_lock, PnpmCatalogChange, PnpmCatalogRef, PnpmImporterChange,
    PnpmLockDrift, PnpmLockDrifted, PnpmLockLeft, PnpmLockLeftReason, PnpmLockRetarget,
    PnpmManifest,
};
pub use rewrite::{
    rewrite_dependencies, rewrite_dependency, rewrite_workspace_dependency, RewriteError,
};
pub(crate) use toml::{cargo_declared_version, cargo_workspace_version, DeclaredVersion};
pub use toml::{cargo_package_version_inherits_workspace, set_toml_string, TomlEditError};
pub use workspace_root::{
    root_changes, RootChanges, RootFile, RootFileError, RootFiles, RootReason,
};
