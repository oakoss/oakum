//! Why discovery could not produce a [`crate::plan::Workspace`].

use core::fmt;
use std::io;
use std::path::PathBuf;

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
    /// Metadata or package.json parsed but is not usable for planning.
    InvalidMetadata {
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
    /// `catalog:` / `catalog:<name>` until okm-1t8 resolves catalog bounds.
    UnresolvedCatalog {
        package: String,
        dependency: String,
        catalog_name: Option<String>,
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
            Self::InvalidMetadata { message } => write!(f, "discovery metadata: {message}"),
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
            } => match catalog_name {
                None => write!(
                    f,
                    "{package} dependency on {dependency}: unresolved catalog protocol catalog: (okm-1t8)"
                ),
                Some(catalog) => write!(
                    f,
                    "{package} dependency on {dependency}: unresolved catalog protocol catalog:{catalog} (okm-1t8)"
                ),
            },
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
