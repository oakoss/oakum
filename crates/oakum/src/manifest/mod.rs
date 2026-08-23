//! In-memory manifest edits. Filesystem writes belong to `version` (ADR-0003).

mod json;
mod rewrite;
mod toml;

pub use json::{set_json_string, JsonEditError};
pub use rewrite::{rewrite_dependencies, rewrite_dependency, RewriteError};
pub use toml::set_preserving_decor;
