//! Cargo adapter: `cargo metadata --format-version 1 --no-deps`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use semver::Version;
use serde::Deserialize;

use super::paths::{canonicalize, ensure_contained};
use super::DiscoverError;
use crate::plan::{
    Bounds, BuildResolution, DeclaredRange, Dependency, DependencyKind, Ecosystem, Package,
    PackageId, ResolvesDependenciesAt, Workspace,
};

/// Discover the Cargo workspace visible from `manifest_dir`.
///
/// `repository_root` bounds cargo's `workspace_root` (containment, not
/// equality) so an ancestor workspace outside the repo cannot absorb packages.
/// Always passes `--no-deps` so a lock-free crate is not mutated. Relays cargo's
/// exit-101 stderr verbatim (stray nested manifests).
///
/// # Errors
///
/// Returns [`DiscoverError`] when cargo fails, the workspace root escapes the
/// repository, JSON/TOML is unusable, or [`Workspace::new`] refuses the graph.
pub fn discover_cargo(
    manifest_dir: impl AsRef<Path>,
    repository_root: impl AsRef<Path>,
) -> Result<Workspace, DiscoverError> {
    let json = run_cargo_metadata(manifest_dir.as_ref())?;
    workspace_from_cargo_metadata(&json, repository_root)
}

/// Build a [`Workspace`] from `cargo metadata` JSON (`--format-version 1
/// --no-deps` shape). May read package manifests for path-linked peeks.
///
/// # Errors
///
/// Same class of failures as [`discover_cargo`], except cargo is not invoked.
pub fn workspace_from_cargo_metadata(
    json: &str,
    repository_root: impl AsRef<Path>,
) -> Result<Workspace, DiscoverError> {
    let meta: Metadata = serde_json::from_str(json)?;
    let repository_root = canonicalize(repository_root.as_ref())?;
    let workspace_root = canonicalize(Path::new(&meta.workspace_root))?;
    ensure_contained(&workspace_root, &repository_root)?;

    let member_ids: BTreeSet<&str> = meta.workspace_members.iter().map(String::as_str).collect();
    let members: Vec<&MetaPackage> = meta
        .packages
        .iter()
        .filter(|package| member_ids.contains(package.id.as_str()))
        .collect();

    if members.is_empty() {
        return Err(DiscoverError::InvalidMetadata {
            message: String::from("cargo metadata reported no workspace members"),
        });
    }

    let mut by_dir = BTreeMap::new();
    for package in &members {
        let dir = package_dir(package)?;
        if by_dir.insert(dir.clone(), *package).is_some() {
            return Err(DiscoverError::InvalidMetadata {
                message: format!(
                    "duplicate workspace member directories for {}",
                    dir.display()
                ),
            });
        }
    }

    let mut packages = Vec::with_capacity(members.len());
    for package in &members {
        packages.push(map_package(package, &by_dir, &workspace_root)?);
    }
    Ok(Workspace::new(packages)?)
}

fn run_cargo_metadata(manifest_dir: &Path) -> Result<String, DiscoverError> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(manifest_dir)
        .output()
        .map_err(|source| DiscoverError::CargoNotRunnable { source })?;

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
        return Err(DiscoverError::CargoMetadata {
            status: output.status.code(),
            message,
        });
    }

    String::from_utf8(output.stdout).map_err(|err| DiscoverError::InvalidMetadata {
        message: format!("stdout was not utf-8: {err}"),
    })
}

fn map_package(
    package: &MetaPackage,
    members_by_dir: &BTreeMap<PathBuf, &MetaPackage>,
    workspace_root: &Path,
) -> Result<Package, DiscoverError> {
    let version = Version::parse(&package.version).map_err(|err| DiscoverError::Version {
        package: package.name.clone(),
        message: err.to_string(),
    })?;

    let resolves = if package
        .targets
        .iter()
        .any(|t| t.kind.iter().any(|k| k == "bin"))
    {
        ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget)
    } else {
        ResolvesDependenciesAt::Install
    };

    let mut dependencies = Vec::new();
    for dep in &package.dependencies {
        // Registry (and other pathless) deps are never intra-workspace edges,
        // even when a workspace member reuses the same package name.
        let Some(dep_path) = dep.path.as_ref() else {
            continue;
        };

        let dep_dir = canonicalize(Path::new(dep_path))?;
        let Some(member) = members_by_dir.get(&dep_dir) else {
            continue;
        };
        if member.name != dep.name {
            return Err(DiscoverError::InvalidMetadata {
                message: format!(
                    "{} path dependency {} resolves to package {}, not {}",
                    package.name, dep_path, member.name, dep.name
                ),
            });
        }
        dependencies.push(map_dependency(package, dep, workspace_root)?);
    }

    Ok(Package::new(
        PackageId::new(Ecosystem::Cargo, package.name.clone()),
        version,
        resolves,
        cargo_is_publishable(package.publish.as_ref()),
        dependencies,
    ))
}

