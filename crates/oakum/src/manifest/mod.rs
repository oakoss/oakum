//! In-memory manifest edits. Filesystem writes belong to `version` (ADR-0003).

mod json;
mod toml;

pub use json::{set_json_string, JsonEditError};
pub use toml::set_preserving_decor;
