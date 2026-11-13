//! Workspace discovery for every command, and the one way to get a workspace
//! checked against the config's package selection.

use std::path::{Path, PathBuf};

use oakum::discover::{discover_cargo, discover_pnpm};
use oakum::plan::{Package, Workspace};

use super::config::LoadedConfig;
use super::repository::Repository;
use super::CliError;

const NOTHING_TO_DISCOVER: &str = "no Cargo.toml or package.json found; nothing to discover";

/// The workspace with the config's `resolves-dependencies-at` applied,
/// refused when the config's `include` or `exclude` names a package it does
/// not have: a planner that skipped the check would plan over a selection the
/// config did not mean.
///
/// # Errors
///
/// Discovery, nothing to discover, or a selection the workspace cannot meet.
pub(super) fn discover_validated(
    repo: &Repository,
    config: &LoadedConfig,
) -> Result<Workspace, CliError> {
    let workspace = discover_workspace(repo)?;
    config.validate_workspace_selection(&workspace)?;
    super::release_state::apply_package_overrides(&workspace, config)
        .map_err(|err| CliError::new(err.to_string()))
}

/// Empty discovery is a typed miss (`Ok(None)`), not a string-matched error.
/// Hard failures (adapter errors, invalid workspace) remain `Err`. Unvalidated:
/// `migrate` reads it before any config exists.
pub(super) fn try_discover_workspace(
    repo: &Repository,
) -> Result<Option<Workspace>, Box<dyn std::error::Error>> {
    let path = repo.ambient_path()?;
    let mut packages = Vec::new();
    let mut errors = Vec::new();
    // Unverified only when no tool gave a real answer: a finding outranks a
    // look that did not happen.
    let mut unanswered = true;
    let cwd = std::env::current_dir()?;

    let mut cargo_workspace_root = None;
    let mut catalog_file = None;
    let mut pnpm_lock_dir = None;
    let mut pnpm_projects = Vec::new();

    let cargo_dir = find_manifest_dir(&cwd, path, "Cargo.toml");
    if cargo_dir.is_some() || path.join("Cargo.toml").is_file() {
        let path = repo.ambient_path()?;
        let start = cargo_dir.as_deref().unwrap_or(path);
        match discover_cargo(start, path) {
            Ok(ws) => {
                cargo_workspace_root = ws.cargo_workspace_root().map(str::to_owned);
                packages.extend(ws.packages().cloned());
            }
            Err(err) => {
                unanswered &= err.is_unverified();
                errors.push(format!("cargo: {err}"));
            }
        }
    }

    let pnpm_marker = path.join("pnpm-workspace.yaml").is_file()
        || path.join("package.json").is_file()
        || find_manifest_dir(&cwd, path, "package.json").is_some();
    if pnpm_marker {
        let path = repo.ambient_path()?;
        let start = find_manifest_dir(&cwd, path, "package.json")
            .or_else(|| find_manifest_dir(&cwd, path, "pnpm-workspace.yaml"))
            .unwrap_or_else(|| path.to_path_buf());
        match discover_pnpm(&start, path) {
            Ok(ws) => {
                catalog_file = ws.catalog_file().map(str::to_owned);
                pnpm_lock_dir = ws.pnpm_lock_dir().map(str::to_owned);
                pnpm_projects = ws.pnpm_projects().to_vec();
                packages.extend(ws.packages().cloned());
            }
            Err(err) => {
                unanswered &= err.is_unverified();
                errors.push(format!("pnpm: {err}"));
            }
        }
    }

    if !errors.is_empty() {
        let detail = format!("workspace discovery failed ({})", errors.join("; "));
        return Err(Box::new(if unanswered {
            CliError::unverified(format!("unverified: {detail}"))
        } else {
            CliError::new(detail)
        }));
    }

    let _ = repo.ambient_path()?;
    if packages.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        workspace_from_discovered(packages, cargo_workspace_root, catalog_file, pnpm_lock_dir)?
            .with_pnpm_projects(pnpm_projects),
    ))
}

fn discover_workspace(repo: &Repository) -> Result<Workspace, Box<dyn std::error::Error>> {
    match try_discover_workspace(repo)? {
        Some(workspace) => Ok(workspace),
        None => Err(Box::new(CliError::new(NOTHING_TO_DISCOVER))),
    }
}

