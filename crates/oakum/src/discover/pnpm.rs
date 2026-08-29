//! pnpm adapter: `pnpm root -w` + `pnpm list -r --depth -1 --json`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use semver::Version;
use serde::Deserialize;

use super::catalog_file::{catalog_target, CatalogFile, CatalogTarget};
use super::paths::{ensure_contained, normalize_for_containment, repo_relative};
use super::DiscoverError;
use crate::plan::{
    Bounds, BuildResolution, DeclaredRange, Dependency, DependencyKind, Ecosystem, Package,
    PackageId, ResolvesDependenciesAt, Tracking, Workspace,
};

/// Discover the pnpm workspace (or lone package) visible from `package_dir`.
///
/// Probes with `pnpm root -w`: not-in-a-workspace means a lone package; a path
/// inside `repository_root` is a workspace (subdirectory roots ok); a path
/// outside aborts (stray-ancestor). Then `pnpm list -r --depth -1 --json`.
/// Never `pnpm exec` (that installs).
///
/// Edges come from each member's `package.json`. An `optionalDependencies`
/// entry overrides a same-named `dependencies` entry (npm precedence), so
/// only the effective edge is mapped. `catalog:` / `catalog:<name>`
/// resolve against `pnpm-workspace.yaml` (ADR-0010). A `bin` field maps to
/// [`ResolvesDependenciesAt::Build`] with [`BuildResolution::BinaryTarget`].
///
/// # Errors
///
/// Returns [`DiscoverError`] when pnpm fails, the workspace root escapes the
/// repository, JSON is unusable, a `catalog:` entry cannot be resolved, or
/// [`Workspace::new`] refuses the graph.
pub fn discover_pnpm(
    package_dir: impl AsRef<Path>,
    repository_root: impl AsRef<Path>,
) -> Result<Workspace, DiscoverError> {
    let package_dir = package_dir.as_ref();
    let repository_root = normalize_for_containment(repository_root.as_ref())?;
    let workspace_dir = probe_pnpm_root(package_dir, &repository_root)?;
    let catalogs = match &workspace_dir {
        Some(dir) => load_catalogs_at(dir)?,
        None => CatalogTable::empty(),
    };
    let json = run_pnpm_list(package_dir)?;
    workspace_from_pnpm_list_with_catalogs(&json, &repository_root, &catalogs)
}

/// Build a [`Workspace`] from `pnpm list -r --depth -1 --json` output.
///
/// Reads each entry's `package.json` via `path`. Skips entries without a
/// `version` (typical of private workspace roots). Every member path must be
/// contained in `repository_root`, same rule as the `pnpm root -w` probe.
/// Both roots go through [`normalize_for_containment`]. Catalogs are loaded by
/// walking up from the first member to `pnpm-workspace.yaml` (prefer
/// [`discover_pnpm`], which loads from the probed workspace root).
///
/// # Errors
///
/// Same class as [`discover_pnpm`], except pnpm is not invoked.
pub fn workspace_from_pnpm_list(
    json: &str,
    repository_root: impl AsRef<Path>,
) -> Result<Workspace, DiscoverError> {
    let repository_root = normalize_for_containment(repository_root.as_ref())?;
    let entries: Vec<ListEntry> = serde_json::from_str(json)?;
    let packages = packages_from_list_entries(&entries, &repository_root)?;
    let catalogs = catalogs_for_members(&packages, &repository_root)?;
    workspace_from_packages(packages, &catalogs, &repository_root)
}

fn workspace_from_pnpm_list_with_catalogs(
    json: &str,
    repository_root: &Path,
    catalogs: &CatalogTable,
) -> Result<Workspace, DiscoverError> {
    let entries: Vec<ListEntry> = serde_json::from_str(json)?;
    let packages = packages_from_list_entries(&entries, repository_root)?;
    workspace_from_packages(packages, catalogs, repository_root)
}

fn packages_from_list_entries(
    entries: &[ListEntry],
    repository_root: &Path,
) -> Result<Vec<(String, Version, String, PackageManifest)>, DiscoverError> {
    let mut by_name = BTreeMap::new();
    let mut packages = Vec::new();

    for entry in entries {
        let Some(path) = entry.path.as_deref() else {
            if entry.version.is_some() {
                let label = entry.name.as_deref().unwrap_or("<unnamed>");
                return Err(DiscoverError::InvalidMetadata {
                    message: format!("pnpm list entry {label} missing path"),
                });
            }
            continue;
        };
        let member_path = normalize_for_containment(Path::new(path))?;
        ensure_contained(&member_path, repository_root)?;

        let Some(version_text) = entry.version.as_deref() else {
            continue;
        };
        let name = entry
            .name
            .clone()
            .ok_or_else(|| DiscoverError::InvalidMetadata {
                message: String::from("pnpm list entry missing name"),
            })?;
        let version = Version::parse(version_text).map_err(|err| DiscoverError::Version {
            package: name.clone(),
            message: err.to_string(),
        })?;
        let manifest = read_package_json(&member_path)?;
        if by_name.insert(name.clone(), path.to_owned()).is_some() {
            return Err(DiscoverError::InvalidMetadata {
                message: format!("duplicate package name {name} in pnpm list"),
            });
        }
        packages.push((name, version, path.to_owned(), manifest));
    }

    if packages.is_empty() {
        return Err(DiscoverError::InvalidMetadata {
            message: String::from("pnpm list reported no versioned packages"),
        });
    }
    Ok(packages)
}

fn workspace_from_packages(
    packages: Vec<(String, Version, String, PackageManifest)>,
    catalogs: &CatalogTable,
    repository_root: &Path,
) -> Result<Workspace, DiscoverError> {
    let member_names: BTreeSet<String> = packages
        .iter()
        .map(|(name, _, _, _)| name.clone())
        .collect();
    let mut mapped = Vec::with_capacity(packages.len());
    for (name, version, path, manifest) in packages {
        let abs = normalize_for_containment(Path::new(&path))?;
        let manifest_dir = repo_relative(&abs, repository_root)?;
        mapped.push(map_package(
            name,
            version,
            &manifest,
            &member_names,
            catalogs,
            manifest_dir,
        )?);
    }
    let mut workspace = Workspace::new(mapped)?;
    if let Some(path) = catalogs.path.as_deref() {
        workspace = workspace.with_catalog_file(repo_relative(
            &normalize_for_containment(path)?,
            repository_root,
        )?);
    }
    Ok(workspace)
}

/// `Some(workspace_dir)` for a contained workspace; `None` for a lone package.
fn probe_pnpm_root(
    package_dir: &Path,
    repository_root: &Path,
) -> Result<Option<PathBuf>, DiscoverError> {
    let output = Command::new("pnpm")
        .args(["root", "-w"])
        .current_dir(package_dir)
        .output()
        .map_err(|source| DiscoverError::PnpmNotRunnable { source })?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();

    if output.status.success() {
        // `pnpm root -w` prints the workspace `node_modules` directory, which
        // may not exist yet. Strip that leaf and normalize the workspace dir.
        // Join relative output to `package_dir`; `absolute` would use process cwd.
        let printed = Path::new(&stdout);
        let printed: PathBuf = if printed.is_absolute() {
            printed.to_path_buf()
        } else {
            package_dir.join(printed)
        };
        let workspace_dir = if printed
            .file_name()
            .is_some_and(|name| name == "node_modules")
        {
            printed.parent().unwrap_or(printed.as_path())
        } else {
            printed.as_path()
        };
        let root = normalize_for_containment(workspace_dir)?;
        ensure_contained(&root, repository_root)?;
        return Ok(Some(root));
    }

    if is_not_in_workspace(&stderr) {
        return Ok(None);
    }

    let message = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        String::from("no stderr or stdout")
    };
    Err(DiscoverError::PnpmRoot {
        status: output.status.code(),
        message,
    })
}

