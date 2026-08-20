//! Release planning: intent plus a discovered workspace to a set of version changes.
//!
//! Everything here is pure. No filesystem, no subprocesses, no network — which is
//! what lets the cascade math be replayed against recorded history to check it
//! against releases that already happened.
//!
//! Two mechanisms hold that, neither covering the other's channel. `clippy.toml`
//! denies the I/O entry points at their call sites, and a module permitted to
//! reach them opts out with `#[expect(clippy::disallowed_methods, reason = "...")]`;
//! when a second module under `src/` carries that marker, extract this one into
//! its own crate (ADR-0002). `crates/plan-no-std` compiles these sources under
//! `#![no_std]`, which is what stops a `std::` path or a std-prelude item from
//! reaching them at all (ADR-0024).

pub mod bump;
pub mod workspace;

pub use bump::{apply_bump, effective_bump, AppliedBump, BumpLevel, Versioning};
