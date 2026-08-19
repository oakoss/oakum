//! Release planning: intent plus a discovered workspace to a set of version changes.
//!
//! Everything here is pure. No filesystem, no subprocesses, no network — which is
//! what lets the cascade math be replayed against recorded history to check it
//! against releases that already happened.
//!
//! Purity is enforced at the call sites: `clippy.toml` denies the I/O entry
//! points, and a module permitted to reach them opts out with
//! `#[expect(clippy::disallowed_methods, reason = "...")]`. When a second module
//! under `src/` carries that marker, extract this one into its own crate
//! (ADR-0002).
