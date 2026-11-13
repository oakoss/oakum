//! Cargo adapter: `cargo metadata --format-version 1 --no-deps`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use semver::{Version, VersionReq};
use serde::Deserialize;

use toml_edit::{DocumentMut, Item};

use super::paths::{canonicalize, ensure_contained, repo_relative};
use super::DiscoverError;
use crate::manifest::{
    dependency_entry, item_workspace_true, member_version, workspace_entry,
    workspace_entry_declares_version, EntryMiss, MemberVersion, WorkspaceMiss,
};
use crate::plan::{
    Bounds, BuildResolution, DeclaredRange, Dependency, DependencyKind, Ecosystem,
    ExternalDependency, Package, PackageId, PathCrate, ResolvesDependenciesAt, Workspace,
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
    let contained = ContainedMetadata::parse(json, repository_root.as_ref())?;
    let (repository_root, workspace_root) = (&contained.repository_root, &contained.workspace_root);
    let members = contained.members();

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
        packages.push(map_package(
            package,
            &by_dir,
            workspace_root,
            repository_root,
        )?);
    }
    let root = repo_relative(workspace_root, repository_root)?;
    let workspace = Workspace::new(packages)?.with_cargo_workspace_root(root);
    Ok(match linked_root_files(workspace_root, repository_root) {
        Some((manifest, lock)) => workspace.with_cargo_root_files(manifest, lock),
        None => workspace,
    })
}

/// Where the root `Cargo.toml` and `Cargo.lock` lead when either is a link
/// cargo reads and writes through, repository-relative. `None` when neither is
/// a link, or one leads out of the repository, which no diff names.
fn linked_root_files(workspace_root: &Path, repository_root: &Path) -> Option<(String, String)> {
    let resolve = |name: &str| -> Option<(bool, String)> {
        let path = workspace_root.join(name);
        let linked = fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_symlink());
        let target = if linked {
            canonicalize(&path).ok()?
        } else {
            path
        };
        Some((linked, repo_relative(&target, repository_root).ok()?))
    };
    let (manifest_linked, manifest) = resolve("Cargo.toml")?;
    let (lock_linked, lock) = resolve("Cargo.lock")?;
    (manifest_linked || lock_linked).then_some((manifest, lock))
}

/// One Cargo workspace member as `cargo metadata` reports it, without the
/// dependency graph [`discover_cargo`] builds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CargoMember {
    pub name: String,
    pub version: Version,
    /// Repository-relative directory. Empty string is the repository root.
    pub manifest_dir: String,
}

/// The members of the Cargo workspace visible from `manifest_dir`, an empty
/// list included. Lighter than [`discover_cargo`]: nothing beyond the metadata
/// is read, so a member graph [`Workspace::new`] would refuse still answers.
///
/// # Errors
///
/// As [`discover_cargo`], short of the graph checks.
pub fn cargo_members(
    manifest_dir: impl AsRef<Path>,
    repository_root: impl AsRef<Path>,
) -> Result<Vec<CargoMember>, DiscoverError> {
    let json = run_cargo_metadata(manifest_dir.as_ref())?;
    members_from_cargo_metadata(&json, repository_root.as_ref())
}

fn members_from_cargo_metadata(
    json: &str,
    repository_root: &Path,
) -> Result<Vec<CargoMember>, DiscoverError> {
    let contained = ContainedMetadata::parse(json, repository_root)?;
    contained
        .members()
        .into_iter()
        .map(|package| {
            Ok(CargoMember {
                name: package.name.clone(),
                version: parse_version(package)?,
                manifest_dir: repo_relative(&package_dir(package)?, &contained.repository_root)?,
            })
        })
        .collect()
}

/// `cargo metadata` output whose workspace root sits inside the repository,
/// the one gate every reader of it passes through.
struct ContainedMetadata {
    meta: Metadata,
    repository_root: PathBuf,
    workspace_root: PathBuf,
}