fn workspace_from_discovered(
    packages: Vec<Package>,
    cargo_workspace_root: Option<String>,
    catalog_file: Option<String>,
    pnpm_lock_dir: Option<String>,
) -> Result<Workspace, Box<dyn std::error::Error>> {
    let mut workspace = Workspace::new(packages).map_err(|err| -> Box<dyn std::error::Error> {
        Box::new(CliError::new(err.to_string()))
    })?;
    if let Some(dir) = cargo_workspace_root {
        workspace = workspace.with_cargo_workspace_root(dir);
    }
    if let Some(path) = catalog_file {
        workspace = workspace.with_catalog_file(path);
    }
    if let Some(dir) = pnpm_lock_dir {
        workspace = workspace.with_pnpm_lock_dir(dir);
    }
    Ok(workspace)
}

fn find_manifest_dir(start: &Path, stop: &Path, file_name: &str) -> Option<PathBuf> {
    let mut dir = std::fs::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
    let stop = std::fs::canonicalize(stop).unwrap_or_else(|_| stop.to_path_buf());
    loop {
        if dir.join(file_name).is_file() {
            return Some(dir);
        }
        if same_walk_stop(&dir, &stop) || !dir.pop() {
            return None;
        }
    }
}

/// `Path` equality is structural: `C:\repo` and `\\?\C:\repo` are not equal,
/// so a walk that stops only on `==` continues into an ancestor cargo
/// workspace. Canonicalize when the path exists; also treat Windows
/// drive/UNC spellings as one identity.
fn same_walk_stop(dir: &Path, stop: &Path) -> bool {
    if dir == stop {
        return true;
    }
    #[cfg(any(windows, test))]
    {
        let a = super::fs::normalized_windows_path(dir);
        let b = super::fs::normalized_windows_path(stop);
        if windows_abs(&a) && a.eq_ignore_ascii_case(&b) {
            return true;
        }
    }
    false
}

#[cfg(any(windows, test))]
fn windows_abs(path: &str) -> bool {
    let bytes = path.as_bytes();
    (bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/'))
        || path.starts_with(r"\\")
}

#[cfg(test)]
mod tests {
    use oakum::plan::{Ecosystem, Package, PackageId, ResolvesDependenciesAt};
    use semver::Version;

    use super::workspace_from_discovered;

    fn pkg(ecosystem: Ecosystem, name: &str) -> Package {
        Package::new(
            PackageId::new(ecosystem, name),
            Version::new(0, 1, 0),
            ResolvesDependenciesAt::Install,
            true,
            vec![],
        )
    }

    #[test]
    fn cargo_and_pnpm_merge_keeps_both_discovery_paths() {
        let workspace = workspace_from_discovered(
            vec![pkg(Ecosystem::Cargo, "core"), pkg(Ecosystem::Npm, "app")],
            Some("rust".into()),
            Some("js/pnpm-workspace.yaml".into()),
            Some("js".into()),
        )
        .expect("merge");
        assert_eq!(workspace.cargo_workspace_root(), Some("rust"));
        assert_eq!(workspace.catalog_file(), Some("js/pnpm-workspace.yaml"));
        assert_eq!(workspace.pnpm_lock_dir(), Some("js"));
    }

    #[test]
    fn merge_without_paths_leaves_them_none() {
        let workspace =
            workspace_from_discovered(vec![pkg(Ecosystem::Cargo, "core")], None, None, None)
                .expect("merge");
        assert_eq!(workspace.cargo_workspace_root(), None);
        assert_eq!(workspace.catalog_file(), None);
        assert_eq!(workspace.pnpm_lock_dir(), None);
    }

    #[test]
    fn verbatim_and_plain_windows_paths_are_the_same_walk_stop() {
        use std::path::Path;
        assert!(super::same_walk_stop(
            Path::new(r"C:\a\oakum\oakum\target\tmp\pin-npm"),
            Path::new(r"\\?\C:\a\oakum\oakum\target\tmp\pin-npm"),
        ));
        assert!(super::same_walk_stop(
            Path::new(r"C:\Repo"),
            Path::new(r"\\?\c:\repo"),
        ));
        assert!(!super::same_walk_stop(
            Path::new(r"C:\a\oakum\oakum"),
            Path::new(r"\\?\C:\a\oakum\oakum\target\tmp\pin-npm"),
        ));
    }

    #[test]
    fn find_manifest_dir_does_not_walk_past_a_verbatim_stop() {
        use std::path::Path;
        let start = Path::new(r"C:\a\oakum\oakum\target\tmp\pin-npm");
        let stop = Path::new(r"\\?\C:\a\oakum\oakum\target\tmp\pin-npm");
        assert_eq!(super::find_manifest_dir(start, stop, "Cargo.toml"), None);
    }
}
