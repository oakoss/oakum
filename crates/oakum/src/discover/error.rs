//! Why discovery could not produce a [`crate::plan::Workspace`].

use core::fmt;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use super::child::{ChildFailure, DEADLINE_VAR};
use crate::plan::WorkspaceError;

#[derive(Debug)]
pub enum DiscoverError {
    /// For exit 101, `message` is cargo's stderr (stray-manifest wording)
    /// relayed verbatim.
    CargoMetadata {
        status: Option<i32>,
        message: String,
    },
    CargoNotRunnable {
        source: io::Error,
    },
    PnpmList {
        status: Option<i32>,
        message: String,
    },
    PnpmRoot {
        status: Option<i32>,
        message: String,
    },
    PnpmNotRunnable {
        source: io::Error,
    },
    /// A discovery child ran past the deadline and oakum killed it: whatever
    /// it would have answered, the look did not happen.
    ChildDeadline {
        tool: &'static str,
        limit: Duration,
    },
    /// The child exited, but something it spawned held its output open until
    /// the deadline, so its answer could not be collected.
    ChildStalled {
        tool: &'static str,
        limit: Duration,
    },
    /// Waiting on the child, or reading its output, failed partway.
    ChildIo {
        tool: &'static str,
        message: String,
    },
    /// `OAKUM_REMOTE_DEADLINE` is not a positive whole number of seconds.
    BadDeadline(String),
    /// Metadata or package.json parsed but is not usable for planning.
    InvalidMetadata {
        message: String,
    },
    PnpmVersion {
        message: String,
    },
    WorkspaceRootOutsideRepository {
        workspace_root: PathBuf,
        repository_root: PathBuf,
    },
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Json(serde_json::Error),
    Toml {
        path: PathBuf,
        message: String,
    },
    Version {
        package: String,
        message: String,
    },
    Range {
        package: String,
        dependency: String,
        message: String,
    },
    /// `catalog:` / `catalog:<name>` could not be resolved from the workspace yaml.
    UnresolvedCatalog {
        package: String,
        dependency: String,
        catalog_name: Option<String>,
        /// Loaded or searched `pnpm-workspace.yaml` path; `None` only when no
        /// catalog search ran (e.g. lone package).
        path: Option<PathBuf>,
    },
    UnknownDependencyKind {
        package: String,
        dependency: String,
        kind: String,
    },
    Workspace(WorkspaceError),
}

impl fmt::Display for DiscoverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CargoMetadata { status, message } => match status {
                Some(code) => write!(f, "cargo metadata exited {code}: {message}"),
                None => write!(f, "cargo metadata failed: {message}"),
            },
            Self::CargoNotRunnable { source } => write!(f, "could not run cargo: {source}"),
            Self::PnpmList { status, message } => match status {
                Some(code) => write!(f, "pnpm list exited {code}: {message}"),
                None => write!(f, "pnpm list failed: {message}"),
            },
            Self::PnpmRoot { status, message } => match status {
                Some(code) => write!(f, "pnpm root -w exited {code}: {message}"),
                None => write!(f, "pnpm root -w failed: {message}"),
            },
            Self::PnpmNotRunnable { source } => write!(f, "could not run pnpm: {source}"),
            Self::ChildDeadline { tool, limit } => write!(
                f,
                "`{tool}` gave no answer within {}s, so oakum killed it (anything it started may still be running); a file it reads may be a FIFO or a lock it waits on. Set {DEADLINE_VAR} (seconds) if it is legitimately slow",
                limit.as_secs()
            ),
            Self::ChildStalled { tool, limit } => write!(
                f,
                "`{tool}` exited but something it spawned still held its output open {}s later and may still be running, so its answer could not be collected. Set {DEADLINE_VAR} (seconds) to wait longer",
                limit.as_secs()
            ),
            Self::ChildIo { tool, message } => write!(f, "`{tool}` ran but {message}"),
            Self::BadDeadline(message) => f.write_str(message),
            Self::InvalidMetadata { message } => write!(f, "discovery metadata: {message}"),
            Self::PnpmVersion { message } => write!(f, "`pnpm --version` {message}"),
            Self::WorkspaceRootOutsideRepository {
                workspace_root,
                repository_root,
            } => write!(
                f,
                "workspace root {} is outside repository {}",
                workspace_root.display(),
                repository_root.display()
            ),
            Self::Io { path, source } => write!(f, "read {}: {source}", path.display()),
            Self::Json(err) => write!(f, "discovery JSON: {err}"),
            Self::Toml { path, message } => {
                write!(f, "parse {}: {message}", path.display())
            }
            Self::Version { package, message } => {
                write!(f, "package {package}: {message}")
            }
            Self::Range {
                package,
                dependency,
                message,
            } => write!(f, "{package} dependency on {dependency}: {message}"),
            Self::UnresolvedCatalog {
                package,
                dependency,
                catalog_name,
                path,
            } => {
                let protocol = match catalog_name {
                    None => String::from("catalog:"),
                    Some(catalog) => format!("catalog:{catalog}"),
                };
                match path {
                    Some(path) => write!(
                        f,
                        "{package} dependency on {dependency}: unresolved catalog protocol {protocol} in {}",
                        path.display()
                    ),
                    None => write!(
                        f,
                        "{package} dependency on {dependency}: unresolved catalog protocol {protocol} (no pnpm-workspace.yaml found)"
                    ),
                }
            }
            Self::UnknownDependencyKind {
                package,
                dependency,
                kind,
            } => write!(
                f,
                "{package} dependency on {dependency}: unknown kind {kind:?}"
            ),
            Self::Workspace(err) => write!(f, "{err}"),
        }
    }
}

