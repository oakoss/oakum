//! Release planning across package ecosystems.
//!
//! The planner is a pure function from intent (change files or conventional
//! commits) plus a discovered workspace to a release plan. Everything that
//! touches the filesystem, a registry, or GitHub lives outside it, so the
//! cascade math can be tested against recorded history without side effects.
//!
//! Modules are named for the crates they would become if this is ever split;
//! see `AGENTS.md` for what would trigger that.

pub mod discover;
pub mod plan;
