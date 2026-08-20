//! Compiles `plan`'s real sources under `no_std`. See this package's manifest
//! for why, and ADR-0024 for what it does and does not prove.
//!
//! One `#[path]` covers the whole module tree because `plan` is a directory
//! module: submodules of a `mod.rs` resolve beside it, so a new file under
//! `crates/oakum/src/plan/` is picked up here with no edit and cannot drift out
//! of coverage.
#![no_std]

extern crate alloc;

#[path = "../../oakum/src/plan/mod.rs"]
pub mod plan;