impl DiscoverError {
    /// A child whose answer never arrived, or that never ran because the
    /// deadline itself was malformed: the look did not happen, which is not
    /// the same as a look that found a problem. Git's children class the same
    /// failures the same way.
    #[must_use]
    pub fn is_unverified(&self) -> bool {
        matches!(
            self,
            Self::ChildDeadline { .. }
                | Self::ChildStalled { .. }
                | Self::ChildIo { .. }
                | Self::BadDeadline(_)
        )
    }
}

impl DiscoverError {
    /// A discovery child named `tool` that produced no output, worded for
    /// discovery; `not_runnable` words a spawn failure for its tool.
    pub(super) fn from_child(
        tool: &'static str,
        failure: ChildFailure,
        not_runnable: fn(io::Error) -> Self,
    ) -> Self {
        match failure {
            ChildFailure::Spawn(err) => not_runnable(err),
            ChildFailure::Deadline { limit } => Self::ChildDeadline { tool, limit },
            ChildFailure::DrainStalled { limit, .. } => Self::ChildStalled { tool, limit },
            ChildFailure::Wait(err) => Self::ChildIo {
                tool,
                message: format!("waiting on it failed ({err}); oakum killed it"),
            },
            ChildFailure::Read(err) => Self::ChildIo {
                tool,
                message: format!("its output could not be read: {err}"),
            },
        }
    }
}

impl std::error::Error for DiscoverError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CargoNotRunnable { source }
            | Self::PnpmNotRunnable { source }
            | Self::Io { source, .. } => Some(source),
            Self::Json(err) => Some(err),
            Self::Workspace(err) => Some(err),
            _ => None,
        }
    }
}

impl From<WorkspaceError> for DiscoverError {
    fn from(value: WorkspaceError) -> Self {
        Self::Workspace(value)
    }
}

impl From<serde_json::Error> for DiscoverError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::time::Duration;

    use super::{ChildFailure, DiscoverError};

    fn exit_status() -> std::process::ExitStatus {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(0)
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(0)
        }
    }

    /// A spawn failure is a finding about the machine; every other failure is
    /// an answer that never arrived.
    #[test]
    fn each_child_failure_keeps_its_class() {
        let limit = Duration::from_secs(2);
        let not_runnable = |source| DiscoverError::CargoNotRunnable { source };
        let classify = |failure| DiscoverError::from_child("cargo metadata", failure, not_runnable);

        let spawned = classify(ChildFailure::Spawn(io::Error::other("missing")));
        assert!(matches!(spawned, DiscoverError::CargoNotRunnable { .. }));
        assert!(!spawned.is_unverified());

        let expired = classify(ChildFailure::Deadline { limit });
        assert!(matches!(expired, DiscoverError::ChildDeadline { .. }));
        assert!(expired.is_unverified());

        let stalled = classify(ChildFailure::DrainStalled {
            limit,
            status: exit_status(),
        });
        assert!(matches!(stalled, DiscoverError::ChildStalled { .. }));
        assert!(stalled.is_unverified());

        for failure in [
            ChildFailure::Wait(io::Error::other("wait")),
            ChildFailure::Read(io::Error::other("read")),
        ] {
            let failed = classify(failure);
            assert!(matches!(failed, DiscoverError::ChildIo { .. }), "{failed}");
            assert!(failed.is_unverified(), "{failed}");
        }

        assert!(DiscoverError::BadDeadline(String::from("bad")).is_unverified());
    }
}