/// Cargo metadata: `null` = any registry; empty list = nowhere; non-empty
/// allow-list = publishable somewhere (not necessarily crates.io).
fn cargo_is_publishable(publish: Option<&Vec<String>>) -> bool {
    match publish {
        None => true,
        Some(registries) => !registries.is_empty(),
    }
}

fn map_dependency(
    dependent: &MetaPackage,
    dep: &MetaDependency,
    workspace_root: &Path,
) -> Result<Dependency, DiscoverError> {
    let kind = match dep.kind.as_deref() {
        None => DependencyKind::Normal,
        Some("dev") => DependencyKind::Development,
        Some("build") => DependencyKind::Build,
        Some(other) => {
            return Err(DiscoverError::UnknownDependencyKind {
                package: dependent.name.clone(),
                dependency: dep.name.clone(),
                kind: other.to_owned(),
            });
        }
    };

    let declared_as = dep.rename.clone().unwrap_or_else(|| dep.name.clone());
    let range = declared_range(dependent, dep, &declared_as, kind, workspace_root)?;

    Ok(Dependency {
        on: PackageId::new(Ecosystem::Cargo, dep.name.clone()),
        kind,
        declared_as,
        target: dep.target.clone(),
        range,
    })
}

fn declared_range(
    dependent: &MetaPackage,
    dep: &MetaDependency,
    declared_as: &str,
    kind: DependencyKind,
    workspace_root: &Path,
) -> Result<DeclaredRange, DiscoverError> {
    // Path + authored `version = "*"` and path-only both report req "*". Only a
    // TOML peek (ADR-0026 / research) distinguishes PathLinked from Plain(*).
    if dep.path.is_some() && dep.req == "*" {
        let manifest = PathBuf::from(&dependent.manifest_path);
        if !dependency_declares_version(
            &manifest,
            workspace_root,
            declared_as,
            kind,
            dep.target.as_deref(),
            &dependent.name,
            &dep.name,
        )? {
            return Ok(DeclaredRange::PathLinked);
        }
    }

    let bounds = Bounds::from_cargo_text(&dep.req).map_err(|err| DiscoverError::Range {
        package: dependent.name.clone(),
        dependency: dep.name.clone(),
        message: err.to_string(),
    })?;
    Ok(DeclaredRange::Plain(bounds))
}

/// True when the authored dep has a `version` key, including via
/// `[workspace.dependencies]` when `workspace = true`.
fn dependency_declares_version(
    manifest_path: &Path,
    workspace_root: &Path,
    declared_as: &str,
    kind: DependencyKind,
    target: Option<&str>,
    package_name: &str,
    dependency_name: &str,
) -> Result<bool, DiscoverError> {
    let entry = dependency_entry(
        manifest_path,
        declared_as,
        kind,
        target,
        package_name,
        dependency_name,
    )?;
    match entry {
        toml::Value::String(_) => Ok(true),
        toml::Value::Table(table) => {
            if table.contains_key("version") {
                return Ok(true);
            }
            if table.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
                return workspace_dependency_declares_version(
                    workspace_root,
                    declared_as,
                    package_name,
                    dependency_name,
                );
            }
            Ok(false)
        }
        other => Err(DiscoverError::Toml {
            path: manifest_path.to_path_buf(),
            message: format!(
                "{package_name} dependency {declared_as}: unexpected TOML value {other:?}"
            ),
        }),
    }
}

fn workspace_dependency_declares_version(
    workspace_root: &Path,
    declared_as: &str,
    package_name: &str,
    dependency_name: &str,
) -> Result<bool, DiscoverError> {
    let root_manifest = workspace_root.join("Cargo.toml");
    let text = fs::read_to_string(&root_manifest).map_err(|source| DiscoverError::Io {
        path: root_manifest.clone(),
        source,
    })?;
    let table: toml::Table = toml::from_str(&text).map_err(|err| DiscoverError::Toml {
        path: root_manifest.clone(),
        message: err.to_string(),
    })?;
    let Some(workspace) = table.get("workspace").and_then(|v| v.as_table()) else {
        return Err(DiscoverError::Toml {
            path: root_manifest,
            message: format!(
                "{package_name} inherits {dependency_name} via workspace = true, but root has no [workspace]"
            ),
        });
    };
    let Some(deps) = workspace.get("dependencies").and_then(|v| v.as_table()) else {
        return Err(DiscoverError::Toml {
            path: root_manifest,
            message: format!(
                "{package_name} inherits {dependency_name} via workspace = true, but root has no [workspace.dependencies]"
            ),
        });
    };
    let Some(entry) = deps.get(declared_as) else {
        return Err(DiscoverError::Toml {
            path: root_manifest,
            message: format!(
                "{package_name} inherits {declared_as}, but [workspace.dependencies] has no such key"
            ),
        });
    };
    match entry {
        toml::Value::String(_) => Ok(true),
        toml::Value::Table(t) => Ok(t.contains_key("version")),
        other => Err(DiscoverError::Toml {
            path: root_manifest,
            message: format!("[workspace.dependencies].{declared_as}: unexpected {other:?}"),
        }),
    }
}