impl ContainedMetadata {
    fn parse(json: &str, repository_root: &Path) -> Result<Self, DiscoverError> {
        let meta: Metadata = serde_json::from_str(json)?;
        let repository_root = canonicalize(repository_root)?;
        let workspace_root = canonicalize(Path::new(&meta.workspace_root))?;
        ensure_contained(&workspace_root, &repository_root)?;
        Ok(Self {
            meta,
            repository_root,
            workspace_root,
        })
    }

    /// The workspace members; `packages` also lists non-member path packages.
    fn members(&self) -> Vec<&MetaPackage> {
        let member_ids: BTreeSet<&str> = self
            .meta
            .workspace_members
            .iter()
            .map(String::as_str)
            .collect();
        self.meta
            .packages
            .iter()
            .filter(|package| member_ids.contains(package.id.as_str()))
            .collect()
    }
}

fn parse_version(package: &MetaPackage) -> Result<Version, DiscoverError> {
    Version::parse(&package.version).map_err(|err| DiscoverError::Version {
        package: package.name.clone(),
        message: err.to_string(),
    })
}

fn run_cargo_metadata(manifest_dir: &Path) -> Result<String, DiscoverError> {
    const TOOL: &str = "cargo metadata";
    let limit = super::child::deadline().map_err(DiscoverError::BadDeadline)?;
    let output = super::child::output_within(
        Command::new("cargo")
            .args(["metadata", "--format-version", "1", "--no-deps"])
            .current_dir(manifest_dir),
        limit,
    )
    .map_err(|failure| {
        DiscoverError::from_child(TOOL, failure, |source| DiscoverError::CargoNotRunnable {
            source,
        })
    })?;

    if !output.status.success() {
        return Err(DiscoverError::CargoMetadata {
            status: output.status.code(),
            message: super::child::complaint(&output),
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
    repository_root: &Path,
) -> Result<Package, DiscoverError> {
    let version = parse_version(package)?;

    let resolves = if package
        .targets
        .iter()
        .any(|t| t.kind.iter().any(|k| k == "bin"))
    {
        ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget)
    } else {
        ResolvesDependenciesAt::Install
    };

    let mut manifest = read_manifest(Path::new(&package.manifest_path))?;
    let mut dependencies = Vec::new();
    let mut external_dependencies = Vec::new();
    let mut inherited_path_dependencies = BTreeSet::new();
    let mut path_crates = Vec::new();
    for dep in &package.dependencies {
        let declared_as = declared_as(dep);
        // A kind cargo adds later must not fail a discovery that succeeds
        // today; it counts as Normal.
        let kind = dependency_kind(package, dep).unwrap_or(DependencyKind::Normal);
        // An entry the manifest walk cannot find counts as inherited, so a
        // root change reaches it rather than passing it over.
        let inherited = dependency_entry(&mut manifest, kind, dep.target.as_deref(), &declared_as)
            .map_or(true, |entry| item_workspace_true(entry));
        // Registry (and other pathless) deps are never intra-workspace edges,
        // even when a workspace member reuses the same package name.
        let Some(dep_path) = dep.path.as_ref() else {
            external_dependencies.push(ExternalDependency {
                name: dep.name.clone(),
                declared_as,
                kind,
                source: dep.source.clone(),
                // Unparsable counts as any version, so no lock row escapes it.
                req: VersionReq::parse(&dep.req).unwrap_or(VersionReq::STAR),
                inherited,
            });
            continue;
        };
        if inherited && kind.is_runtime() {
            inherited_path_dependencies.insert(declared_as.clone());
        }

        let dep_dir = canonicalize(Path::new(dep_path))?;
        let Some(member) = members_by_dir.get(&dep_dir) else {
            // One outside the repository has no path a diff can name.
            if let Ok(dir) = repo_relative(&dep_dir, repository_root) {
                path_crates.push(PathCrate {
                    name: dep.name.clone(),
                    dir,
                    kind,
                });
            }
            // A path crate outside the workspace is a sourceless `Cargo.lock`
            // row a change below it passes through.
            external_dependencies.push(ExternalDependency {
                name: dep.name.clone(),
                declared_as,
                kind,
                source: None,
                req: VersionReq::parse(&dep.req).unwrap_or(VersionReq::STAR),
                inherited,
            });
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

    let dir = package_dir(package)?;
    let manifest_dir = repo_relative(&dir, repository_root)?;
    Ok(Package::new(
        PackageId::new(Ecosystem::Cargo, package.name.clone()),
        version,
        resolves,
        cargo_is_publishable(package.publish.as_ref()),
        dependencies,
    )
    .with_external_dependencies(external_dependencies)
    .with_inherited_fields(inherited_fields(&manifest))
    .with_inherited_path_dependencies(inherited_path_dependencies)
    .with_path_crates(path_crates)
    .with_manifest_dir(manifest_dir))
}

fn inherited_fields(manifest: &DocumentMut) -> BTreeSet<String> {
    manifest
        .get("package")
        .and_then(Item::as_table_like)
        .map(|table| {
            table
                .iter()
                .filter(|(_, item)| item_workspace_true(item))
                .map(|(key, _)| key.to_owned())
                .collect()
        })
        .unwrap_or_default()
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
    let kind = dependency_kind(dependent, dep)?;
    let declared_as = declared_as(dep);
    let range = declared_range(dependent, dep, &declared_as, kind, workspace_root)?;

    Ok(Dependency {
        on: PackageId::new(Ecosystem::Cargo, dep.name.clone()),
        kind,
        declared_as,
        target: dep.target.clone(),
        range,
    })
}

fn dependency_kind(
    dependent: &MetaPackage,
    dep: &MetaDependency,
) -> Result<DependencyKind, DiscoverError> {
    match dep.kind.as_deref() {
        None => Ok(DependencyKind::Normal),
        Some("dev") => Ok(DependencyKind::Development),
        Some("build") => Ok(DependencyKind::Build),
        Some(other) => Err(DiscoverError::UnknownDependencyKind {
            package: dependent.name.clone(),
            dependency: dep.name.clone(),
            kind: other.to_owned(),
        }),
    }
}

fn declared_as(dep: &MetaDependency) -> String {
    dep.rename.clone().unwrap_or_else(|| dep.name.clone())
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
    let mut doc = read_manifest(manifest_path)?;
    let section = kind.section(Ecosystem::Cargo).unwrap_or_default();
    let predicate = target.unwrap_or_default();
    let toml_error = |message: String| DiscoverError::Toml {
        path: manifest_path.to_path_buf(),
        message,
    };
    let entry = dependency_entry(&mut doc, kind, target, declared_as).map_err(|miss| match miss {
        EntryMiss::KindHasNoSection => DiscoverError::UnknownDependencyKind {
            package: package_name.to_owned(),
            dependency: dependency_name.to_owned(),
            kind: format!("{kind}"),
        },
        EntryMiss::TargetTable => toml_error(format!(
            "{package_name}: metadata listed target {predicate:?} for {declared_as}, but manifest has no [target]"
        )),
        EntryMiss::TargetPredicate => toml_error(format!(
            "{package_name}: metadata listed target {predicate:?} for {declared_as}, missing from manifest"
        )),
        EntryMiss::SectionTable => toml_error(format!(
            "{package_name}: metadata listed {declared_as} under {section}, missing from manifest"
        )),
        EntryMiss::Key => toml_error(format!(
            "{package_name}: metadata listed dependency key {declared_as}, missing from manifest"
        )),
    })?;
    match member_version(entry) {
        MemberVersion::Declared => Ok(true),
        MemberVersion::Absent => Ok(false),
        MemberVersion::Inherited => workspace_dependency_declares_version(
            workspace_root,
            declared_as,
            package_name,
            dependency_name,
        ),
        MemberVersion::Unexpected => Err(toml_error(format!(
            "{package_name} dependency {declared_as}: unexpected TOML value {entry:?}"
        ))),
    }
}

fn read_manifest(path: &Path) -> Result<DocumentMut, DiscoverError> {
    let text = fs::read_to_string(path).map_err(|source| DiscoverError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    text.parse()
        .map_err(|err: toml_edit::TomlError| DiscoverError::Toml {
            path: path.to_path_buf(),
            message: err.to_string(),
        })
}

fn workspace_dependency_declares_version(
    workspace_root: &Path,
    declared_as: &str,
    package_name: &str,
    dependency_name: &str,
) -> Result<bool, DiscoverError> {
    let root_manifest = workspace_root.join("Cargo.toml");
    let mut doc = read_manifest(&root_manifest)?;
    let toml_error = |message: String| DiscoverError::Toml {
        path: root_manifest.clone(),
        message,
    };
    let entry = workspace_entry(&mut doc, declared_as).map_err(|miss| match miss {
        WorkspaceMiss::WorkspaceTable => toml_error(format!(
            "{package_name} inherits {dependency_name} via workspace = true, but root has no [workspace]"
        )),
        WorkspaceMiss::DependenciesTable => toml_error(format!(
            "{package_name} inherits {dependency_name} via workspace = true, but root has no [workspace.dependencies]"
        )),
        WorkspaceMiss::Key => toml_error(format!(
            "{package_name} inherits {declared_as}, but [workspace.dependencies] has no such key"
        )),
    })?;
    workspace_entry_declares_version(entry).ok_or_else(|| {
        toml_error(format!(
            "[workspace.dependencies].{declared_as}: unexpected {entry:?}"
        ))
    })
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
    source: Option<String>,
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
    fn cargo_members_lists_each_member_with_its_directory() {
        let root = fixture_dir("workspace-inherit-star");
        let mut members = cargo_members(&root, &root).expect("members");
        members.sort_by(|a, b| a.name.cmp(&b.name));
        let listed: Vec<(&str, &str)> = members
            .iter()
            .map(|member| (member.name.as_str(), member.manifest_dir.as_str()))
            .collect();
        assert_eq!(listed, [("app", "app"), ("core", "core")]);
        assert_eq!(members[0].version, Version::new(0, 1, 0));
    }

    /// `discover_cargo` refuses an empty member set; a caller asking only
    /// which members exist gets none.
    #[test]
    fn cargo_members_of_an_empty_workspace_is_empty() {
        let root = crate::test_fixture::Fixture::new("cargo-members", "empty");
        fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = []\n").expect("manifest");
        assert_eq!(cargo_members(&root, &root).expect("no members"), []);
        assert!(discover_cargo(&root, &root).is_err());
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
        assert_eq!(pkg.manifest_dir(), "");
        assert_eq!(workspace.cargo_workspace_root(), Some(""));
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
    fn nested_members_have_repo_relative_dirs() {
        let workspace = discover("path-linked");
        assert_eq!(
            workspace
                .get(&PackageId::new(Ecosystem::Cargo, "app"))
                .expect("app")
                .manifest_dir(),
            "app"
        );
        assert_eq!(
            workspace
                .get(&PackageId::new(Ecosystem::Cargo, "core"))
                .expect("core")
                .manifest_dir(),
            "core"
        );
        assert_eq!(workspace.cargo_workspace_root(), Some(""));
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

    const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";

    fn external(
        name: &str,
        declared_as: &str,
        kind: DependencyKind,
        req: &str,
    ) -> ExternalDependency {
        ExternalDependency {
            name: name.to_owned(),
            declared_as: declared_as.to_owned(),
            kind,
            source: Some(CRATES_IO.to_owned()),
            req: VersionReq::parse(req).expect("req"),
            inherited: false,
        }
    }

    fn inherited(dep: ExternalDependency) -> ExternalDependency {
        ExternalDependency {
            inherited: true,
            ..dep
        }
    }

    #[test]
    fn registry_and_git_dependencies_are_kept_beside_the_graph() {
        let workspace = discover("external-deps");
        let externals = |name: &str| {
            let mut list = workspace
                .get(&PackageId::new(Ecosystem::Cargo, name))
                .expect(name)
                .external_dependencies()
                .to_vec();
            list.sort_by(|a, b| a.name.cmp(&b.name));
            list
        };
        assert_eq!(
            externals("cli"),
            [
                external("anyhow", "anyhow", DependencyKind::Normal, "^1"),
                external("cc", "cc", DependencyKind::Build, "^1"),
                ExternalDependency {
                    source: Some(String::from("git+https://example.invalid/foo.git")),
                    ..external("foo", "gitdep", DependencyKind::Normal, "*")
                },
                ExternalDependency {
                    source: None,
                    ..external("helper", "helper", DependencyKind::Normal, "*")
                },
                inherited(external(
                    "serde",
                    "serde",
                    DependencyKind::Development,
                    "^1"
                )),
                external("serde_json", "json", DependencyKind::Normal, "^1"),
                external("winapi", "winapi", DependencyKind::Normal, "^0.3"),
            ]
        );
        assert_eq!(externals("core"), []);
        assert_eq!(
            externals("store"),
            [inherited(external(
                "rusqlite",
                "rusqlite",
                DependencyKind::Normal,
                "^0.31"
            ))]
        );
        let fields = |name: &str| {
            workspace
                .get(&PackageId::new(Ecosystem::Cargo, name))
                .expect(name)
                .inherited_fields()
                .clone()
        };
        assert!(fields("cli").is_empty());
        assert_eq!(fields("store"), BTreeSet::from([String::from("edition")]));
        let paths = |name: &str| {
            workspace
                .get(&PackageId::new(Ecosystem::Cargo, name))
                .expect(name)
                .inherited_path_dependencies()
                .clone()
        };
        assert!(paths("cli").is_empty(), "cli writes its `core` path out");
        assert_eq!(paths("core"), BTreeSet::from([String::from("store")]));
    }

    #[test]
    fn an_external_dependency_of_an_unknown_kind_counts_as_normal() {
        let root = fixture_dir("external-deps");
        let json = run_cargo_metadata(&root)
            .expect("metadata")
            .replace(r#""kind":"build""#, r#""kind":"future""#);
        assert!(
            json.contains(r#""kind":"future""#),
            "fixture lost its build dependency"
        );
        let workspace = workspace_from_cargo_metadata(&json, &root).expect("discover");
        let cc = workspace
            .get(&PackageId::new(Ecosystem::Cargo, "cli"))
            .expect("cli")
            .external_dependencies()
            .iter()
            .find(|dep| dep.name == "cc")
            .expect("cc");
        assert_eq!(cc.kind, DependencyKind::Normal);
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
        // `/tmp` is not an existing path on Windows (canonicalize fails with Io).
        let outside = std::env::temp_dir();
        let json = serde_json::json!({
            "packages": [],
            "workspace_members": [],
            "workspace_root": outside,
        });
        let err = workspace_from_cargo_metadata(&json.to_string(), fixture_dir("lone-lib"))
            .expect_err("outside root");
        assert!(
            matches!(err, DiscoverError::WorkspaceRootOutsideRepository { .. }),
            "{err}"
        );
    }

    #[test]
    fn workspace_root_missing_is_io() {
        let json = serde_json::json!({
            "packages": [],
            "workspace_members": [],
            "workspace_root": "/this-path-does-not-exist-oakum-probe",
        });
        let err = workspace_from_cargo_metadata(&json.to_string(), fixture_dir("lone-lib"))
            .expect_err("missing root");
        assert!(matches!(err, DiscoverError::Io { .. }), "{err}");
    }

    #[cfg(windows)]
    #[test]
    fn workspace_root_unix_tmp_is_io_on_windows() {
        let json = r#"{"packages":[],"workspace_members":[],"workspace_root":"/tmp"}"#;
        let err =
            workspace_from_cargo_metadata(json, fixture_dir("lone-lib")).expect_err("unix tmp");
        assert!(matches!(err, DiscoverError::Io { .. }), "{err}");
    }
}