fn is_not_in_workspace(stderr: &str) -> bool {
    stderr.contains("may only be used inside a workspace")
}

fn run_pnpm_list(package_dir: &Path) -> Result<String, DiscoverError> {
    let output = Command::new("pnpm")
        .args(["list", "-r", "--depth", "-1", "--json"])
        .current_dir(package_dir)
        .output()
        .map_err(|source| DiscoverError::PnpmNotRunnable { source })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let message = if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            String::from("no stderr or stdout")
        };
        return Err(DiscoverError::PnpmList {
            status: output.status.code(),
            message,
        });
    }

    String::from_utf8(output.stdout).map_err(|err| DiscoverError::InvalidMetadata {
        message: format!("stdout was not utf-8: {err}"),
    })
}

fn read_package_json(package_dir: &Path) -> Result<PackageManifest, DiscoverError> {
    let path = package_dir.join("package.json");
    let text = fs::read_to_string(&path).map_err(|source| DiscoverError::Io {
        path: path.clone(),
        source,
    })?;
    serde_json::from_str(&text).map_err(|err| DiscoverError::InvalidMetadata {
        message: format!("{}: {err}", path.display()),
    })
}

fn package_has_bin(manifest: &PackageManifest) -> bool {
    match &manifest.bin {
        None => false,
        Some(PackageBin::Command(s)) => !s.is_empty(),
        Some(PackageBin::Map(map)) => !map.is_empty(),
    }
}

