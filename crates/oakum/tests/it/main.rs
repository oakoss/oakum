//! The integration suite as one binary. Add a suite as a module here: a new
//! `tests/*.rs` file becomes its own binary and relinks on every library change.

// Tests spawn the binary and read repository files; not an ADR-0002 marker.
#![allow(clippy::disallowed_methods)]

mod support;

mod add_cli;
mod cargo_lockfile;
mod changeset_foreign_parsers;
mod check;
mod cli;
mod config_cli;
mod detect_cli;
mod discover_cascade;
mod fixture_probe;
mod generate_cli;
mod git_boundary;
mod init_cli;
mod io_boundary;
mod layout;
mod migrate_cli;
mod no_std_probe;
mod one_binary;
mod plan_fixtures;
mod plan_intent_cli;
mod pr_status_cli;
mod prettier_oracle;
mod reachable_tags;
mod release_cli;
mod require_signed_commits;
mod status_cli;
mod tag_drift;
mod upgrade_cli;
mod version_cli;
mod version_pr_cli;
mod write_ownership;
