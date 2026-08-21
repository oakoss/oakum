//! pnpm adapter: `pnpm root -w` + `pnpm list -r --depth -1 --json`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use semver::Version;
use serde::Deserialize;

use super::paths::{ensure_contained, normalize_for_containment};
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
/// Edges come from each member's `package.json`. `catalog:` is refused until
/// okm-1t8. A `bin` field maps to [`ResolvesDependenciesAt::Build`] with
/// [`BuildResolution::BinaryTarget`].
///
/// # Errors
///
/// Returns [`DiscoverError`] when pnpm fails, the workspace root escapes the
/// repository, JSON is unusable, a `catalog:` edge appears, or
/// [`Workspace::new`] refuses the graph.
pub fn discover_pnpm(
    package_dir: impl AsRef<Path>,
    repository_root: impl AsRef<Path>,
) -> Result<Workspace, DiscoverError> {
    let package_dir = package_dir.as_ref();
    let repository_root = normalize_for_containment(repository_root.as_ref())?;
    probe_pnpm_root(package_dir, &repository_root)?;
    let json = run_pnpm_list(package_dir)?;
    workspace_from_pnpm_list(&json, &repository_root)
}

/// Build a [`Workspace`] from `pnpm list -r --depth -1 --json` output.
///
/// Reads each entry's `package.json` via `path`. Skips entries without a
/// `version` (typical of private workspace roots). Every member path must be
/// contained in `repository_root`, same rule as the `pnpm root -w` probe.
/// Both roots go through [`normalize_for_containment`].
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
    let mut by_name = BTreeMap::new();
    let mut packages = Vec::new();

    for entry in &entries {
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
        ensure_contained(&member_path, &repository_root)?;

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

    let member_names: BTreeSet<String> = packages
        .iter()
        .map(|(name, _, _, _)| name.clone())
        .collect();
    let mut mapped = Vec::with_capacity(packages.len());
    for (name, version, _path, manifest) in packages {
        mapped.push(map_package(name, version, &manifest, &member_names)?);
    }
    Ok(Workspace::new(mapped)?)
}

