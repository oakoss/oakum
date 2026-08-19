//! Release planning: intent plus a discovered workspace to a set of version changes.
//!
//! Everything here is pure. No filesystem, no subprocesses, no network — which is
//! what lets the cascade math be replayed against recorded history to check it
//! against releases that already happened.
//!
//! That purity is currently held by convention. If this module ever needs a
//! dependency that performs I/O, extract it to its own crate instead, so the
//! absence of I/O is enforced by the dependency list rather than by review.
