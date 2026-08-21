//! Workspace discovery.
//!
//! Ask the package manager rather than parsing manifests: `cargo metadata`
//! resolves `version.workspace = true` and lists implicit members that a
//! `members` array never mentions. `pnpm list` returns the flat member set for
//! npm-shaped workspaces.
//!
//! Discovery must not mutate the repository. `cargo metadata` without
//! `--no-deps` writes a `Cargo.lock` into a crate that had none, and `pnpm exec`
//! performs an install. Both are disqualified from this path.
//!
//! A stray `pnpm-workspace.yaml` above the repository silently reparents the
//! workspace root: `pnpm list -r` reports that ancestor's packages and omits the
//! one you asked about, exit 0, nothing on stderr. Assert the resolved root is
//! inside the repository before planning. Subdirectory roots are fine; ancestor
//! roots are not.

#[expect(
    clippy::disallowed_methods,
    reason = "discovery runs cargo metadata and peeks Cargo.toml for path-linked edges"
)]
mod cargo;
mod error;
#[expect(
    clippy::disallowed_methods,
    reason = "discovery path helpers canonicalize on the I/O boundary"
)]
mod paths;
#[expect(
    clippy::disallowed_methods,
    reason = "discovery runs pnpm list / root -w and reads package.json for edges"
)]
mod pnpm;

pub use cargo::{discover_cargo, workspace_from_cargo_metadata};
pub use error::DiscoverError;
pub use pnpm::{discover_pnpm, workspace_from_pnpm_list};