fn probe_pnpm_root(package_dir: &Path, repository_root: &Path) -> Result<(), DiscoverError> {
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
        return ensure_contained(&root, repository_root);
    }

    if is_not_in_workspace(&stderr) {
        return Ok(());
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
) -> Result<Package, DiscoverError> {
    let resolves = if package_has_bin(manifest) {
        ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget)
    } else {
        ResolvesDependenciesAt::Install
    };

    let mut dependencies = Vec::new();
    for (kind, deps) in [
        (DependencyKind::Normal, &manifest.dependencies),
        (DependencyKind::Peer, &manifest.peer_dependencies),
        (DependencyKind::Optional, &manifest.optional_dependencies),
        (DependencyKind::Development, &manifest.dev_dependencies),
    ] {
        for (declared_as, range_text) in deps {
            let Some(on_name) = resolve_dependency_name(declared_as, range_text) else {
                // Malformed npm:/workspace:/catalog: must not vanish as "not a member".
                let trimmed = range_text.trim();
                if trimmed.starts_with("npm:")
                    || trimmed.starts_with("workspace:")
                    || trimmed.starts_with("catalog:")
                {
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
        dependencies,
    ))
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
    if let Some(rest) = text.strip_prefix("catalog:") {
        let rest = rest.trim();
        let name = if rest.is_empty() {
            None
        } else {
            Some(rest.to_owned())
        };
        return Err(DiscoverError::UnresolvedCatalog {
            package: package.to_owned(),
            dependency: dependency.to_owned(),
            catalog_name: name,
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

    fn fixture_dir(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/pnpm-discover")
            .join(name)
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
        assert!(pkg.dependencies().is_empty());
    }

    #[test]
    fn workspace_star_edge_is_tracking_exact() {
        let root = fixture_dir("workspace");
        let workspace = discover_pnpm(&root, &root).expect("discover");
        assert_eq!(workspace.packages().count(), 3);
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
    fn development_edge_keeps_kind_and_caret_tracking() {
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
            DeclaredRange::WorkspaceTracking(Tracking::Caret)
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
            .find(|d| d.on.name == "@oakum/core" && d.kind == DependencyKind::Optional)
            .expect("optional edge");
        assert_eq!(
            optional.range,
            DeclaredRange::WorkspaceTracking(Tracking::Tilde)
        );
    }

    #[test]
    fn workspace_tilde_is_tracking_tilde() {
        let range = parse_npm_declared_range("app", "core", "workspace:~").expect("parse");
        assert_eq!(range, DeclaredRange::WorkspaceTracking(Tracking::Tilde));
    }

    #[test]
    fn empty_workspace_protocol_is_tracking_exact() {
        let range = parse_npm_declared_range("app", "core", "workspace:").expect("parse");
        assert_eq!(range, DeclaredRange::WorkspaceTracking(Tracking::Exact));
    }

    #[test]
    fn versioned_workspace_protocol_is_workspace_bounds() {
        let range = parse_npm_declared_range("app", "core", "workspace:^1.2.3").expect("parse");
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
        let range = parse_npm_declared_range("app", "core-alias", "workspace:@oakum/core@^1.0.0")
            .expect("parse");
        assert_eq!(
            range,
            DeclaredRange::Workspace(Bounds::from_npm_text("^1.0.0").expect("bounds"))
        );
    }

    #[test]
    fn numeric_workspace_package_alias_splits_name() {
        assert_eq!(resolve_dependency_name("alias", "workspace:1@*"), Some("1"));
        let range = parse_npm_declared_range("app", "alias", "workspace:1@*").expect("parse");
        assert_eq!(range, DeclaredRange::WorkspaceTracking(Tracking::Exact));
    }

    #[test]
    fn versionless_list_entry_outside_repository_is_rejected() {
        let repo = tempfile_dir("pnpm-list-versionless-outside");
        let outside = tempfile_dir("pnpm-list-versionless-outside-root");
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
        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn npm_protocol_is_plain_bounds() {
        assert_eq!(
            resolve_dependency_name("core-alias", "npm:@oakum/core@^1.0.0"),
            Some("@oakum/core")
        );
        let range =
            parse_npm_declared_range("app", "core-alias", "npm:@oakum/core@^1.0.0").expect("parse");
        assert_eq!(
            range,
            DeclaredRange::Plain(Bounds::from_npm_text("^1.0.0").expect("bounds"))
        );
    }

    #[test]
    fn npm_protocol_without_version_is_refused() {
        let err =
            parse_npm_declared_range("app", "core-alias", "npm:@oakum/core").expect_err("version");
        assert!(matches!(err, DiscoverError::Range { .. }), "{err}");
    }

    #[test]
    fn malformed_npm_alias_to_member_is_refused_not_omitted() {
        let repo = tempfile_dir("pnpm-malformed-npm");
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
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn catalog_protocol_is_refused() {
        let bare = parse_npm_declared_range("app", "core", "catalog:").expect_err("catalog");
        assert!(
            matches!(
                bare,
                DiscoverError::UnresolvedCatalog {
                    catalog_name: None,
                    ..
                }
            ),
            "{bare}"
        );
        let named = parse_npm_declared_range("app", "core", "catalog:foo").expect_err("catalog");
        assert!(
            matches!(
                named,
                DiscoverError::UnresolvedCatalog {
                    catalog_name: Some(ref n),
                    ..
                } if n == "foo"
            ),
            "{named}"
        );
    }

    #[test]
    fn relative_workspace_protocol_is_refused() {
        for declared in ["workspace:../core", "workspace:/abs/core"] {
            let err = parse_npm_declared_range("app", "core", declared).expect_err("relative");
            assert!(
                matches!(err, DiscoverError::Range { .. }),
                "{declared}: {err}"
            );
        }
    }

    #[test]
    fn list_member_outside_repository_is_rejected() {
        let repo = tempfile_dir("pnpm-list-outside");
        let outside = tempfile_dir("pnpm-list-outside-member");
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
        let _ = fs::remove_dir_all(&repo);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn list_member_parent_escape_is_rejected() {
        let repo = tempfile_dir("pnpm-list-dotdot");
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
        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn subdirectory_workspace_is_accepted_under_repo_root() {
        let repo = tempfile_dir("pnpm-subdir");
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
        assert!(workspace
            .get(&PackageId::new(Ecosystem::Npm, "@oakum/pkg"))
            .is_some());

        let _ = fs::remove_dir_all(&repo);
    }

    #[test]
    fn stray_ancestor_aborts() {
        let stray = tempfile_dir("pnpm-stray");
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

        let _ = fs::remove_dir_all(&stray);
    }

    fn tempfile_dir(prefix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }
}