fn dependency_entry(
    manifest_path: &Path,
    declared_as: &str,
    kind: DependencyKind,
    target: Option<&str>,
    package_name: &str,
    dependency_name: &str,
) -> Result<toml::Value, DiscoverError> {
    let text = fs::read_to_string(manifest_path).map_err(|source| DiscoverError::Io {
        path: manifest_path.to_path_buf(),
        source,
    })?;
    let table: toml::Table = toml::from_str(&text).map_err(|err| DiscoverError::Toml {
        path: manifest_path.to_path_buf(),
        message: err.to_string(),
    })?;

    let section = match kind {
        DependencyKind::Normal => "dependencies",
        DependencyKind::Development => "dev-dependencies",
        DependencyKind::Build => "build-dependencies",
        DependencyKind::Peer | DependencyKind::Optional => {
            return Err(DiscoverError::UnknownDependencyKind {
                package: package_name.to_owned(),
                dependency: dependency_name.to_owned(),
                kind: format!("{kind}"),
            });
        }
    };

    let deps_table = if let Some(predicate) = target {
        let Some(targets) = table.get("target").and_then(|v| v.as_table()) else {
            return Err(DiscoverError::Toml {
                path: manifest_path.to_path_buf(),
                message: format!(
                    "{package_name}: metadata listed target {predicate:?} for {declared_as}, but manifest has no [target]"
                ),
            });
        };
        let Some(target_table) = targets.get(predicate).and_then(|v| v.as_table()) else {
            return Err(DiscoverError::Toml {
                path: manifest_path.to_path_buf(),
                message: format!(
                    "{package_name}: metadata listed target {predicate:?} for {declared_as}, missing from manifest"
                ),
            });
        };
        target_table.get(section).and_then(|v| v.as_table())
    } else {
        table.get(section).and_then(|v| v.as_table())
    };

    let Some(deps) = deps_table else {
        return Err(DiscoverError::Toml {
            path: manifest_path.to_path_buf(),
            message: format!(
                "{package_name}: metadata listed {declared_as} under {section}, missing from manifest"
            ),
        });
    };
    let Some(entry) = deps.get(declared_as) else {
        return Err(DiscoverError::Toml {
            path: manifest_path.to_path_buf(),
            message: format!(
                "{package_name}: metadata listed dependency key {declared_as}, missing from manifest"
            ),
        });
    };
    Ok(entry.clone())
}

fn package_dir(package: &MetaPackage) -> Result<PathBuf, DiscoverError> {
    let manifest = Path::new(&package.manifest_path);
    let Some(dir) = manifest.parent() else {
        return Err(DiscoverError::InvalidMetadata {
            message: format!("package {} has no manifest directory", package.name),
        });
    };
    canonicalize(dir)
}

#[derive(Debug, Deserialize)]
struct Metadata {
    packages: Vec<MetaPackage>,
    workspace_members: Vec<String>,
    workspace_root: String,
}

#[derive(Debug, Deserialize)]
struct MetaPackage {
    name: String,
    version: String,
    id: String,
    manifest_path: String,
    /// Always present in `cargo metadata` JSON: `null` = unrestricted, `[]` =
    /// nowhere. No `#[serde(default)]` — a missing key must not look like null.
    publish: Option<Vec<String>>,
    dependencies: Vec<MetaDependency>,
    targets: Vec<MetaTarget>,
}

#[derive(Debug, Deserialize)]
struct MetaDependency {
    name: String,
    req: String,
    kind: Option<String>,
    rename: Option<String>,
    target: Option<String>,
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MetaTarget {
    kind: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::DeclaredRange;

    fn fixture_dir(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/cargo-discover")
            .join(name)
    }

    fn discover(name: &str) -> Workspace {
        let root = fixture_dir(name);
        discover_cargo(&root, &root).expect("discover")
    }

    #[test]
    fn lone_lib_is_one_member_workspace() {
        let root = fixture_dir("lone-lib");
        let workspace = discover_cargo(&root, &root).expect("discover");
        assert_eq!(workspace.packages().count(), 1);
        let pkg = workspace
            .get(&PackageId::new(Ecosystem::Cargo, "lone-lib"))
            .expect("lone-lib");
        assert_eq!(
            pkg.resolves_dependencies_at(),
            ResolvesDependenciesAt::Install
        );
        assert!(
            !root.join("Cargo.lock").exists(),
            "--no-deps must not write a lockfile"
        );
    }

    #[test]
    fn binary_target_marks_build_resolution() {
        let pkg = discover("with-bin")
            .get(&PackageId::new(Ecosystem::Cargo, "with-bin"))
            .expect("with-bin")
            .clone();
        assert_eq!(
            pkg.resolves_dependencies_at(),
            ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget)
        );
    }

