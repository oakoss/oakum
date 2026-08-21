use std::io;
use std::path::{Component, Path, PathBuf};

use super::DiscoverError;

pub(super) fn canonicalize(path: &Path) -> Result<PathBuf, DiscoverError> {
    std::fs::canonicalize(path).map_err(|source| DiscoverError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Normalize a path for [`ensure_contained`].
///
/// Prefer `canonicalize` when the path exists so symlink repos compare equal.
/// Fall back to [`std::path::absolute`] plus lexical `..` / `.` cleanup when the
/// path is missing (e.g. `pnpm root -w` prints `…/node_modules` before install).
/// Both arguments to [`ensure_contained`] must go through this helper.
pub(super) fn normalize_for_containment(path: &Path) -> Result<PathBuf, DiscoverError> {
    match std::fs::canonicalize(path) {
        Ok(canonical) => Ok(canonical),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            let absolute = std::path::absolute(path).map_err(|source| DiscoverError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            Ok(lexical_normalize(&absolute))
        }
        Err(source) => Err(DiscoverError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Collapse `.` and `..` without touching the filesystem. Needed because
/// [`std::path::absolute`] preserves `..`, and [`Path::starts_with`] then treats
/// `/repo/../outside` as contained in `/repo`.
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// A workspace rooted in a subdirectory of the repository is legitimate; one
/// rooted in an ancestor is not.
///
/// Both arguments must already be [`normalize_for_containment`] results.
pub(super) fn ensure_contained(
    workspace_root: &Path,
    repository_root: &Path,
) -> Result<(), DiscoverError> {
    if workspace_root.starts_with(repository_root) {
        Ok(())
    } else {
        Err(DiscoverError::WorkspaceRootOutsideRepository {
            workspace_root: workspace_root.to_path_buf(),
            repository_root: repository_root.to_path_buf(),
        })
    }
}

/// Repository-relative directory. Empty string is the repository root.
pub(super) fn repo_relative(dir: &Path, repository_root: &Path) -> Result<String, DiscoverError> {
    let relative =
        dir.strip_prefix(repository_root)
            .map_err(|_| DiscoverError::InvalidMetadata {
                message: format!(
                    "package directory {} is outside the repository root {}",
                    dir.display(),
                    repository_root.display()
                ),
            })?;
    let text = relative
        .to_str()
        .ok_or_else(|| DiscoverError::InvalidMetadata {
            message: format!("package directory {} is not valid UTF-8", dir.display()),
        })?;
    Ok(text.replace('\\', "/"))
}