fn map_package(
    name: String,
    version: Version,
    manifest: &PackageManifest,
    members: &BTreeSet<String>,
    catalogs: &CatalogTable,
    manifest_dir: String,
) -> Result<Package, DiscoverError> {
    let resolves = if package_has_bin(manifest) {
        ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget)
    } else {
        ResolvesDependenciesAt::Install
    };

    // npm gives `optionalDependencies` precedence: a same-named entry there
    // overrides the `dependencies` declaration at install time, so mapping
    // both would let the cascade act on a range or alias target that is
    // never effective.
    let effective_dependencies: BTreeMap<String, String> = manifest
        .dependencies
        .iter()
        .filter(|(key, _)| !manifest.optional_dependencies.contains_key(key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();

    let mut dependencies = Vec::new();
    for (kind, deps) in [
        (DependencyKind::Normal, &effective_dependencies),
        (DependencyKind::Peer, &manifest.peer_dependencies),
        (DependencyKind::Optional, &manifest.optional_dependencies),
        (DependencyKind::Development, &manifest.dev_dependencies),
    ] {
        for (declared_as, range_text) in deps {
            let trimmed = range_text.trim();
            // Catalog lookup keys on the manifest key and may rewrite the target
            // via npm: aliases — resolve before the membership filter.
            if trimmed.starts_with("catalog:") {
                let (on_name, range) =
                    resolve_catalog_dependency(&name, declared_as, trimmed, catalogs)?;
                if !members.contains(&on_name) {
                    continue;
                }
                dependencies.push(Dependency {
                    on: PackageId::new(Ecosystem::Npm, on_name),
                    kind,
                    declared_as: declared_as.to_owned(),
                    target: None,
                    range,
                });
                continue;
            }

            let Some(on_name) = resolve_dependency_name(declared_as, range_text) else {
                // Malformed npm:/workspace: must not vanish as "not a member".
                if trimmed.starts_with("npm:") || trimmed.starts_with("workspace:") {
                    return Err(
                        match parse_npm_declared_range(&name, declared_as, range_text) {
                            Err(err) => err,
                            Ok(_) => DiscoverError::Range {
                                package: name,
                                dependency: declared_as.to_owned(),
                                message: format!("unresolved dependency protocol {trimmed}"),
                            },
                        },
                    );
                }
                continue;
            };
            if !members.contains(on_name) {
                continue;
            }
            dependencies.push(map_dependency(
                &name,
                declared_as,
                on_name,
                range_text,
                kind,
            )?);
        }
    }

    Ok(Package::new(
        PackageId::new(Ecosystem::Npm, name),
        version,
        resolves,
        npm_is_publishable(manifest.private),
        dependencies,
    )
    .with_manifest_dir(manifest_dir))
}

fn npm_is_publishable(private: bool) -> bool {
    !private
}

/// Target package name for membership / `on`, distinct from the manifest key when
/// the dependent uses an npm or workspace alias (`"alias": "npm:real@^1"`).
fn resolve_dependency_name<'a>(declared_as: &'a str, range_text: &'a str) -> Option<&'a str> {
    let text = range_text.trim();
    if let Some(rest) = text.strip_prefix("npm:") {
        return split_name_version(rest).map(|(name, _)| name);
    }
    if let Some(rest) = text.strip_prefix("workspace:") {
        let rest = rest.trim();
        if rest.is_empty()
            || rest == "*"
            || rest == "^"
            || rest == "~"
            || rest.starts_with('.')
            || rest.starts_with('/')
        {
            return Some(declared_as);
        }
        match split_name_version(rest) {
            Some((name, version)) if !name.is_empty() && !version.is_empty() => Some(name),
            _ => Some(declared_as),
        }
    } else {
        Some(declared_as)
    }
}

/// Split `name@version` / `@scope/name@version`. Scoped or bare name with no
/// `@version` yields an empty version string so callers can refuse it.
fn split_name_version(spec: &str) -> Option<(&str, &str)> {
    if let Some(without_at) = spec.strip_prefix('@') {
        if let Some(at) = without_at.find('@') {
            let name_end = at + 1; // include leading `@`
            Some((&spec[..name_end], &without_at[at + 1..]))
        } else if without_at.is_empty() {
            None
        } else {
            Some((spec, ""))
        }
    } else if let Some(at) = spec.find('@') {
        Some((&spec[..at], &spec[at + 1..]))
    } else if spec.is_empty() {
        None
    } else {
        Some((spec, ""))
    }
}

fn map_dependency(
    package: &str,
    declared_as: &str,
    on_name: &str,
    range_text: &str,
    kind: DependencyKind,
) -> Result<Dependency, DiscoverError> {
    let range = parse_npm_declared_range(package, declared_as, range_text)?;
    Ok(Dependency {
        on: PackageId::new(Ecosystem::Npm, on_name.to_owned()),
        kind,
        declared_as: declared_as.to_owned(),
        target: None,
        range,
    })
}

fn parse_npm_declared_range(
    package: &str,
    dependency: &str,
    text: &str,
) -> Result<DeclaredRange, DiscoverError> {
    let text = text.trim();
    if text.starts_with("catalog:") {
        return Err(DiscoverError::Range {
            package: package.to_owned(),
            dependency: dependency.to_owned(),
            message: format!("catalog protocol must be resolved via catalog lookup: {text}"),
        });
    }

    if let Some(rest) = text.strip_prefix("npm:") {
        let Some((_name, version)) = split_name_version(rest) else {
            return Err(DiscoverError::Range {
                package: package.to_owned(),
                dependency: dependency.to_owned(),
                message: format!("unsupported npm protocol {text}"),
            });
        };
        if version.is_empty() {
            return Err(DiscoverError::Range {
                package: package.to_owned(),
                dependency: dependency.to_owned(),
                message: format!("npm protocol missing version in {text}"),
            });
        }
        let bounds = Bounds::from_npm_text(version).map_err(|err| DiscoverError::Range {
            package: package.to_owned(),
            dependency: dependency.to_owned(),
            message: err.to_string(),
        })?;
        return Ok(DeclaredRange::Plain(bounds));
    }

    if let Some(rest) = text.strip_prefix("workspace:") {
        let rest = rest.trim();
        // `name@version` (incl. numeric names like `1@*`) vs bare range (`*`, `^1.2.3`).
        let range_body = match split_name_version(rest) {
            Some((name, version))
                if !name.is_empty()
                    && !version.is_empty()
                    && !name.starts_with('.')
                    && !name.starts_with('/') =>
            {
                version
            }
            _ => rest,
        };
        return parse_workspace_protocol(package, dependency, range_body);
    }

    let bounds = Bounds::from_npm_text(text).map_err(|err| DiscoverError::Range {
        package: package.to_owned(),
        dependency: dependency.to_owned(),
        message: err.to_string(),
    })?;
    Ok(DeclaredRange::Plain(bounds))
}

fn resolve_catalog_dependency(
    package: &str,
    dependency: &str,
    text: &str,
    catalogs: &CatalogTable,
) -> Result<(String, DeclaredRange), DiscoverError> {
    let rest = text.strip_prefix("catalog:").unwrap_or(text).trim();
    let catalog_name = if rest.is_empty() || rest == "default" {
        None
    } else {
        Some(rest.to_owned())
    };
    let entry = catalogs
        .lookup(catalog_name.as_deref(), dependency)
        .ok_or_else(|| DiscoverError::UnresolvedCatalog {
            package: package.to_owned(),
            dependency: dependency.to_owned(),
            catalog_name: catalog_name.clone(),
            path: catalogs.path.clone(),
        })?;
    let (on_name, bounds) = parse_catalog_entry_value(package, dependency, entry, catalogs)?;
    Ok((
        on_name,
        DeclaredRange::Catalog {
            name: catalog_name,
            bounds,
        },
    ))
}

fn parse_catalog_entry_value(
    package: &str,
    dependency: &str,
    entry: &str,
    catalogs: &CatalogTable,
) -> Result<(String, Bounds), DiscoverError> {
    let entry = entry.trim();
    let yaml = catalogs.path.as_ref().map_or_else(
        || String::from("pnpm-workspace.yaml"),
        |p| p.display().to_string(),
    );
    if let Some(rest) = entry.strip_prefix("npm:") {
        let Some((name, version)) = split_name_version(rest) else {
            return Err(DiscoverError::Range {
                package: package.to_owned(),
                dependency: dependency.to_owned(),
                message: format!(
                    "catalog entry for {dependency} in {yaml}: unsupported npm protocol {entry}"
                ),
            });
        };
        if name.is_empty() || version.is_empty() {
            return Err(DiscoverError::Range {
                package: package.to_owned(),
                dependency: dependency.to_owned(),
                message: format!("catalog entry for {dependency} in {yaml}: npm protocol missing name or version in {entry}"),
            });
        }
        let bounds = Bounds::from_npm_text(version).map_err(|err| DiscoverError::Range {
            package: package.to_owned(),
            dependency: dependency.to_owned(),
            message: format!("catalog entry for {dependency} in {yaml}: {err}"),
        })?;
        return Ok((name.to_owned(), bounds));
    }
    if entry.starts_with("catalog:") || entry.starts_with("workspace:") {
        return Err(DiscoverError::Range {
            package: package.to_owned(),
            dependency: dependency.to_owned(),
            message: format!(
                "catalog entry for {dependency} in {yaml}: unsupported nested protocol {entry}"
            ),
        });
    }
    let bounds = Bounds::from_npm_text(entry).map_err(|err| DiscoverError::Range {
        package: package.to_owned(),
        dependency: dependency.to_owned(),
        message: format!("catalog entry for {dependency} in {yaml}: {err}"),
    })?;
    Ok((dependency.to_owned(), bounds))
}

fn parse_workspace_protocol(
    package: &str,
    dependency: &str,
    rest: &str,
) -> Result<DeclaredRange, DiscoverError> {
    let rest = rest.trim();
    if rest.is_empty() || rest == "*" {
        return Ok(DeclaredRange::WorkspaceTracking(Tracking::Exact));
    }
    if rest == "~" {
        return Ok(DeclaredRange::WorkspaceTracking(Tracking::Tilde));
    }
    if rest == "^" {
        return Ok(DeclaredRange::WorkspaceTracking(Tracking::Caret));
    }
    if rest.starts_with('.') || rest.starts_with('/') {
        return Err(DiscoverError::Range {
            package: package.to_owned(),
            dependency: dependency.to_owned(),
            message: format!("unsupported relative workspace protocol workspace:{rest}"),
        });
    }

    let bounds = Bounds::from_npm_text(rest).map_err(|err| DiscoverError::Range {
        package: package.to_owned(),
        dependency: dependency.to_owned(),
        message: format!("workspace:{rest}: {err}"),
    })?;
    Ok(DeclaredRange::Workspace(bounds))
}

#[derive(Debug, Default)]
struct CatalogTable {
    path: Option<PathBuf>,
    default: BTreeMap<String, String>,
    named: BTreeMap<String, BTreeMap<String, String>>,
}

impl CatalogTable {
    fn empty() -> Self {
        Self::default()
    }

    /// Empty table that still names the yaml path that was searched (ADR-0010).
    fn expected(path: PathBuf) -> Self {
        Self {
            path: Some(path),
            default: BTreeMap::new(),
            named: BTreeMap::new(),
        }
    }

    fn loaded(
        path: PathBuf,
        default: BTreeMap<String, String>,
        mut named: BTreeMap<String, BTreeMap<String, String>>,
    ) -> Self {
        debug_assert!(
            !named.contains_key("default"),
            "catalogs.default must be folded into default before CatalogTable::loaded"
        );
        let _ = named.remove("default");
        Self {
            path: Some(path),
            default,
            named,
        }
    }

    /// `None` selects the default catalog.
    fn lookup(&self, catalog_name: Option<&str>, package: &str) -> Option<&str> {
        match catalog_name {
            None => self.default.get(package).map(String::as_str),
            Some(name) => self
                .named
                .get(name)
                .and_then(|catalog| catalog.get(package))
                .map(String::as_str),
        }
    }
}

fn catalogs_for_members(
    packages: &[(String, Version, String, PackageManifest)],
    repository_root: &Path,
) -> Result<CatalogTable, DiscoverError> {
    // Prefer repository-root yaml (fixtures / list-only path) before walking up
    // from a member.
    let root_yaml = repository_root.join("pnpm-workspace.yaml");
    match fs::metadata(&root_yaml) {
        Ok(meta) if meta.is_file() => return load_pnpm_catalogs(&root_yaml),
        Ok(_) => {
            return Err(DiscoverError::InvalidMetadata {
                message: format!("{} exists but is not a regular file", root_yaml.display()),
            });
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(DiscoverError::Io {
                path: root_yaml,
                source,
            });
        }
    }

    let Some((_, _, path, _)) = packages.first() else {
        return Ok(CatalogTable::empty());
    };
    let member = normalize_for_containment(Path::new(path))?;
    match find_pnpm_workspace_yaml(&member, repository_root)? {
        Some(yaml) => load_pnpm_catalogs(&yaml),
        None => Ok(CatalogTable::expected(root_yaml)),
    }
}

fn load_catalogs_at(workspace_dir: &Path) -> Result<CatalogTable, DiscoverError> {
    let yaml = workspace_dir.join("pnpm-workspace.yaml");
    match fs::metadata(&yaml) {
        Ok(meta) if meta.is_file() => load_pnpm_catalogs(&yaml),
        Ok(_) => Err(DiscoverError::InvalidMetadata {
            message: format!("{} exists but is not a regular file", yaml.display()),
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(CatalogTable::expected(yaml)),
        Err(source) => Err(DiscoverError::Io { path: yaml, source }),
    }
}

fn find_pnpm_workspace_yaml(
    start: &Path,
    repository_root: &Path,
) -> Result<Option<PathBuf>, DiscoverError> {
    let mut current = start;
    loop {
        let candidate = current.join("pnpm-workspace.yaml");
        match fs::metadata(&candidate) {
            Ok(meta) if meta.is_file() => return Ok(Some(candidate)),
            Ok(_) => {
                return Err(DiscoverError::InvalidMetadata {
                    message: format!("{} exists but is not a regular file", candidate.display()),
                });
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(DiscoverError::Io {
                    path: candidate,
                    source,
                });
            }
        }
        if current == repository_root {
            return Ok(None);
        }
        let Some(parent) = current.parent() else {
            return Ok(None);
        };
        if !parent.starts_with(repository_root) && parent != repository_root {
            return Ok(None);
        }
        current = parent;
    }
}

fn load_pnpm_catalogs(path: &Path) -> Result<CatalogTable, DiscoverError> {
    let text = fs::read_to_string(path).map_err(|source| DiscoverError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let file = CatalogFile::parse(&text).map_err(|err| DiscoverError::InvalidMetadata {
        message: format!("{}: {err}", path.display()),
    })?;
    if file.has_null_named_table() {
        return Err(DiscoverError::InvalidMetadata {
            message: format!("{}: named catalog is null", path.display()),
        });
    }
    if matches!(
        catalog_target(None, "", file.catalog.is_some(), file.has_default_table(),),
        CatalogTarget::Duplicate
    ) {
        return Err(DiscoverError::InvalidMetadata {
            message: format!(
                "{}: the 'default' catalog was defined multiple times. Use the 'catalog' field or 'catalogs.default', but not both",
                path.display()
            ),
        });
    }
    let mut named = CatalogFile::string_tables(file.catalogs.unwrap_or_default());
    let default = match file.catalog {
        Some(catalog) => CatalogFile::string_pins(catalog),
        None => named.remove("default").unwrap_or_default(),
    };
    Ok(CatalogTable::loaded(path.to_path_buf(), default, named))
}

#[derive(Debug, Deserialize)]
struct ListEntry {
    name: Option<String>,
    version: Option<String>,
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PackageBin {
    Command(String),
    Map(BTreeMap<String, String>),
}

#[derive(Debug, Deserialize)]
struct PackageManifest {
    #[serde(default)]
    private: bool,
    #[serde(default)]
    bin: Option<PackageBin>,
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "devDependencies")]
    dev_dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "peerDependencies")]
    peer_dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "optionalDependencies")]
    optional_dependencies: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::test_fixture::Fixture;

    fn fixture_dir(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/pnpm-discover")
            .join(name)
    }

    fn parse_range(dependency: &str, text: &str) -> Result<DeclaredRange, DiscoverError> {
        parse_npm_declared_range("app", dependency, text)
    }

    fn parse_range_with_catalogs(
        dependency: &str,
        text: &str,
        catalogs: &CatalogTable,
    ) -> Result<DeclaredRange, DiscoverError> {
        resolve_catalog_dependency("app", dependency, text, catalogs).map(|(_, range)| range)
    }

    #[test]
    fn private_package_is_not_publishable() {
        let root = fixture_dir("workspace");
        let workspace = discover_pnpm(&root, &root).expect("discover");
        let config = workspace
            .get(&PackageId::new(Ecosystem::Npm, "@oakum/config"))
            .expect("config");
        let core = workspace
            .get(&PackageId::new(Ecosystem::Npm, "@oakum/core"))
            .expect("core");
        assert!(!config.publishable());
        assert!(core.publishable());
    }

    #[test]
    fn lone_package_with_bin_is_build_resolution() {
        let root = fixture_dir("lone");
        let workspace = discover_pnpm(&root, &root).expect("discover");
        assert_eq!(workspace.packages().count(), 1);
        let pkg = workspace
            .get(&PackageId::new(Ecosystem::Npm, "@oakum/lone"))
            .expect("lone");
        assert_eq!(
            pkg.resolves_dependencies_at(),
            ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget)
        );
        assert!(!pkg.publishable());
        assert!(pkg.dependencies().is_empty());
        assert_eq!(pkg.manifest_dir(), "");
        assert_eq!(workspace.catalog_file(), None);
    }

    #[test]
    fn workspace_star_edge_is_tracking_exact() {
        let root = fixture_dir("workspace");
        let workspace = discover_pnpm(&root, &root).expect("discover");
        assert_eq!(workspace.packages().count(), 3);
        assert_eq!(workspace.catalog_file(), Some("pnpm-workspace.yaml"));
        assert_eq!(
            workspace
                .get(&PackageId::new(Ecosystem::Npm, "@oakum/app"))
                .expect("app")
                .manifest_dir(),
            "packages/app"
        );
        assert_eq!(
            workspace
                .get(&PackageId::new(Ecosystem::Npm, "@oakum/core"))
                .expect("core")
                .manifest_dir(),
            "packages/core"
        );
        assert_eq!(
            workspace
                .get(&PackageId::new(Ecosystem::Npm, "@oakum/config"))
                .expect("config")
                .manifest_dir(),
            "packages/config"
        );
        let core = workspace
            .get(&PackageId::new(Ecosystem::Npm, "@oakum/core"))
            .expect("core");
        assert_eq!(
            core.resolves_dependencies_at(),
            ResolvesDependenciesAt::Install
        );
        let app = workspace
            .get(&PackageId::new(Ecosystem::Npm, "@oakum/app"))
            .expect("app");
        assert_eq!(
            app.resolves_dependencies_at(),
            ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget)
        );
        let edge = app
            .dependencies()
            .iter()
            .find(|d| d.on.name == "@oakum/core" && d.declared_as == "@oakum/core")
            .expect("edge");
        assert_eq!(edge.kind, DependencyKind::Normal);
        assert_eq!(
            edge.range,
            DeclaredRange::WorkspaceTracking(Tracking::Exact)
        );
    }

    #[test]
    fn aliased_workspace_dependency_keeps_declared_as() {
        let root = fixture_dir("workspace");
        let app = discover_pnpm(&root, &root)
            .expect("discover")
            .get(&PackageId::new(Ecosystem::Npm, "@oakum/app"))
            .expect("app")
            .clone();
        let edge = app
            .dependencies()
            .iter()
            .find(|d| d.declared_as == "core-alias")
            .expect("alias edge");
        assert_eq!(edge.on.name, "@oakum/core");
        assert_eq!(
            edge.range,
            DeclaredRange::WorkspaceTracking(Tracking::Exact)
        );
    }

    #[test]
    fn development_edge_resolves_default_catalog() {
        let root = fixture_dir("workspace");
        let app = discover_pnpm(&root, &root)
            .expect("discover")
            .get(&PackageId::new(Ecosystem::Npm, "@oakum/app"))
            .expect("app")
            .clone();
        let edge = app
            .dependencies()
            .iter()
            .find(|d| d.on.name == "@oakum/config" && d.kind == DependencyKind::Development)
            .expect("dev edge");
        assert_eq!(
            edge.range,
            DeclaredRange::Catalog {
                name: None,
                bounds: Bounds::from_npm_text("^1.0.0").expect("bounds"),
            }
        );
    }

    #[test]
    fn peer_and_optional_edges_keep_kind() {
        let root = fixture_dir("workspace");
        let app = discover_pnpm(&root, &root)
            .expect("discover")
            .get(&PackageId::new(Ecosystem::Npm, "@oakum/app"))
            .expect("app")
            .clone();
        let peer = app
            .dependencies()
            .iter()
            .find(|d| d.on.name == "@oakum/config" && d.kind == DependencyKind::Peer)
            .expect("peer edge");
        assert_eq!(
            peer.range,
            DeclaredRange::WorkspaceTracking(Tracking::Exact)
        );
        let optional = app
            .dependencies()
            .iter()
            .find(|d| d.on.name == "@oakum/config" && d.kind == DependencyKind::Optional)
            .expect("optional edge");
        assert_eq!(
            optional.range,
            DeclaredRange::Catalog {
                name: Some(String::from("pinned")),
                bounds: Bounds::from_npm_text("1.5.0").expect("bounds"),
            }
        );
    }

    #[test]
    fn optional_override_shadows_same_named_dependency() {
        let root = fixture_dir("optional-override");
        let app = discover_pnpm(&root, &root)
            .expect("discover")
            .get(&PackageId::new(Ecosystem::Npm, "@oakum/app"))
            .expect("app")
            .clone();
        let lib_edges: Vec<_> = app
            .dependencies()
            .iter()
            .filter(|d| d.declared_as == "@oakum/lib")
            .collect();
        assert_eq!(
            lib_edges.len(),
            1,
            "one effective edge for the shadowed key, got {lib_edges:?}"
        );
        assert_eq!(lib_edges[0].kind, DependencyKind::Optional);
        assert_eq!(
            lib_edges[0].range,
            DeclaredRange::Plain(Bounds::from_npm_text(">=0.1.0").expect("bounds")),
            "cascade input must be the effective optional range, not the shadowed exact one"
        );
    }

    #[test]
    fn optional_override_applies_when_the_optional_side_is_catalog_resolved() {
        let root = fixture_dir("optional-override");
        let app = discover_pnpm(&root, &root)
            .expect("discover")
            .get(&PackageId::new(Ecosystem::Npm, "@oakum/app"))
            .expect("app")
            .clone();
        let edges: Vec<_> = app
            .dependencies()
            .iter()
            .filter(|d| d.declared_as == "@oakum/other")
            .collect();
        assert_eq!(
            edges.len(),
            1,
            "the catalog-resolved optional entry must shadow the workspace \
             dependency, got {edges:?}"
        );
        assert_eq!(edges[0].kind, DependencyKind::Optional);
        assert_eq!(
            edges[0].range,
            DeclaredRange::Catalog {
                name: None,
                bounds: Bounds::from_npm_text("^0.1.0").expect("bounds"),
            }
        );
    }

    #[test]
    fn optional_override_by_a_non_member_drops_the_edge_entirely() {
        let root = fixture_dir("optional-override");
        let app = discover_pnpm(&root, &root)
            .expect("discover")
            .get(&PackageId::new(Ecosystem::Npm, "@oakum/app"))
            .expect("app")
            .clone();
        let edges: Vec<_> = app
            .dependencies()
            .iter()
            .filter(|d| d.declared_as == "ext-dep")
            .collect();
        assert!(
            edges.is_empty(),
            "a member edge shadowed by a non-member optional alias must \
             vanish, not fall back to the shadowed member edge: {edges:?}"
        );
    }

    #[test]
    fn optional_override_alias_targets_only_the_effective_package() {
        let root = fixture_dir("optional-override");
        let app = discover_pnpm(&root, &root)
            .expect("discover")
            .get(&PackageId::new(Ecosystem::Npm, "@oakum/app"))
            .expect("app")
            .clone();
        let alias_edges: Vec<_> = app
            .dependencies()
            .iter()
            .filter(|d| d.declared_as == "alias-dep")
            .collect();
        assert_eq!(
            alias_edges.len(),
            1,
            "one effective edge for the shadowed alias, got {alias_edges:?}"
        );
        assert_eq!(
            alias_edges[0].on.name, "@oakum/other",
            "the shadowed alias must not create an edge to @oakum/lib"
        );
        assert_eq!(alias_edges[0].kind, DependencyKind::Optional);
    }

    #[test]
    fn workspace_tilde_is_tracking_tilde() {
        let range = parse_range("core", "workspace:~").expect("parse");
        assert_eq!(range, DeclaredRange::WorkspaceTracking(Tracking::Tilde));
    }

    #[test]
    fn empty_workspace_protocol_is_tracking_exact() {
        let range = parse_range("core", "workspace:").expect("parse");
        assert_eq!(range, DeclaredRange::WorkspaceTracking(Tracking::Exact));
    }

    #[test]
    fn versioned_workspace_protocol_is_workspace_bounds() {
        let range = parse_range("core", "workspace:^1.2.3").expect("parse");
        assert_eq!(
            range,
            DeclaredRange::Workspace(Bounds::from_npm_text("^1.2.3").expect("bounds"))
        );
    }

    #[test]
    fn aliased_versioned_workspace_protocol_splits_name() {
        assert_eq!(
            resolve_dependency_name("core-alias", "workspace:@oakum/core@^1.0.0"),
            Some("@oakum/core")
        );
        let range = parse_range("core-alias", "workspace:@oakum/core@^1.0.0").expect("parse");
        assert_eq!(
            range,
            DeclaredRange::Workspace(Bounds::from_npm_text("^1.0.0").expect("bounds"))
        );
    }

    #[test]
    fn numeric_workspace_package_alias_splits_name() {
        assert_eq!(resolve_dependency_name("alias", "workspace:1@*"), Some("1"));
        let range = parse_range("alias", "workspace:1@*").expect("parse");
        assert_eq!(range, DeclaredRange::WorkspaceTracking(Tracking::Exact));
    }

    #[test]
    fn versionless_list_entry_outside_repository_is_rejected() {
        let repo = scratch("list-versionless-outside");
        let outside = scratch("list-versionless-outside-root");
        let inside = repo.join("pkg");
        fs::create_dir_all(&inside).expect("mkdir");
        fs::write(
            inside.join("package.json"),
            r#"{"name":"@oakum/inside","version":"1.0.0"}"#,
        )
        .expect("pkg");
        let json = format!(
            r#"[
              {{"name":"stray-root","path":"{}","private":true}},
              {{"name":"@oakum/inside","version":"1.0.0","path":"{}"}}
            ]"#,
            outside.display(),
            inside.display()
        );
        let err = workspace_from_pnpm_list(&json, &repo).expect_err("versionless outside");
        assert!(
            matches!(err, DiscoverError::WorkspaceRootOutsideRepository { .. }),
            "{err}"
        );
    }

    #[test]
    fn npm_protocol_is_plain_bounds() {
        assert_eq!(
            resolve_dependency_name("core-alias", "npm:@oakum/core@^1.0.0"),
            Some("@oakum/core")
        );
        let range = parse_range("core-alias", "npm:@oakum/core@^1.0.0").expect("parse");
        assert_eq!(
            range,
            DeclaredRange::Plain(Bounds::from_npm_text("^1.0.0").expect("bounds"))
        );
    }

    #[test]
    fn npm_protocol_without_version_is_refused() {
        let err = parse_range("core-alias", "npm:@oakum/core").expect_err("version");
        assert!(matches!(err, DiscoverError::Range { .. }), "{err}");
    }

    #[test]
    fn malformed_npm_alias_to_member_is_refused_not_omitted() {
        let repo = scratch("malformed-npm");
        let pkg = repo.join("pkg");
        let app = repo.join("app");
        fs::create_dir_all(&pkg).expect("mkdir pkg");
        fs::create_dir_all(&app).expect("mkdir app");
        fs::write(
            pkg.join("package.json"),
            r#"{"name":"@oakum/core","version":"1.0.0"}"#,
        )
        .expect("core");
        fs::write(
            app.join("package.json"),
            r#"{"name":"@oakum/app","version":"1.0.0","dependencies":{"core-alias":"npm:@oakum/core"}}"#,
        )
        .expect("app");
        let json = format!(
            r#"[
              {{"name":"@oakum/core","version":"1.0.0","path":"{}"}},
              {{"name":"@oakum/app","version":"1.0.0","path":"{}"}}
            ]"#,
            pkg.display(),
            app.display()
        );
        let err = workspace_from_pnpm_list(&json, &repo).expect_err("malformed npm");
        assert!(matches!(err, DiscoverError::Range { .. }), "{err}");
    }

    /// Two members under one name would otherwise resolve to whichever sorted
    /// last, and dependent bumping would target the wrong package.
    #[test]
    fn duplicate_package_name_is_refused() {
        let repo = scratch("duplicate-name");
        for dir in ["one", "two"] {
            let member = repo.join(dir);
            fs::create_dir_all(&member).expect("mkdir");
            fs::write(
                member.join("package.json"),
                r#"{"name":"@oakum/dup","version":"1.0.0"}"#,
            )
            .expect("pkg");
        }
        let json = format!(
            r#"[
              {{"name":"@oakum/dup","version":"1.0.0","path":"{}"}},
              {{"name":"@oakum/dup","version":"1.0.0","path":"{}"}}
            ]"#,
            repo.join("one").display(),
            repo.join("two").display()
        );
        let err = workspace_from_pnpm_list(&json, &repo).expect_err("duplicate name");
        assert!(
            err.to_string()
                .contains("duplicate package name @oakum/dup"),
            "{err}"
        );
    }

    /// An empty workspace must be named, not returned: a release would
    /// otherwise proceed against nothing.
    #[test]
    fn a_list_of_only_versionless_entries_is_refused() {
        let repo = scratch("no-versioned");
        let member = repo.join("private");
        fs::create_dir_all(&member).expect("mkdir");
        fs::write(member.join("package.json"), r#"{"name":"@oakum/private"}"#).expect("pkg");
        let json = format!(
            r#"[{{"name":"@oakum/private","path":"{}"}}]"#,
            member.display()
        );
        let err = workspace_from_pnpm_list(&json, &repo).expect_err("no versioned packages");
        assert!(err.to_string().contains("no versioned packages"), "{err}");
    }

    /// Skipping it silently would drop a package from the graph with no
    /// diagnostic — "we didn't look" collapsed into "it's fine".
    #[test]
    fn a_versioned_entry_without_a_path_is_refused() {
        let repo = scratch("versioned-no-path");
        let err = workspace_from_pnpm_list(r#"[{"name":"@oakum/ghost","version":"1.0.0"}]"#, &repo)
            .expect_err("missing path");
        assert!(
            err.to_string()
                .contains("pnpm list entry @oakum/ghost missing path"),
            "{err}"
        );
    }

    #[test]
    fn catalog_protocol_resolves_from_catalog_table() {
        let catalogs = CatalogTable::loaded(
            PathBuf::from("pnpm-workspace.yaml"),
            BTreeMap::from([("@oakum/core".into(), "^1.0.0".into())]),
            BTreeMap::from([(
                "pinned".into(),
                BTreeMap::from([("@oakum/core".into(), "1.5.0".into())]),
            )]),
        );
        let bare = parse_range_with_catalogs("@oakum/core", "catalog:", &catalogs).expect("bare");
        assert_eq!(
            bare,
            DeclaredRange::Catalog {
                name: None,
                bounds: Bounds::from_npm_text("^1.0.0").expect("bounds"),
            }
        );
        let default_alias = parse_range_with_catalogs("@oakum/core", "catalog:default", &catalogs)
            .expect("default");
        assert_eq!(
            default_alias,
            DeclaredRange::Catalog {
                name: None,
                bounds: Bounds::from_npm_text("^1.0.0").expect("bounds"),
            }
        );
        let named =
            parse_range_with_catalogs("@oakum/core", "catalog:pinned", &catalogs).expect("named");
        assert_eq!(
            named,
            DeclaredRange::Catalog {
                name: Some(String::from("pinned")),
                bounds: Bounds::from_npm_text("1.5.0").expect("bounds"),
            }
        );
    }

    #[test]
    fn catalog_protocol_missing_entry_names_yaml_path() {
        let catalogs = CatalogTable::loaded(
            PathBuf::from("/repo/pnpm-workspace.yaml"),
            BTreeMap::new(),
            BTreeMap::new(),
        );
        let err =
            parse_range_with_catalogs("@oakum/core", "catalog:", &catalogs).expect_err("missing");
        assert!(
            matches!(
                err,
                DiscoverError::UnresolvedCatalog {
                    catalog_name: None,
                    path: Some(ref p),
                    ..
                } if p.ends_with("pnpm-workspace.yaml")
            ),
            "{err}"
        );
        assert!(
            err.to_string().contains("/repo/pnpm-workspace.yaml"),
            "{err}"
        );
    }

    #[test]
    fn catalog_protocol_missing_named_entry_is_refused() {
        let catalogs = CatalogTable::loaded(
            PathBuf::from("/repo/pnpm-workspace.yaml"),
            BTreeMap::new(),
            BTreeMap::from([("pinned".into(), BTreeMap::new())]),
        );
        let err = parse_range_with_catalogs("@oakum/core", "catalog:pinned", &catalogs)
            .expect_err("missing named");
        assert!(
            matches!(
                err,
                DiscoverError::UnresolvedCatalog {
                    catalog_name: Some(ref n),
                    path: Some(_),
                    ..
                } if n == "pinned"
            ),
            "{err}"
        );
    }

    #[test]
    fn catalog_protocol_invalid_range_names_yaml_path() {
        let catalogs = CatalogTable::loaded(
            PathBuf::from("/repo/pnpm-workspace.yaml"),
            BTreeMap::from([("@oakum/core".into(), "!!!".into())]),
            BTreeMap::new(),
        );
        let err =
            parse_range_with_catalogs("@oakum/core", "catalog:", &catalogs).expect_err("bad range");
        assert!(matches!(err, DiscoverError::Range { .. }), "{err}");
        let message = err.to_string();
        assert!(
            message.contains("catalog entry for @oakum/core"),
            "{message}"
        );
        assert!(message.contains("/repo/pnpm-workspace.yaml"), "{message}");
    }

    #[test]
    fn catalog_protocol_without_yaml_is_refused() {
        let bare = parse_range_with_catalogs("core", "catalog:", &CatalogTable::empty())
            .expect_err("catalog");
        assert!(
            matches!(
                bare,
                DiscoverError::UnresolvedCatalog {
                    catalog_name: None,
                    path: None,
                    ..
                }
            ),
            "{bare}"
        );
        let named = parse_range_with_catalogs("core", "catalog:foo", &CatalogTable::empty())
            .expect_err("catalog");
        assert!(
            matches!(
                named,
                DiscoverError::UnresolvedCatalog {
                    catalog_name: Some(ref n),
                    path: None,
                    ..
                } if n == "foo"
            ),
            "{named}"
        );
    }

    #[test]
    fn catalog_edge_without_workspace_yaml_is_refused() {
        let repo = scratch("catalog-no-yaml");
        let pkg = repo.join("pkg");
        let app = repo.join("app");
        fs::create_dir_all(&pkg).expect("mkdir pkg");
        fs::create_dir_all(&app).expect("mkdir app");
        fs::write(
            pkg.join("package.json"),
            r#"{"name":"@oakum/core","version":"1.0.0"}"#,
        )
        .expect("core");
        fs::write(
            app.join("package.json"),
            r#"{"name":"@oakum/app","version":"1.0.0","dependencies":{"@oakum/core":"catalog:"}}"#,
        )
        .expect("app");
        let json = format!(
            r#"[
              {{"name":"@oakum/core","version":"1.0.0","path":"{}"}},
              {{"name":"@oakum/app","version":"1.0.0","path":"{}"}}
            ]"#,
            pkg.display(),
            app.display()
        );
        let err = workspace_from_pnpm_list(&json, &repo).expect_err("no yaml");
        assert!(
            matches!(
                err,
                DiscoverError::UnresolvedCatalog {
                    path: Some(ref p),
                    ..
                } if p.ends_with("pnpm-workspace.yaml")
            ),
            "{err}"
        );
        assert!(err.to_string().contains("pnpm-workspace.yaml"), "{err}");
    }

    #[test]
    fn invalid_workspace_yaml_is_refused() {
        let repo = scratch("catalog-bad-yaml");
        let pkg = repo.join("pkg");
        let app = repo.join("app");
        fs::create_dir_all(&pkg).expect("mkdir pkg");
        fs::create_dir_all(&app).expect("mkdir app");
        fs::write(repo.join("pnpm-workspace.yaml"), "catalog: [\n").expect("yaml");
        fs::write(
            pkg.join("package.json"),
            r#"{"name":"@oakum/core","version":"1.0.0"}"#,
        )
        .expect("core");
        fs::write(
            app.join("package.json"),
            r#"{"name":"@oakum/app","version":"1.0.0","dependencies":{"@oakum/core":"catalog:"}}"#,
        )
        .expect("app");
        let json = format!(
            r#"[
              {{"name":"@oakum/core","version":"1.0.0","path":"{}"}},
              {{"name":"@oakum/app","version":"1.0.0","path":"{}"}}
            ]"#,
            pkg.display(),
            app.display()
        );
        let err = workspace_from_pnpm_list(&json, &repo).expect_err("bad yaml");
        assert!(
            matches!(err, DiscoverError::InvalidMetadata { .. }),
            "{err}"
        );
        assert!(err.to_string().contains("pnpm-workspace.yaml"), "{err}");
    }

    #[test]
    fn catalogs_default_is_default_catalog() {
        let repo = scratch("catalogs-default");
        let pkg = repo.join("pkg");
        let dep = repo.join("dep");
        fs::create_dir_all(&pkg).expect("mkdir pkg");
        fs::create_dir_all(&dep).expect("mkdir dep");
        fs::write(
            repo.join("pnpm-workspace.yaml"),
            "packages:\n  - '*'\ncatalogs:\n  default:\n    '@oakum/dep': '^1.0.0'\n",
        )
        .expect("yaml");
        fs::write(
            pkg.join("package.json"),
            r#"{"name":"@oakum/pkg","version":"1.0.0","dependencies":{"@oakum/dep":"catalog:"}}"#,
        )
        .expect("pkg");
        fs::write(
            dep.join("package.json"),
            r#"{"name":"@oakum/dep","version":"1.0.0"}"#,
        )
        .expect("dep");
        let json = format!(
            r#"[{{"name":"@oakum/pkg","version":"1.0.0","path":"{}"}},{{"name":"@oakum/dep","version":"1.0.0","path":"{}"}}]"#,
            pkg.display(),
            dep.display()
        );
        let workspace = workspace_from_pnpm_list(&json, &repo).expect("discover");
        let edge = workspace
            .get(&PackageId::new(Ecosystem::Npm, "@oakum/pkg"))
            .expect("pkg")
            .dependencies()
            .iter()
            .find(|d| d.on.name == "@oakum/dep")
            .expect("edge");
        assert!(matches!(
            &edge.range,
            DeclaredRange::Catalog { name: None, .. }
        ));
    }

    #[test]
    fn dual_default_catalog_definition_is_refused() {
        let repo = scratch("dual-default");
        let pkg = repo.join("pkg");
        fs::create_dir_all(&pkg).expect("mkdir");
        fs::write(
            repo.join("pnpm-workspace.yaml"),
            "packages:\n  - 'pkg'\ncatalog:\n  lodash: '^4.0.0'\ncatalogs:\n  default:\n    lodash: '^4.0.0'\n",
        )
        .expect("yaml");
        fs::write(
            pkg.join("package.json"),
            r#"{"name":"@oakum/pkg","version":"1.0.0","dependencies":{"lodash":"catalog:"}}"#,
        )
        .expect("pkg");
        let json = format!(
            r#"[{{"name":"@oakum/pkg","version":"1.0.0","path":"{}"}}]"#,
            pkg.display()
        );
        let err = workspace_from_pnpm_list(&json, &repo).expect_err("dual default");
        assert!(
            matches!(err, DiscoverError::InvalidMetadata { .. }),
            "{err}"
        );
        assert!(err.to_string().contains("defined multiple times"), "{err}");
    }

    #[test]
    fn null_named_catalog_table_is_refused() {
        let repo = scratch("null-named-catalog");
        let pkg = repo.join("pkg");
        fs::create_dir_all(&pkg).expect("mkdir");
        fs::write(
            repo.join("pnpm-workspace.yaml"),
            "packages:\n  - 'pkg'\ncatalogs:\n  default: null\n",
        )
        .expect("yaml");
        fs::write(
            pkg.join("package.json"),
            r#"{"name":"@oakum/pkg","version":"1.0.0","dependencies":{"lodash":"catalog:"}}"#,
        )
        .expect("pkg");
        let json = format!(
            r#"[{{"name":"@oakum/pkg","version":"1.0.0","path":"{}"}}]"#,
            pkg.display()
        );
        let err = workspace_from_pnpm_list(&json, &repo).expect_err("null named");
        assert!(
            matches!(err, DiscoverError::InvalidMetadata { .. }),
            "{err}"
        );
        assert!(err.to_string().contains("named catalog is null"), "{err}");
    }

    #[test]
    fn null_non_default_named_catalog_table_is_refused() {
        let repo = scratch("null-pinned-catalog");
        let pkg = repo.join("pkg");
        fs::create_dir_all(&pkg).expect("mkdir");
        fs::write(
            repo.join("pnpm-workspace.yaml"),
            "packages:\n  - 'pkg'\ncatalog:\n  lodash: '^4.0.0'\ncatalogs:\n  pinned: null\n",
        )
        .expect("yaml");
        fs::write(
            pkg.join("package.json"),
            r#"{"name":"@oakum/pkg","version":"1.0.0","dependencies":{"lodash":"catalog:"}}"#,
        )
        .expect("pkg");
        let json = format!(
            r#"[{{"name":"@oakum/pkg","version":"1.0.0","path":"{}"}}]"#,
            pkg.display()
        );
        let err = workspace_from_pnpm_list(&json, &repo).expect_err("null pinned");
        assert!(
            matches!(err, DiscoverError::InvalidMetadata { .. }),
            "{err}"
        );
        assert!(err.to_string().contains("named catalog is null"), "{err}");
    }

    #[test]
    fn catalog_npm_alias_to_member_keeps_edge() {
        let repo = scratch("catalog-alias");
        let pkg = repo.join("pkg");
        let core = repo.join("core");
        fs::create_dir_all(&pkg).expect("mkdir pkg");
        fs::create_dir_all(&core).expect("mkdir core");
        fs::write(
            repo.join("pnpm-workspace.yaml"),
            "packages:\n  - '*'\ncatalog:\n  core-legacy: 'npm:@oakum/core@^1.0.0'\n",
        )
        .expect("yaml");
        fs::write(
            pkg.join("package.json"),
            r#"{"name":"@oakum/pkg","version":"1.0.0","dependencies":{"core-legacy":"catalog:"}}"#,
        )
        .expect("pkg");
        fs::write(
            core.join("package.json"),
            r#"{"name":"@oakum/core","version":"1.0.0"}"#,
        )
        .expect("core");
        let json = format!(
            r#"[{{"name":"@oakum/pkg","version":"1.0.0","path":"{}"}},{{"name":"@oakum/core","version":"1.0.0","path":"{}"}}]"#,
            pkg.display(),
            core.display()
        );
        let workspace = workspace_from_pnpm_list(&json, &repo).expect("discover");
        let edge = workspace
            .get(&PackageId::new(Ecosystem::Npm, "@oakum/pkg"))
            .expect("pkg")
            .dependencies()
            .iter()
            .find(|d| d.on.name == "@oakum/core")
            .expect("aliased edge");
        assert_eq!(edge.declared_as, "core-legacy");
        assert!(matches!(
            &edge.range,
            DeclaredRange::Catalog { name: None, .. }
        ));
    }

    #[test]
    fn external_catalog_edge_still_resolves() {
        let repo = scratch("catalog-external");
        let pkg = repo.join("pkg");
        fs::create_dir_all(&pkg).expect("mkdir");
        fs::write(
            repo.join("pnpm-workspace.yaml"),
            "packages:\n  - 'pkg'\ncatalog:\n  lodash: '^4.0.0'\n",
        )
        .expect("yaml");
        fs::write(
            pkg.join("package.json"),
            r#"{"name":"@oakum/pkg","version":"1.0.0","dependencies":{"lodash":"catalog:"}}"#,
        )
        .expect("pkg");
        let json = format!(
            r#"[{{"name":"@oakum/pkg","version":"1.0.0","path":"{}"}}]"#,
            pkg.display()
        );
        let workspace = workspace_from_pnpm_list(&json, &repo).expect("discover");
        let pkg = workspace
            .get(&PackageId::new(Ecosystem::Npm, "@oakum/pkg"))
            .expect("pkg");
        assert!(pkg.dependencies().is_empty());
    }

    #[test]
    fn external_catalog_missing_entry_is_refused() {
        let repo = scratch("catalog-external-miss");
        let pkg = repo.join("pkg");
        fs::create_dir_all(&pkg).expect("mkdir");
        fs::write(
            repo.join("pnpm-workspace.yaml"),
            "packages:\n  - 'pkg'\ncatalog: {}\n",
        )
        .expect("yaml");
        fs::write(
            pkg.join("package.json"),
            r#"{"name":"@oakum/pkg","version":"1.0.0","dependencies":{"lodash":"catalog:"}}"#,
        )
        .expect("pkg");
        let json = format!(
            r#"[{{"name":"@oakum/pkg","version":"1.0.0","path":"{}"}}]"#,
            pkg.display()
        );
        let err = workspace_from_pnpm_list(&json, &repo).expect_err("missing external");
        assert!(
            matches!(err, DiscoverError::UnresolvedCatalog { .. }),
            "{err}"
        );
    }

    #[test]
    fn relative_workspace_protocol_is_refused() {
        for declared in ["workspace:../core", "workspace:/abs/core"] {
            let err = parse_range("core", declared).expect_err("relative");
            assert!(
                matches!(err, DiscoverError::Range { .. }),
                "{declared}: {err}"
            );
        }
    }

    #[test]
    fn list_member_outside_repository_is_rejected() {
        let repo = scratch("list-outside");
        let outside = scratch("list-outside-member");
        fs::write(
            outside.join("package.json"),
            r#"{"name":"@oakum/escaped","version":"1.0.0"}"#,
        )
        .expect("pkg");
        let json = format!(
            r#"[{{"name":"@oakum/escaped","version":"1.0.0","path":"{}"}}]"#,
            outside.display()
        );
        let err = workspace_from_pnpm_list(&json, &repo).expect_err("outside member");
        assert!(
            matches!(err, DiscoverError::WorkspaceRootOutsideRepository { .. }),
            "{err}"
        );
    }

    #[test]
    fn list_member_parent_escape_is_rejected() {
        let repo = scratch("list-dotdot");
        let escaped = repo
            .join("..")
            .join(format!("pnpm-escaped-{}", std::process::id()));
        // Path must not exist so normalize takes the absolute+lexical fallback.
        assert!(!escaped.exists());
        let json = format!(
            r#"[{{"name":"@oakum/escaped","version":"1.0.0","path":"{}"}}]"#,
            escaped.display()
        );
        let err = workspace_from_pnpm_list(&json, &repo).expect_err("dotdot escape");
        assert!(
            matches!(err, DiscoverError::WorkspaceRootOutsideRepository { .. }),
            "{err}"
        );
    }

    #[test]
    fn subdirectory_workspace_is_accepted_under_repo_root() {
        let repo = scratch("subdir");
        let js = repo.join("js");
        fs::create_dir_all(js.join("packages").join("pkg")).expect("mkdir");
        fs::write(
            js.join("pnpm-workspace.yaml"),
            "packages:\n  - \"packages/*\"\n",
        )
        .expect("workspace yaml");
        fs::write(
            js.join("package.json"),
            r#"{"name":"js-root","private":true}"#,
        )
        .expect("root pkg");
        fs::write(
            js.join("packages").join("pkg").join("package.json"),
            r#"{"name":"@oakum/pkg","version":"1.0.0"}"#,
        )
        .expect("pkg");

        let workspace = discover_pnpm(&js, &repo).expect("subdir workspace");
        assert_eq!(workspace.packages().count(), 1);
        let pkg = workspace
            .get(&PackageId::new(Ecosystem::Npm, "@oakum/pkg"))
            .expect("pkg");
        assert_eq!(pkg.manifest_dir(), "js/packages/pkg");
    }

    #[test]
    fn stray_ancestor_aborts() {
        let stray = scratch("stray");
        let nested = stray.join("deep").join("nested");
        fs::create_dir_all(stray.join("packages").join("a")).expect("mkdir");
        fs::create_dir_all(&nested).expect("mkdir nested");
        fs::write(
            stray.join("pnpm-workspace.yaml"),
            "packages:\n  - \"packages/*\"\n",
        )
        .expect("workspace yaml");
        fs::write(
            stray.join("package.json"),
            r#"{"name":"stray-root","private":true}"#,
        )
        .expect("root pkg");
        fs::write(
            stray.join("packages").join("a").join("package.json"),
            r#"{"name":"pkg-a","version":"1.0.0"}"#,
        )
        .expect("pkg-a");
        fs::write(
            nested.join("package.json"),
            r#"{"name":"nested-lone","version":"9.9.9","private":true}"#,
        )
        .expect("nested");

        let err = discover_pnpm(&nested, &nested).expect_err("stray");
        assert!(
            matches!(err, DiscoverError::WorkspaceRootOutsideRepository { .. }),
            "{err}"
        );
    }

    fn scratch(label: &str) -> Fixture {
        Fixture::new("discover", label)
    }
}