    #[test]
    fn path_only_edge_is_path_linked() {
        let edge = discover("path-linked")
            .get(&PackageId::new(Ecosystem::Cargo, "app"))
            .expect("app")
            .dependencies()
            .iter()
            .find(|d| d.on.name == "core")
            .expect("edge")
            .clone();
        assert_eq!(edge.range, DeclaredRange::PathLinked);
    }

    #[test]
    fn path_with_star_version_is_plain_star() {
        let edge = discover("path-star-version")
            .get(&PackageId::new(Ecosystem::Cargo, "app"))
            .expect("app")
            .dependencies()
            .iter()
            .find(|d| d.on.name == "core")
            .expect("edge")
            .clone();
        let expected = DeclaredRange::Plain(Bounds::from_cargo_text("*").expect("*"));
        assert_eq!(edge.range, expected);
    }

    #[test]
    fn workspace_inherited_star_is_plain_star() {
        let edge = discover("workspace-inherit-star")
            .get(&PackageId::new(Ecosystem::Cargo, "app"))
            .expect("app")
            .dependencies()
            .iter()
            .find(|d| d.on.name == "core")
            .expect("edge")
            .clone();
        let expected = DeclaredRange::Plain(Bounds::from_cargo_text("*").expect("*"));
        assert_eq!(edge.range, expected);
    }

    #[test]
    fn cargo_publish_null_is_publishable_empty_list_is_not() {
        assert!(cargo_is_publishable(None));
        assert!(!cargo_is_publishable(Some(&Vec::new())));
        assert!(cargo_is_publishable(Some(&vec![String::from("crates-io")])));
    }

    #[test]
    fn publish_gate_maps_null_empty_and_allow_list() {
        let workspace = discover("publish-gate");
        let open = workspace
            .get(&PackageId::new(Ecosystem::Cargo, "open"))
            .expect("open");
        let closed = workspace
            .get(&PackageId::new(Ecosystem::Cargo, "closed"))
            .expect("closed");
        let restricted = workspace
            .get(&PackageId::new(Ecosystem::Cargo, "restricted"))
            .expect("restricted");
        assert!(open.publishable());
        assert!(!closed.publishable());
        assert!(restricted.publishable());
    }

    #[test]
    fn workspace_maps_runtime_and_dev_edges() {
        let workspace = discover("workspace");
        assert_eq!(workspace.packages().count(), 3);
        let cli = workspace
            .get(&PackageId::new(Ecosystem::Cargo, "cli"))
            .expect("cli");
        assert_eq!(
            cli.resolves_dependencies_at(),
            ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget)
        );
        let normal = cli
            .dependencies()
            .iter()
            .find(|d| d.on.name == "core" && d.kind == DependencyKind::Normal)
            .expect("normal");
        assert!(matches!(normal.range, DeclaredRange::Plain(_)));
        let tool = workspace
            .get(&PackageId::new(Ecosystem::Cargo, "tool"))
            .expect("tool");
        let dev = tool
            .dependencies()
            .iter()
            .find(|d| d.kind == DependencyKind::Development)
            .expect("dev");
        assert_eq!(dev.on.name, "core");
    }

    #[test]
    fn exit_101_relays_cargo_stderr() {
        let root = fixture_dir("stray-nested");
        let err = discover_cargo(root.join("stray"), &root).expect_err("must fail");
        match err {
            DiscoverError::CargoMetadata {
                status: Some(101),
                message,
            } => {
                assert!(
                    message.contains("workspace") || message.contains("members"),
                    "expected cargo's stray-manifest wording, got: {message}"
                );
            }
            other => panic!("expected CargoMetadata exit 101, got {other}"),
        }
    }

    #[test]
    fn workspace_root_outside_repository_is_rejected() {
        let json = r#"{"packages":[],"workspace_members":[],"workspace_root":"/tmp"}"#;
        let err =
            workspace_from_cargo_metadata(json, fixture_dir("lone-lib")).expect_err("outside root");
        assert!(matches!(
            err,
            DiscoverError::WorkspaceRootOutsideRepository { .. }
        ));
    }
}
