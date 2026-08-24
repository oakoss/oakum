//! Release planning across package ecosystems.
//!
//! The planner is a pure function from intent (change files or conventional
//! commits) plus a discovered workspace to a release plan. Everything that
//! touches the filesystem, a registry, or GitHub lives outside it, so the
//! cascade math can be tested against recorded history without side effects.
//!
//! Modules are named for the crates they would become if this is ever split;
//! see `docs/contributing/structure.md` for what would trigger that.

// `plan` is written against `alloc` rather than `std` so that extraction under
// ADR-0024 is a file move: `alloc` supplies what a `no_std` prelude does not.
// Declared at the crate root, which is what puts `alloc::` in scope for `plan`.
extern crate alloc;

pub mod changeset;
pub mod commits;
pub mod config;
pub mod detect;
pub mod discover;
pub mod manifest;
pub mod plan;
pub mod state;
pub mod tags;
pub mod template;
