//! In-memory manifest and lockfile edits. Filesystem writes belong to
//! `version` (ADR-0003).

mod cargo_lock;
mod json;
mod rewrite;
mod toml;

pub use cargo_lock::{retarget_cargo_lock, CargoLockBump, CargoLockError};
pub use json::{set_json_string, JsonEditError};
pub use rewrite::{rewrite_dependencies, rewrite_dependency, RewriteError};
pub use toml::set_preserving_decor;
