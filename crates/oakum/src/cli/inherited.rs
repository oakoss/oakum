//! Collect inherited pins before opening workspace or catalog files.
//!
//! Member `workspace = true` / `catalog:` lines are not rewritten.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use cap_std::fs::Dir;
use oakum::manifest::{
    collect_inherited_pins, rewrite_collected_pins, CatalogRewrite, CatalogText, InheritedSources,
};
use oakum::plan::{DeclaredRange, Ecosystem, Package, PackageId, Workspace};
use semver::Version;

use super::fs::repo_path_display;
#[cfg(test)]
use super::write_set::commit_writes;
use super::write_set::{read_text, PlannedWrite};

const CARGO_TOML: &str = "Cargo.toml";
const PACKAGE_JSON: &str = "package.json";

enum CatalogSource {
    Yaml { path: PathBuf, text: String },
    Json { path: PathBuf, text: String },
}

/// # Errors
///
/// Returns when a required file cannot be read, a rewrite fails, or a writer fails.
#[cfg(test)]
pub(super) fn apply_inherited_pins(
    dir: &Dir,
    workspace: &Workspace,
    new_versions: &BTreeMap<PackageId, Version>,
) -> Result<(), Box<dyn std::error::Error>> {
    commit_writes(dir, &plan_inherited_writes(dir, workspace, new_versions)?)
}

/// # Errors
///
/// Returns when a required file cannot be read or a rewrite fails.
pub(super) fn plan_inherited_writes(
    dir: &Dir,
    workspace: &Workspace,
    new_versions: &BTreeMap<PackageId, Version>,
) -> Result<Vec<PlannedWrite>, Box<dyn std::error::Error>> {
    if new_versions.is_empty() {
        return Ok(Vec::new());
    }

    let mut member_texts = BTreeMap::new();
    if needs_cargo_member_texts(workspace, new_versions) {
        for id in new_versions.keys() {
            for (dependent, _) in workspace.dependents(id) {
                if dependent.id().ecosystem != Ecosystem::Cargo {
                    continue;
                }
                if member_texts.contains_key(dependent.id()) {
                    continue;
                }
                let path = cargo_toml_path(dependent);
                let text = read_text(dir, &path)?.ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("{} is missing", repo_path_display(&path)),
                    )
                })?;
                member_texts.insert(dependent.id().clone(), text);
            }
        }
    }
    let members: BTreeMap<_, _> = member_texts
        .iter()
        .map(|(id, text)| (id.clone(), text.as_str()))
        .collect();

    let pins = collect_inherited_pins(workspace, new_versions, &members)
        .map_err(|err| rewrite_err(err, workspace, None, None))?;
    let workspace_file = if pins.needs_workspace() {
        open_cargo_workspace_toml(dir, workspace)?
    } else {
        None
    };
    let catalog = if pins.needs_catalog() {
        open_catalog_file(dir, workspace)?
    } else {
        None
    };
    let rewritten = rewrite_collected_pins(
        &pins,
        InheritedSources {
            workspace_toml: workspace_file.as_ref().map(|(_, text)| text.as_str()),
            catalog: match &catalog {
                Some(CatalogSource::Yaml { text, .. }) => Some(CatalogText::Yaml(text)),
                Some(CatalogSource::Json { text, .. }) => Some(CatalogText::Json(text)),
                None => None,
            },
        },
    )
    .map_err(|err| rewrite_err(err, workspace, workspace_file.as_ref(), catalog.as_ref()))?;

    let mut writes = Vec::new();
    if let Some(write) =
        planned_workspace_write(workspace_file.as_ref(), rewritten.workspace_toml())?
    {
        writes.push(write);
    }
    if let Some(write) = planned_catalog_write(catalog.as_ref(), rewritten.catalog())? {
        writes.push(write);
    }
    Ok(writes)
}

fn planned_workspace_write(
    opened: Option<&(PathBuf, String)>,
    rewritten: Option<&str>,
) -> Result<Option<PlannedWrite>, Box<dyn std::error::Error>> {
    match (opened, rewritten) {
        (Some((path, original)), Some(next)) => Ok(Some(PlannedWrite::new(
            path.clone(),
            original.clone(),
            next,
        ))),
        (_, None) => Ok(None),
        (None, Some(_)) => Err("workspace source is none but rewrite is some".into()),
    }
}

fn planned_catalog_write(
    catalog: Option<&CatalogSource>,
    rewritten: Option<&CatalogRewrite>,
) -> Result<Option<PlannedWrite>, Box<dyn std::error::Error>> {
    match (catalog, rewritten) {
        (Some(CatalogSource::Yaml { path, text }), Some(CatalogRewrite::Yaml(next)))
        | (Some(CatalogSource::Json { path, text }), Some(CatalogRewrite::Json(next))) => {
            Ok(Some(PlannedWrite::new(path.clone(), text.clone(), next)))
        }
        (Some(_) | None, None) => Ok(None),
        (opened, rewritten) => Err(format!(
            "catalog source is {} but rewrite is {}",
            catalog_kind(opened),
            rewrite_kind(rewritten),
        )
        .into()),
    }
}

fn catalog_kind(src: Option<&CatalogSource>) -> &'static str {
    match src {
        Some(CatalogSource::Yaml { .. }) => "yaml",
        Some(CatalogSource::Json { .. }) => "json",
        None => "none",
    }
}

fn rewrite_kind(src: Option<&CatalogRewrite>) -> &'static str {
    match src {
        Some(CatalogRewrite::Yaml(_)) => "yaml",
        Some(CatalogRewrite::Json(_)) => "json",
        None => "none",
    }
}

fn rewrite_err(
    err: oakum::manifest::InheritedError,
    workspace: &Workspace,
    workspace_file: Option<&(PathBuf, String)>,
    catalog: Option<&CatalogSource>,
) -> Box<dyn std::error::Error> {
    use oakum::manifest::InheritedError;
    let member_path = match &err {
        InheritedError::Member { package, .. } | InheritedError::MissingMember(package) => {
            workspace.get(package).map(cargo_toml_path)
        }
        _ => None,
    };
    let path = match &err {
        InheritedError::Rewrite(_) | InheritedError::MissingWorkspaceToml => {
            workspace_file.map(|(path, _)| path.as_path())
        }
        InheritedError::CatalogYaml(_)
        | InheritedError::CatalogJson(_)
        | InheritedError::MissingCatalogFile => catalog.map(CatalogSource::path),
        InheritedError::Member { .. } | InheritedError::MissingMember(_) => member_path.as_deref(),
        InheritedError::ConflictingPin { .. } | InheritedError::NotRetargetable { .. } => None,
    };
    match path {
        Some(path) => format!("{}: {err}", repo_path_display(path)).into(),
        None => err.into(),
    }
}

impl CatalogSource {
    fn path(&self) -> &Path {
        match self {
            Self::Yaml { path, .. } | Self::Json { path, .. } => path,
        }
    }
}

fn needs_cargo_member_texts(
    workspace: &Workspace,
    new_versions: &BTreeMap<PackageId, Version>,
) -> bool {
    new_versions.keys().any(|id| {
        workspace.dependents(id).any(|(dependent, dep)| {
            dependent.id().ecosystem == Ecosystem::Cargo
                && matches!(dep.range, DeclaredRange::Plain(_))
        })
    })
}

pub(super) fn cargo_toml_path(package: &Package) -> PathBuf {
    let dir = package.manifest_dir();
    if dir.is_empty() {
        PathBuf::from(CARGO_TOML)
    } else {
        Path::new(dir).join(CARGO_TOML)
    }
}

fn open_cargo_workspace_toml(
    dir: &Dir,
    workspace: &Workspace,
) -> Result<Option<(PathBuf, String)>, Box<dyn std::error::Error>> {
    let Some(root) = workspace.cargo_workspace_root() else {
        return Ok(None);
    };
    let path = if root.is_empty() {
        PathBuf::from(CARGO_TOML)
    } else {
        Path::new(root).join(CARGO_TOML)
    };
    let text = read_text(dir, &path)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} is missing", repo_path_display(&path)),
        )
    })?;
    Ok(Some((path, text)))
}

fn open_catalog_file(
    dir: &Dir,
    workspace: &Workspace,
) -> Result<Option<CatalogSource>, Box<dyn std::error::Error>> {
    let Some(rel) = workspace.catalog_file() else {
        return Ok(None);
    };
    let path = PathBuf::from(rel);
    let text = read_text(dir, &path)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} is missing", repo_path_display(&path)),
        )
    })?;
    Ok(Some(catalog_source_from_path(path, text)))
}

fn catalog_source_from_path(path: PathBuf, text: String) -> CatalogSource {
    if path.file_name().is_some_and(|name| name == PACKAGE_JSON) {
        CatalogSource::Json { path, text }
    } else {
        CatalogSource::Yaml { path, text }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    use cap_std::fs::Dir;
    use oakum::plan::{
        Bounds, DeclaredRange, Dependency, DependencyKind, Ecosystem, Package, PackageId,
        ResolvesDependenciesAt, Workspace,
    };
    use semver::Version;

    use oakum::manifest::CatalogRewrite;

    use crate::test_fixture::Fixture;

    use super::super::fs::repo_path_display;
    use super::{
        apply_inherited_pins, catalog_kind, planned_catalog_write, planned_workspace_write,
        rewrite_kind, CatalogSource,
    };

    fn scratch(label: &str) -> Fixture {
        Fixture::new("inherited", label)
    }

    fn v(text: &str) -> Version {
        Version::parse(text).expect("version")
    }

    fn cargo(name: &str) -> PackageId {
        PackageId::new(Ecosystem::Cargo, name)
    }

    fn npm(name: &str) -> PackageId {
        PackageId::new(Ecosystem::Npm, name)
    }

    fn pkg(id: PackageId, dir: &str, deps: Vec<Dependency>) -> Package {
        Package::new(
            id,
            Version::new(0, 1, 0),
            ResolvesDependenciesAt::Install,
            true,
            deps,
        )
        .with_manifest_dir(dir)
    }

    fn workspace_with(
        packages: impl IntoIterator<Item = Package>,
        cargo_root: Option<&str>,
        catalog: Option<&str>,
    ) -> Workspace {
        let mut workspace = Workspace::new(packages).expect("workspace");
        if let Some(dir) = cargo_root {
            workspace = workspace.with_cargo_workspace_root(dir);
        }
        if let Some(path) = catalog {
            workspace = workspace.with_catalog_file(path);
        }
        workspace
    }

    #[test]
    fn cargo_inherit_writes_workspace_toml_and_leaves_member() {
        let root = scratch("cargo");
        fs::create_dir_all(root.join("crates/app")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = \"^0.1.0\"   # keep\n",
        )
        .unwrap();
        fs::write(
            root.join("crates/app/Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\ncore = { workspace = true }\n",
        )
        .unwrap();

        let workspace = workspace_with(
            [
                pkg(cargo("core"), "crates/core", vec![]),
                pkg(
                    cargo("app"),
                    "crates/app",
                    vec![Dependency {
                        on: cargo("core"),
                        kind: DependencyKind::Normal,
                        declared_as: "core".into(),
                        target: None,
                        range: DeclaredRange::Plain(
                            Bounds::from_cargo_text("^0.1.0").expect("range"),
                        ),
                    }],
                ),
            ],
            Some(""),
            None,
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(cargo("core"), v("0.2.0"))]),
        )
        .expect("apply");

        let root_toml = fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert_eq!(
            root_toml,
            "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = \"^0.2.0\"   # keep\n"
        );
        let member_before = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\ncore = { workspace = true }\n";
        assert_eq!(
            fs::read_to_string(root.join("crates/app/Cargo.toml")).unwrap(),
            member_before
        );
    }

    #[test]
    fn cargo_workspace_version_inherit_names_the_path() {
        let root = scratch("cargo-inherit-version");
        fs::create_dir_all(root.join("crates/app")).unwrap();
        let workspace_toml = "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = { version.workspace = true }\n";
        fs::write(root.join("Cargo.toml"), workspace_toml).unwrap();
        fs::write(
            root.join("crates/app/Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\ncore = { workspace = true }\n",
        )
        .unwrap();

        let workspace = workspace_with(
            [
                pkg(cargo("core"), "crates/core", vec![]),
                pkg(cargo("app"), "crates/app", vec![cargo_core_dep()]),
            ],
            Some(""),
            None,
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(cargo("core"), v("0.2.0"))]),
        )
        .expect_err("inherit");
        assert!(err.to_string().contains("Cargo.toml"), "{err}");
        assert!(
            err.to_string()
                .contains("workspace/dependencies/core/version"),
            "{err}"
        );
        assert!(err.to_string().contains("inherits"), "{err}");
        assert!(!err.to_string().contains("is missing"), "{err}");
        assert_eq!(
            fs::read_to_string(root.join("Cargo.toml")).unwrap(),
            workspace_toml
        );
    }

    #[test]
    fn cargo_both_writes_workspace_toml_and_leaves_member_version() {
        let root = scratch("cargo-both");
        fs::create_dir_all(root.join("crates/app")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = \"^0.1.0\"\n",
        )
        .unwrap();
        let member = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\ncore = { workspace = true, version = \"^0.1.0\" }\n";
        fs::write(root.join("crates/app/Cargo.toml"), member).unwrap();

        let workspace = workspace_with(
            [
                pkg(cargo("core"), "crates/core", vec![]),
                pkg(
                    cargo("app"),
                    "crates/app",
                    vec![Dependency {
                        on: cargo("core"),
                        kind: DependencyKind::Normal,
                        declared_as: "core".into(),
                        target: None,
                        range: DeclaredRange::Plain(
                            Bounds::from_cargo_text("^0.1.0").expect("range"),
                        ),
                    }],
                ),
            ],
            Some(""),
            None,
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(cargo("core"), v("0.2.0"))]),
        )
        .expect("apply");

        assert_eq!(
            fs::read_to_string(root.join("Cargo.toml")).unwrap(),
            "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = \"^0.2.0\"\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("crates/app/Cargo.toml")).unwrap(),
            member
        );
    }

    #[test]
    fn catalog_kind_mismatch_is_an_error() {
        let yaml = CatalogSource::Yaml {
            path: PathBuf::from("pnpm-workspace.yaml"),
            text: "catalog:\n  core: '^0.1.0'\n".into(),
        };
        let json = CatalogSource::Json {
            path: PathBuf::from("package.json"),
            text: "{}".into(),
        };
        let json_rewrite = CatalogRewrite::Json("{}".into());
        let yaml_rewrite = CatalogRewrite::Yaml("catalog:\n".into());
        for (opened, rewritten) in [
            (Some(&yaml), Some(&json_rewrite)),
            (Some(&json), Some(&yaml_rewrite)),
            (None, Some(&yaml_rewrite)),
        ] {
            let err = planned_catalog_write(opened, rewritten).expect_err("mismatch");
            let message = err.to_string();
            assert!(message.contains(catalog_kind(opened)), "{message}");
            assert!(message.contains(rewrite_kind(rewritten)), "{message}");
        }
    }

    #[test]
    fn workspace_rewrite_without_opened_file_is_an_error() {
        let err = planned_workspace_write(None, Some("core = \"^0.2.0\"\n")).expect_err("mismatch");
        let message = err.to_string();
        assert!(message.contains("none"), "{message}");
        assert!(message.contains("some"), "{message}");
    }

    #[test]
    fn cargo_member_pin_does_not_write_workspace_toml() {
        let root = scratch("own-pin");
        fs::create_dir_all(root.join("crates/app")).unwrap();
        let workspace_toml =
            "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = \"^0.1.0\"\n";
        fs::write(root.join("Cargo.toml"), workspace_toml).unwrap();
        fs::write(
            root.join("crates/app/Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\ncore = \"^0.1.0\"\n",
        )
        .unwrap();

        let workspace = Workspace::new([
            pkg(cargo("core"), "crates/core", vec![]),
            pkg(
                cargo("app"),
                "crates/app",
                vec![Dependency {
                    on: cargo("core"),
                    kind: DependencyKind::Normal,
                    declared_as: "core".into(),
                    target: None,
                    range: DeclaredRange::Plain(Bounds::from_cargo_text("^0.1.0").expect("range")),
                }],
            ),
        ])
        .expect("workspace");

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(cargo("core"), v("0.2.0"))]),
        )
        .expect("apply");

        assert_eq!(
            fs::read_to_string(root.join("Cargo.toml")).unwrap(),
            workspace_toml
        );
        assert_eq!(
            fs::read_to_string(root.join("crates/app/Cargo.toml")).unwrap(),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\ncore = \"^0.1.0\"\n"
        );
    }

    #[test]
    fn cargo_workspace_under_subdirectory_is_opened() {
        let root = scratch("subdir-cargo");
        fs::create_dir_all(root.join("rust/crates/app")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"decoy\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::write(
            root.join("rust/Cargo.toml"),
            "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = \"^0.1.0\"\n",
        )
        .unwrap();
        fs::write(
            root.join("rust/crates/app/Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\ncore = { workspace = true }\n",
        )
        .unwrap();

        let workspace = workspace_with(
            [
                pkg(cargo("core"), "rust/crates/core", vec![]),
                pkg(
                    cargo("app"),
                    "rust/crates/app",
                    vec![Dependency {
                        on: cargo("core"),
                        kind: DependencyKind::Normal,
                        declared_as: "core".into(),
                        target: None,
                        range: DeclaredRange::Plain(
                            Bounds::from_cargo_text("^0.1.0").expect("range"),
                        ),
                    }],
                ),
            ],
            Some("rust"),
            None,
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(cargo("core"), v("0.2.0"))]),
        )
        .expect("apply");

        assert_eq!(
            fs::read_to_string(root.join("rust/Cargo.toml")).unwrap(),
            "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = \"^0.2.0\"\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("Cargo.toml")).unwrap(),
            "[package]\nname = \"decoy\"\nversion = \"0.1.0\"\n"
        );
    }

    #[test]
    fn pnpm_catalog_writes_yaml_and_leaves_member() {
        let root = scratch("pnpm");
        fs::create_dir_all(root.join("packages/app")).unwrap();
        fs::write(
            root.join("pnpm-workspace.yaml"),
            "packages:\n  - 'packages/*'\ncatalog:\n  core: '^0.1.0'   # keep\n",
        )
        .unwrap();
        fs::write(
            root.join("packages/app/package.json"),
            "{\n  \"name\": \"app\",\n  \"dependencies\": {\n    \"core\": \"catalog:\"\n  }\n}\n",
        )
        .unwrap();

        let workspace = workspace_with(
            [
                pkg(npm("core"), "packages/core", vec![]),
                pkg(
                    npm("app"),
                    "packages/app",
                    vec![Dependency {
                        on: npm("core"),
                        kind: DependencyKind::Normal,
                        declared_as: "core".into(),
                        target: None,
                        range: DeclaredRange::Catalog {
                            name: None,
                            bounds: Bounds::from_npm_text("^0.1.0").expect("range"),
                        },
                    }],
                ),
            ],
            None,
            Some("pnpm-workspace.yaml"),
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("0.2.0"))]),
        )
        .expect("apply");

        let yaml = fs::read_to_string(root.join("pnpm-workspace.yaml")).unwrap();
        assert_eq!(
            yaml,
            "packages:\n  - 'packages/*'\ncatalog:\n  core: '^0.2.0'   # keep\n"
        );
        let member = fs::read_to_string(root.join("packages/app/package.json")).unwrap();
        assert_eq!(
            member,
            "{\n  \"name\": \"app\",\n  \"dependencies\": {\n    \"core\": \"catalog:\"\n  }\n}\n"
        );
    }

    #[test]
    fn pnpm_yaml_wins_when_yarnrc_also_exists() {
        let root = scratch("xor");
        fs::write(
            root.join("pnpm-workspace.yaml"),
            "catalog:\n  core: '^0.1.0'\n",
        )
        .unwrap();
        fs::write(root.join(".yarnrc.yml"), "catalog:\n  core: '^0.1.0'\n").unwrap();

        let workspace = workspace_with(
            [
                pkg(npm("core"), "", vec![]),
                pkg(
                    npm("app"),
                    "",
                    vec![Dependency {
                        on: npm("core"),
                        kind: DependencyKind::Normal,
                        declared_as: "core".into(),
                        target: None,
                        range: DeclaredRange::Catalog {
                            name: None,
                            bounds: Bounds::from_npm_text("^0.1.0").expect("range"),
                        },
                    }],
                ),
            ],
            None,
            Some("pnpm-workspace.yaml"),
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("0.2.0"))]),
        )
        .expect("apply");

        assert_eq!(
            fs::read_to_string(root.join("pnpm-workspace.yaml")).unwrap(),
            "catalog:\n  core: '^0.2.0'\n"
        );
        assert_eq!(
            fs::read_to_string(root.join(".yarnrc.yml")).unwrap(),
            "catalog:\n  core: '^0.1.0'\n"
        );
    }

    #[test]
    fn catalog_under_subdirectory_is_opened() {
        let root = scratch("subdir-js");
        fs::create_dir_all(root.join("js/packages/app")).unwrap();
        fs::write(
            root.join("js/pnpm-workspace.yaml"),
            "catalog:\n  core: '^0.1.0'\n",
        )
        .unwrap();

        let workspace = workspace_with(
            [
                pkg(npm("core"), "js/packages/core", vec![]),
                pkg(
                    npm("app"),
                    "js/packages/app",
                    vec![Dependency {
                        on: npm("core"),
                        kind: DependencyKind::Normal,
                        declared_as: "core".into(),
                        target: None,
                        range: DeclaredRange::Catalog {
                            name: None,
                            bounds: Bounds::from_npm_text("^0.1.0").expect("range"),
                        },
                    }],
                ),
            ],
            None,
            Some("js/pnpm-workspace.yaml"),
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("0.2.0"))]),
        )
        .expect("apply");

        assert_eq!(
            fs::read_to_string(root.join("js/pnpm-workspace.yaml")).unwrap(),
            "catalog:\n  core: '^0.2.0'\n"
        );
    }

    #[test]
    fn yarnrc_is_opened_when_pnpm_yaml_is_absent() {
        let root = scratch("yarn");
        fs::write(root.join(".yarnrc.yml"), "catalog:\n  core: '^0.1.0'\n").unwrap();

        let workspace = workspace_with(
            [
                pkg(npm("core"), "", vec![]),
                pkg(
                    npm("app"),
                    "",
                    vec![Dependency {
                        on: npm("core"),
                        kind: DependencyKind::Normal,
                        declared_as: "core".into(),
                        target: None,
                        range: DeclaredRange::Catalog {
                            name: None,
                            bounds: Bounds::from_npm_text("^0.1.0").expect("range"),
                        },
                    }],
                ),
            ],
            None,
            Some(".yarnrc.yml"),
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("0.2.0"))]),
        )
        .expect("apply");

        let yaml = fs::read_to_string(root.join(".yarnrc.yml")).unwrap();
        assert_eq!(yaml, "catalog:\n  core: '^0.2.0'\n");
    }

    #[test]
    fn named_catalog_is_not_passed_as_none() {
        let root = scratch("named");
        fs::write(
            root.join("pnpm-workspace.yaml"),
            "catalog:\n  other: '^9.0.0'\ncatalogs:\n  pinned:\n    core: '1.0.0'\n",
        )
        .unwrap();

        let workspace = workspace_with(
            [
                pkg(npm("core"), "", vec![]),
                pkg(
                    npm("app"),
                    "",
                    vec![Dependency {
                        on: npm("core"),
                        kind: DependencyKind::Normal,
                        declared_as: "core".into(),
                        target: None,
                        range: DeclaredRange::Catalog {
                            name: Some("pinned".into()),
                            bounds: Bounds::from_npm_text("1.0.0").expect("range"),
                        },
                    }],
                ),
            ],
            None,
            Some("pnpm-workspace.yaml"),
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("2.0.0"))]),
        )
        .expect("apply");

        let yaml = fs::read_to_string(root.join("pnpm-workspace.yaml")).unwrap();
        assert_eq!(
            yaml,
            "catalog:\n  other: '^9.0.0'\ncatalogs:\n  pinned:\n    core: '2.0.0'\n"
        );
    }

    #[test]
    fn package_json_catalog_is_opened_when_yaml_is_absent() {
        let root = scratch("json");
        fs::write(
            root.join("package.json"),
            "{\n  \"catalog\": {\n    \"core\": \"^0.1.0\"\n  }\n}\n",
        )
        .unwrap();

        let workspace = workspace_with(
            [
                pkg(npm("core"), "", vec![]),
                pkg(
                    npm("app"),
                    "",
                    vec![Dependency {
                        on: npm("core"),
                        kind: DependencyKind::Normal,
                        declared_as: "core".into(),
                        target: None,
                        range: DeclaredRange::Catalog {
                            name: None,
                            bounds: Bounds::from_npm_text("^0.1.0").expect("range"),
                        },
                    }],
                ),
            ],
            None,
            Some("package.json"),
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("0.2.0"))]),
        )
        .expect("apply");

        let json = fs::read_to_string(root.join("package.json")).unwrap();
        assert_eq!(
            json,
            "{\n  \"catalog\": {\n    \"core\": \"^0.2.0\"\n  }\n}\n"
        );
    }

    fn cargo_core_dep() -> Dependency {
        Dependency {
            on: cargo("core"),
            kind: DependencyKind::Normal,
            declared_as: "core".into(),
            target: None,
            range: DeclaredRange::Plain(Bounds::from_cargo_text("^0.1.0").expect("range")),
        }
    }

    fn npm_catalog_dep() -> Dependency {
        Dependency {
            on: npm("core"),
            kind: DependencyKind::Normal,
            declared_as: "core".into(),
            target: None,
            range: DeclaredRange::Catalog {
                name: None,
                bounds: Bounds::from_npm_text("^0.1.0").expect("range"),
            },
        }
    }

    #[test]
    fn nested_member_package_json_does_not_shadow_root_catalog() {
        let root = scratch("nested-json");
        fs::create_dir_all(root.join("packages/app")).unwrap();
        fs::write(
            root.join("package.json"),
            "{\n  \"catalog\": {\n    \"core\": \"^0.1.0\"\n  }\n}\n",
        )
        .unwrap();
        fs::write(
            root.join("packages/app/package.json"),
            "{\n  \"name\": \"app\",\n  \"dependencies\": {\n    \"core\": \"catalog:\"\n  }\n}\n",
        )
        .unwrap();

        let workspace = workspace_with(
            [
                pkg(npm("core"), "packages/core", vec![]),
                pkg(npm("app"), "packages/app", vec![npm_catalog_dep()]),
            ],
            None,
            Some("package.json"),
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("0.2.0"))]),
        )
        .expect("apply");

        assert_eq!(
            fs::read_to_string(root.join("package.json")).unwrap(),
            "{\n  \"catalog\": {\n    \"core\": \"^0.2.0\"\n  }\n}\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("packages/app/package.json")).unwrap(),
            "{\n  \"name\": \"app\",\n  \"dependencies\": {\n    \"core\": \"catalog:\"\n  }\n}\n"
        );
    }

    #[test]
    fn pnpm_yaml_wins_when_package_json_catalog_also_exists() {
        let root = scratch("yaml-json");
        fs::write(
            root.join("pnpm-workspace.yaml"),
            "catalog:\n  core: '^0.1.0'\n",
        )
        .unwrap();
        let json = "{\n  \"catalog\": {\n    \"core\": \"^0.1.0\"\n  }\n}\n";
        fs::write(root.join("package.json"), json).unwrap();

        let workspace = workspace_with(
            [
                pkg(npm("core"), "", vec![]),
                pkg(npm("app"), "", vec![npm_catalog_dep()]),
            ],
            None,
            Some("pnpm-workspace.yaml"),
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("0.2.0"))]),
        )
        .expect("apply");

        assert_eq!(
            fs::read_to_string(root.join("pnpm-workspace.yaml")).unwrap(),
            "catalog:\n  core: '^0.2.0'\n"
        );
        assert_eq!(fs::read_to_string(root.join("package.json")).unwrap(), json);
    }

    #[test]
    fn opens_the_catalog_file_on_workspace() {
        let root = scratch("two-catalog");
        fs::create_dir_all(root.join("js/packages/app")).unwrap();
        fs::create_dir_all(root.join("other/packages/cli")).unwrap();
        fs::create_dir_all(root.join("catalogs")).unwrap();
        let decoy = "catalog:\n  core: '^0.1.0'\n";
        fs::write(root.join("js/pnpm-workspace.yaml"), decoy).unwrap();
        fs::write(root.join("other/pnpm-workspace.yaml"), decoy).unwrap();
        fs::write(root.join("catalogs/pnpm-workspace.yaml"), decoy).unwrap();

        let workspace = workspace_with(
            [
                pkg(npm("core"), "js/packages/core", vec![]),
                pkg(npm("app"), "js/packages/app", vec![npm_catalog_dep()]),
                pkg(npm("cli"), "other/packages/cli", vec![npm_catalog_dep()]),
            ],
            None,
            Some("catalogs/pnpm-workspace.yaml"),
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("0.2.0"))]),
        )
        .expect("apply");
        assert_eq!(
            fs::read_to_string(root.join("catalogs/pnpm-workspace.yaml")).unwrap(),
            "catalog:\n  core: '^0.2.0'\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("js/pnpm-workspace.yaml")).unwrap(),
            decoy
        );
        assert_eq!(
            fs::read_to_string(root.join("other/pnpm-workspace.yaml")).unwrap(),
            decoy
        );
    }

    #[test]
    fn catalog_dependents_without_catalog_file_do_not_rewrite_disk() {
        let root = scratch("catalog-none");
        fs::write(
            root.join("pnpm-workspace.yaml"),
            "catalog:\n  core: '^0.1.0'\n",
        )
        .unwrap();
        let workspace = workspace_with(
            [
                pkg(npm("core"), "", vec![]),
                pkg(npm("app"), "", vec![npm_catalog_dep()]),
            ],
            None,
            None,
        );
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("0.2.0"))]),
        )
        .expect_err("missing catalog path");
        assert!(err.to_string().contains("catalog"), "{err}");
        assert_eq!(
            fs::read_to_string(root.join("pnpm-workspace.yaml")).unwrap(),
            "catalog:\n  core: '^0.1.0'\n"
        );
    }

    #[test]
    fn opens_the_cargo_workspace_root_on_workspace() {
        let root = scratch("two-cargo");
        fs::create_dir_all(root.join("rust/crates/app")).unwrap();
        fs::create_dir_all(root.join("go/crates/app")).unwrap();
        fs::create_dir_all(root.join("lib")).unwrap();
        let decoy = "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = \"^0.1.0\"\n";
        fs::write(root.join("rust/Cargo.toml"), decoy).unwrap();
        fs::write(root.join("go/Cargo.toml"), decoy).unwrap();
        fs::write(root.join("lib/Cargo.toml"), decoy).unwrap();
        fs::write(
            root.join("rust/crates/app/Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\ncore = { workspace = true }\n",
        )
        .unwrap();
        fs::write(
            root.join("go/crates/app/Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\ncore = { workspace = true }\n",
        )
        .unwrap();

        let workspace = workspace_with(
            [
                pkg(cargo("core"), "rust/crates/core", vec![]),
                pkg(cargo("app"), "rust/crates/app", vec![cargo_core_dep()]),
                pkg(cargo("cli"), "go/crates/app", vec![cargo_core_dep()]),
            ],
            Some("lib"),
            None,
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(cargo("core"), v("0.2.0"))]),
        )
        .expect("apply");
        assert_eq!(
            fs::read_to_string(root.join("lib/Cargo.toml")).unwrap(),
            "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = \"^0.2.0\"\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("rust/Cargo.toml")).unwrap(),
            decoy
        );
        assert_eq!(
            fs::read_to_string(root.join("go/Cargo.toml")).unwrap(),
            decoy
        );
    }

    #[test]
    fn missing_cargo_workspace_toml_names_the_path() {
        let root = scratch("missing-cargo-root");
        fs::create_dir_all(root.join("rust/crates/app")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\n[workspace.dependencies]\ncore = \"^0.1.0\"\n",
        )
        .unwrap();
        fs::write(
            root.join("rust/crates/app/Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\ncore = { workspace = true }\n",
        )
        .unwrap();
        let workspace = workspace_with(
            [
                pkg(cargo("core"), "crates/core", vec![]),
                pkg(cargo("app"), "rust/crates/app", vec![cargo_core_dep()]),
            ],
            Some("rust"),
            None,
        );
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(cargo("core"), v("0.2.0"))]),
        )
        .expect_err("missing rust workspace");
        assert!(
            err.to_string()
                .contains(&repo_path_display(&Path::new("rust").join("Cargo.toml"))),
            "{err}"
        );
        assert!(err.to_string().contains("is missing"), "{err}");
        assert!(fs::read_to_string(root.join("Cargo.toml"))
            .unwrap()
            .contains("^0.1.0"));
    }

    #[test]
    fn workspace_in_a_comment_is_not_a_workspace_root() {
        let root = scratch("comment-ws");
        fs::create_dir_all(root.join("rust/crates/app")).unwrap();
        fs::write(
            root.join("rust/Cargo.toml"),
            "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = \"^0.1.0\"\n",
        )
        .unwrap();
        fs::write(
            root.join("rust/crates/app/Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n# see [workspace]\n[dependencies]\ncore = { workspace = true }\n",
        )
        .unwrap();

        let workspace = workspace_with(
            [
                pkg(cargo("core"), "rust/crates/core", vec![]),
                pkg(cargo("app"), "rust/crates/app", vec![cargo_core_dep()]),
            ],
            Some("rust"),
            None,
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(cargo("core"), v("0.2.0"))]),
        )
        .expect("apply");

        assert_eq!(
            fs::read_to_string(root.join("rust/Cargo.toml")).unwrap(),
            "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = \"^0.2.0\"\n"
        );
    }

    #[test]
    fn spaced_workspace_header_is_still_a_workspace() {
        let root = scratch("spaced-ws");
        fs::create_dir_all(root.join("rust/crates/app")).unwrap();
        fs::write(
            root.join("rust/Cargo.toml"),
            "[ workspace ]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = \"^0.1.0\"\n",
        )
        .unwrap();
        fs::write(
            root.join("rust/crates/app/Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\ncore = { workspace = true }\n",
        )
        .unwrap();

        let workspace = workspace_with(
            [
                pkg(cargo("core"), "rust/crates/core", vec![]),
                pkg(cargo("app"), "rust/crates/app", vec![cargo_core_dep()]),
            ],
            Some("rust"),
            None,
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(cargo("core"), v("0.2.0"))]),
        )
        .expect("apply");

        assert!(fs::read_to_string(root.join("rust/Cargo.toml"))
            .unwrap()
            .contains("core = \"^0.2.0\""));
    }

    fn write_mixed(root: &std::path::Path) {
        fs::create_dir_all(root.join("rust/crates/app")).unwrap();
        fs::create_dir_all(root.join("js/packages/app")).unwrap();
        fs::write(
            root.join("rust/Cargo.toml"),
            "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = \"^0.1.0\"\n",
        )
        .unwrap();
        fs::write(
            root.join("rust/crates/app/Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\ncore = { workspace = true }\n",
        )
        .unwrap();
        fs::write(
            root.join("js/pnpm-workspace.yaml"),
            "catalog:\n  core: '^0.1.0'\n",
        )
        .unwrap();
        fs::write(
            root.join("js/packages/app/package.json"),
            "{\n  \"name\": \"app\",\n  \"dependencies\": {\n    \"core\": \"catalog:\"\n  }\n}\n",
        )
        .unwrap();
    }

    fn mixed_graph() -> Workspace {
        workspace_with(
            [
                pkg(cargo("core"), "rust/crates/core", vec![]),
                pkg(cargo("app"), "rust/crates/app", vec![cargo_core_dep()]),
                pkg(npm("core"), "js/packages/core", vec![]),
                pkg(npm("app"), "js/packages/app", vec![npm_catalog_dep()]),
            ],
            Some("rust"),
            Some("js/pnpm-workspace.yaml"),
        )
    }

    #[test]
    fn cargo_and_catalog_pins_both_write() {
        let root = scratch("mixed-ok");
        write_mixed(&root);
        let workspace = mixed_graph();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(cargo("core"), v("0.2.0")), (npm("core"), v("0.2.0"))]),
        )
        .expect("apply");

        assert_eq!(
            fs::read_to_string(root.join("rust/Cargo.toml")).unwrap(),
            "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = \"^0.2.0\"\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("js/pnpm-workspace.yaml")).unwrap(),
            "catalog:\n  core: '^0.2.0'\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn catalog_write_failure_restores_workspace() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("mixed-restore");
        write_mixed(&root);
        let workspace = mixed_graph();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let js = root.join("js");
        let mut perms = fs::metadata(&js).unwrap().permissions();
        let original_mode = perms.mode();
        perms.set_mode(0o555);
        fs::set_permissions(&js, perms).unwrap();

        let err = apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(cargo("core"), v("0.2.0")), (npm("core"), v("0.2.0"))]),
        );
        let mut restore = fs::metadata(&js).unwrap().permissions();
        restore.set_mode(original_mode);
        fs::set_permissions(&js, restore).unwrap();
        err.expect_err("catalog write");

        assert_eq!(
            fs::read_to_string(root.join("rust/Cargo.toml")).unwrap(),
            "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = \"^0.1.0\"\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("js/pnpm-workspace.yaml")).unwrap(),
            "catalog:\n  core: '^0.1.0'\n"
        );
    }

    #[test]
    fn yaml_without_catalog_does_not_shadow_package_json() {
        let root = scratch("yaml-no-catalog");
        fs::write(
            root.join("pnpm-workspace.yaml"),
            "packages:\n  - 'packages/*'\n",
        )
        .unwrap();
        fs::write(
            root.join("package.json"),
            "{\n  \"catalog\": {\n    \"core\": \"^0.1.0\"\n  }\n}\n",
        )
        .unwrap();

        let workspace = workspace_with(
            [
                pkg(npm("core"), "", vec![]),
                pkg(npm("app"), "", vec![npm_catalog_dep()]),
            ],
            None,
            Some("package.json"),
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("0.2.0"))]),
        )
        .expect("apply");

        assert_eq!(
            fs::read_to_string(root.join("package.json")).unwrap(),
            "{\n  \"catalog\": {\n    \"core\": \"^0.2.0\"\n  }\n}\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("pnpm-workspace.yaml")).unwrap(),
            "packages:\n  - 'packages/*'\n"
        );
    }

    #[test]
    fn catalogs_only_package_json_is_opened() {
        let root = scratch("json-catalogs");
        fs::write(
            root.join("package.json"),
            "{\n  \"catalogs\": {\n    \"pinned\": {\n      \"core\": \"1.0.0\"\n    }\n  }\n}\n",
        )
        .unwrap();

        let workspace = workspace_with(
            [
                pkg(npm("core"), "", vec![]),
                pkg(
                    npm("app"),
                    "",
                    vec![Dependency {
                        on: npm("core"),
                        kind: DependencyKind::Normal,
                        declared_as: "core".into(),
                        target: None,
                        range: DeclaredRange::Catalog {
                            name: Some("pinned".into()),
                            bounds: Bounds::from_npm_text("1.0.0").expect("range"),
                        },
                    }],
                ),
            ],
            None,
            Some("package.json"),
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("2.0.0"))]),
        )
        .expect("apply");

        assert_eq!(
            fs::read_to_string(root.join("package.json")).unwrap(),
            "{\n  \"catalogs\": {\n    \"pinned\": {\n      \"core\": \"2.0.0\"\n    }\n  }\n}\n"
        );
    }

    #[test]
    fn cargo_only_bump_ignores_two_catalog_files() {
        let root = scratch("cargo-ignores-catalogs");
        fs::create_dir_all(root.join("crates/app")).unwrap();
        fs::create_dir_all(root.join("js")).unwrap();
        fs::create_dir_all(root.join("other")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = \"^0.1.0\"\n",
        )
        .unwrap();
        fs::write(
            root.join("crates/app/Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\ncore = { workspace = true }\n",
        )
        .unwrap();
        fs::write(
            root.join("js/pnpm-workspace.yaml"),
            "catalog:\n  core: '^0.1.0'\n",
        )
        .unwrap();
        fs::write(
            root.join("other/pnpm-workspace.yaml"),
            "catalog:\n  core: '^0.1.0'\n",
        )
        .unwrap();

        let workspace = workspace_with(
            [
                pkg(cargo("core"), "crates/core", vec![]),
                pkg(cargo("app"), "crates/app", vec![cargo_core_dep()]),
                pkg(npm("js"), "js", vec![]),
                pkg(npm("other"), "other", vec![]),
            ],
            Some(""),
            None,
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(cargo("core"), v("0.2.0"))]),
        )
        .expect("apply");

        assert!(fs::read_to_string(root.join("Cargo.toml"))
            .unwrap()
            .contains("core = \"^0.2.0\""));
        assert_eq!(
            fs::read_to_string(root.join("js/pnpm-workspace.yaml")).unwrap(),
            "catalog:\n  core: '^0.1.0'\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("other/pnpm-workspace.yaml")).unwrap(),
            "catalog:\n  core: '^0.1.0'\n"
        );
    }

    #[test]
    fn npm_only_bump_ignores_two_cargo_workspaces() {
        let root = scratch("npm-ignores-cargo");
        fs::create_dir_all(root.join("rust")).unwrap();
        fs::create_dir_all(root.join("go")).unwrap();
        fs::write(root.join("rust/Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        fs::write(root.join("go/Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        fs::write(
            root.join("pnpm-workspace.yaml"),
            "catalog:\n  core: '^0.1.0'\n",
        )
        .unwrap();

        let workspace = workspace_with(
            [
                pkg(npm("core"), "", vec![]),
                pkg(npm("app"), "", vec![npm_catalog_dep()]),
                pkg(cargo("rust"), "rust", vec![]),
                pkg(cargo("go"), "go", vec![]),
            ],
            None,
            Some("pnpm-workspace.yaml"),
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("0.2.0"))]),
        )
        .expect("apply");

        assert_eq!(
            fs::read_to_string(root.join("pnpm-workspace.yaml")).unwrap(),
            "catalog:\n  core: '^0.2.0'\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("rust/Cargo.toml")).unwrap(),
            "[workspace]\nmembers = []\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("go/Cargo.toml")).unwrap(),
            "[workspace]\nmembers = []\n"
        );
    }

    #[test]
    fn unparseable_workspace_toml_is_an_error() {
        let root = scratch("bad-toml");
        fs::create_dir_all(root.join("rust/crates/app")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = []\n[workspace.dependencies]\ncore = \"^9.9.9\"\n",
        )
        .unwrap();
        fs::write(
            root.join("rust/Cargo.toml"),
            "[workspace]\nthis is not valid toml\n",
        )
        .unwrap();
        fs::write(
            root.join("rust/crates/app/Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\ncore = { workspace = true }\n",
        )
        .unwrap();

        let workspace = workspace_with(
            [
                pkg(cargo("core"), "rust/crates/core", vec![]),
                pkg(cargo("app"), "rust/crates/app", vec![cargo_core_dep()]),
            ],
            Some("rust"),
            None,
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(cargo("core"), v("0.2.0"))]),
        )
        .expect_err("bad toml");
        assert!(err.to_string().contains("TOML parse error"), "{err}");
        assert!(
            err.to_string()
                .contains(&repo_path_display(&Path::new("rust").join("Cargo.toml"))),
            "{err}"
        );
        assert!(fs::read_to_string(root.join("Cargo.toml"))
            .unwrap()
            .contains("core = \"^9.9.9\""));
    }

    #[test]
    fn unparseable_package_json_is_an_error() {
        let root = scratch("bad-json");
        fs::create_dir_all(root.join("packages/app")).unwrap();
        fs::write(
            root.join("package.json"),
            "{\n  \"catalog\": {\n    \"core\": \"^0.1.0\"\n  }\n}\n",
        )
        .unwrap();
        fs::write(
            root.join("packages/app/package.json"),
            "{ not json catalog\n",
        )
        .unwrap();

        let workspace = workspace_with(
            [
                pkg(npm("core"), "packages/core", vec![]),
                pkg(npm("app"), "packages/app", vec![npm_catalog_dep()]),
            ],
            None,
            Some("packages/app/package.json"),
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("0.2.0"))]),
        )
        .expect_err("bad json");
        assert!(
            err.to_string()
                .contains(&repo_path_display(Path::new("packages/app/package.json"))),
            "{err}"
        );
        assert!(fs::read_to_string(root.join("package.json"))
            .unwrap()
            .contains("\"^0.1.0\""));
    }

    #[test]
    fn version_workspace_true_writes_workspace_toml() {
        let root = scratch("version-ws");
        fs::create_dir_all(root.join("crates/app")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = \"^0.1.0\"\n",
        )
        .unwrap();
        fs::write(
            root.join("crates/app/Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\ncore = { version.workspace = true }\n",
        )
        .unwrap();

        let workspace = workspace_with(
            [
                pkg(cargo("core"), "crates/core", vec![]),
                pkg(cargo("app"), "crates/app", vec![cargo_core_dep()]),
            ],
            Some(""),
            None,
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(cargo("core"), v("0.2.0"))]),
        )
        .expect("apply");

        assert_eq!(
            fs::read_to_string(root.join("Cargo.toml")).unwrap(),
            "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = \"^0.2.0\"\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("crates/app/Cargo.toml")).unwrap(),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\ncore = { version.workspace = true }\n"
        );
    }

    #[test]
    fn dev_inherit_writes_workspace_toml() {
        let root = scratch("dev-ws");
        fs::create_dir_all(root.join("crates/app")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = \"^0.1.0\"\n",
        )
        .unwrap();
        fs::write(
            root.join("crates/app/Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dev-dependencies]\ncore = { workspace = true }\n",
        )
        .unwrap();

        let mut dep = cargo_core_dep();
        dep.kind = DependencyKind::Development;
        let workspace = workspace_with(
            [
                pkg(cargo("core"), "crates/core", vec![]),
                pkg(cargo("app"), "crates/app", vec![dep]),
            ],
            Some(""),
            None,
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(cargo("core"), v("0.2.0"))]),
        )
        .expect("apply");

        assert_eq!(
            fs::read_to_string(root.join("Cargo.toml")).unwrap(),
            "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = \"^0.2.0\"\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("crates/app/Cargo.toml")).unwrap(),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dev-dependencies]\ncore = { workspace = true }\n"
        );
    }

    #[test]
    fn member_pin_ignores_two_cargo_workspaces() {
        let root = scratch("member-pin-two-ws");
        fs::create_dir_all(root.join("rust/crates/app")).unwrap();
        fs::create_dir_all(root.join("go")).unwrap();
        let rust = "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = \"^0.1.0\"\n";
        let go = "[workspace]\nmembers = []\n";
        fs::write(root.join("rust/Cargo.toml"), rust).unwrap();
        fs::write(root.join("go/Cargo.toml"), go).unwrap();
        fs::write(
            root.join("rust/crates/app/Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\ncore = \"^0.1.0\"\n",
        )
        .unwrap();

        let workspace = Workspace::new([
            pkg(cargo("core"), "rust/crates/core", vec![]),
            pkg(cargo("app"), "rust/crates/app", vec![cargo_core_dep()]),
            pkg(cargo("other"), "go", vec![]),
        ])
        .expect("workspace");

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(cargo("core"), v("0.2.0"))]),
        )
        .expect("apply");

        assert_eq!(
            fs::read_to_string(root.join("rust/Cargo.toml")).unwrap(),
            rust
        );
        assert_eq!(fs::read_to_string(root.join("go/Cargo.toml")).unwrap(), go);
    }

    #[test]
    fn unparseable_yaml_is_an_error() {
        let root = scratch("bad-yaml");
        fs::write(root.join("pnpm-workspace.yaml"), "catalog: [\n").unwrap();
        fs::write(
            root.join("package.json"),
            "{\n  \"catalog\": {\n    \"core\": \"^0.1.0\"\n  }\n}\n",
        )
        .unwrap();

        let workspace = workspace_with(
            [
                pkg(npm("core"), "", vec![]),
                pkg(npm("app"), "", vec![npm_catalog_dep()]),
            ],
            None,
            Some("pnpm-workspace.yaml"),
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("0.2.0"))]),
        )
        .expect_err("bad yaml");
        assert!(err.to_string().contains("pnpm-workspace.yaml"), "{err}");
        assert!(
            err.to_string().contains("not a valid catalog schema"),
            "{err}"
        );
        assert!(fs::read_to_string(root.join("package.json"))
            .unwrap()
            .contains("\"^0.1.0\""));
    }

    #[test]
    fn catalogs_only_yaml_is_opened() {
        let root = scratch("yaml-catalogs");
        fs::write(
            root.join("pnpm-workspace.yaml"),
            "catalogs:\n  pinned:\n    core: '1.0.0'\n",
        )
        .unwrap();

        let workspace = workspace_with(
            [
                pkg(npm("core"), "", vec![]),
                pkg(
                    npm("app"),
                    "",
                    vec![Dependency {
                        on: npm("core"),
                        kind: DependencyKind::Normal,
                        declared_as: "core".into(),
                        target: None,
                        range: DeclaredRange::Catalog {
                            name: Some("pinned".into()),
                            bounds: Bounds::from_npm_text("1.0.0").expect("range"),
                        },
                    }],
                ),
            ],
            None,
            Some("pnpm-workspace.yaml"),
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("2.0.0"))]),
        )
        .expect("apply");

        assert_eq!(
            fs::read_to_string(root.join("pnpm-workspace.yaml")).unwrap(),
            "catalogs:\n  pinned:\n    core: '2.0.0'\n"
        );
    }

    #[test]
    fn inherit_ignores_unused_cargo_workspace() {
        let root = scratch("unused-cargo-ws");
        fs::create_dir_all(root.join("rust/crates/app")).unwrap();
        fs::create_dir_all(root.join("go")).unwrap();
        let rust = "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = \"^0.1.0\"\n";
        let go = "[workspace]\nmembers = []\n";
        fs::write(root.join("rust/Cargo.toml"), rust).unwrap();
        fs::write(root.join("go/Cargo.toml"), go).unwrap();
        fs::write(
            root.join("rust/crates/app/Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\ncore = { workspace = true }\n",
        )
        .unwrap();

        let workspace = workspace_with(
            [
                pkg(cargo("core"), "rust/crates/core", vec![]),
                pkg(cargo("app"), "rust/crates/app", vec![cargo_core_dep()]),
                pkg(cargo("other"), "go", vec![]),
            ],
            Some("rust"),
            None,
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(cargo("core"), v("0.2.0"))]),
        )
        .expect("apply");

        assert_eq!(
            fs::read_to_string(root.join("rust/Cargo.toml")).unwrap(),
            "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = \"^0.2.0\"\n"
        );
        assert_eq!(fs::read_to_string(root.join("go/Cargo.toml")).unwrap(), go);
        assert_eq!(
            fs::read_to_string(root.join("rust/crates/app/Cargo.toml")).unwrap(),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\ncore = { workspace = true }\n"
        );
    }

    #[test]
    fn inherit_ignores_unused_catalog() {
        let root = scratch("unused-catalog");
        fs::create_dir_all(root.join("js/packages/app")).unwrap();
        fs::create_dir_all(root.join("other")).unwrap();
        fs::write(
            root.join("js/pnpm-workspace.yaml"),
            "catalog:\n  core: '^0.1.0'\n",
        )
        .unwrap();
        let other = "catalog:\n  leftover: '^9.0.0'\n";
        fs::write(root.join("other/pnpm-workspace.yaml"), other).unwrap();

        let workspace = workspace_with(
            [
                pkg(npm("core"), "js/packages/core", vec![]),
                pkg(npm("app"), "js/packages/app", vec![npm_catalog_dep()]),
                pkg(npm("other"), "other", vec![]),
            ],
            None,
            Some("js/pnpm-workspace.yaml"),
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("0.2.0"))]),
        )
        .expect("apply");

        assert_eq!(
            fs::read_to_string(root.join("js/pnpm-workspace.yaml")).unwrap(),
            "catalog:\n  core: '^0.2.0'\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("other/pnpm-workspace.yaml")).unwrap(),
            other
        );
    }

    #[test]
    fn sequence_catalog_yaml_is_an_error() {
        let root = scratch("seq-catalog");
        fs::write(root.join("pnpm-workspace.yaml"), "catalog: []\n").unwrap();
        fs::write(
            root.join("package.json"),
            "{\n  \"catalog\": {\n    \"core\": \"^0.1.0\"\n  }\n}\n",
        )
        .unwrap();

        let workspace = workspace_with(
            [
                pkg(npm("core"), "", vec![]),
                pkg(npm("app"), "", vec![npm_catalog_dep()]),
            ],
            None,
            Some("pnpm-workspace.yaml"),
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("0.2.0"))]),
        )
        .expect_err("sequence catalog");
        assert!(err.to_string().contains("pnpm-workspace.yaml"), "{err}");
        assert!(
            err.to_string().contains("not a valid catalog schema"),
            "{err}"
        );
        assert!(fs::read_to_string(root.join("package.json"))
            .unwrap()
            .contains("\"^0.1.0\""));
    }

    #[test]
    fn inherit_ignores_own_pin_dependent_workspace() {
        let root = scratch("own-pin-dep-ws");
        fs::create_dir_all(root.join("rust/crates/app")).unwrap();
        fs::create_dir_all(root.join("go/crates/cli")).unwrap();
        let rust = "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = \"^0.1.0\"\n";
        let go = "[workspace]\nmembers = [\"crates/cli\"]\n[workspace.dependencies]\ncore = \"^0.1.0\"\n";
        fs::write(root.join("rust/Cargo.toml"), rust).unwrap();
        fs::write(root.join("go/Cargo.toml"), go).unwrap();
        fs::write(
            root.join("rust/crates/app/Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\ncore = { workspace = true }\n",
        )
        .unwrap();
        fs::write(
            root.join("go/crates/cli/Cargo.toml"),
            "[package]\nname = \"cli\"\nversion = \"0.1.0\"\n[dependencies]\ncore = \"^0.1.0\"\n",
        )
        .unwrap();

        let workspace = workspace_with(
            [
                pkg(cargo("core"), "rust/crates/core", vec![]),
                pkg(cargo("app"), "rust/crates/app", vec![cargo_core_dep()]),
                pkg(cargo("cli"), "go/crates/cli", vec![cargo_core_dep()]),
            ],
            Some("rust"),
            None,
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(cargo("core"), v("0.2.0"))]),
        )
        .expect("apply");

        assert_eq!(
            fs::read_to_string(root.join("rust/Cargo.toml")).unwrap(),
            "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = \"^0.2.0\"\n"
        );
        assert_eq!(fs::read_to_string(root.join("go/Cargo.toml")).unwrap(), go);
    }

    #[test]
    fn malformed_member_toml_names_the_package() {
        let root = scratch("bad-member");
        fs::create_dir_all(root.join("crates/app")).unwrap();
        let workspace_toml =
            "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = \"^0.1.0\"\n";
        fs::write(root.join("Cargo.toml"), workspace_toml).unwrap();
        fs::write(root.join("crates/app/Cargo.toml"), "not toml\n").unwrap();

        let workspace = workspace_with(
            [
                pkg(cargo("core"), "crates/core", vec![]),
                pkg(cargo("app"), "crates/app", vec![cargo_core_dep()]),
            ],
            Some(""),
            None,
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(cargo("core"), v("0.2.0"))]),
        )
        .expect_err("member");
        assert!(err.to_string().contains("app"), "{err}");
        assert!(
            err.to_string().contains(&repo_path_display(
                &Path::new("crates/app").join("Cargo.toml"),
            )),
            "{err}"
        );
        assert_eq!(
            fs::read_to_string(root.join("Cargo.toml")).unwrap(),
            workspace_toml
        );
    }

    #[test]
    fn mixed_catalog_object_and_catalogs_sequence_is_an_error() {
        let root = scratch("mixed-catalog-json");
        fs::write(
            root.join("package.json"),
            "{\n  \"catalog\": {},\n  \"catalogs\": []\n}\n",
        )
        .unwrap();

        let workspace = workspace_with(
            [
                pkg(npm("core"), "", vec![]),
                pkg(npm("app"), "", vec![npm_catalog_dep()]),
            ],
            None,
            Some("package.json"),
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("0.2.0"))]),
        )
        .expect_err("mixed catalog");
        assert!(err.to_string().contains("package.json"), "{err}");
        assert!(err.to_string().contains("does not exist"), "{err}");
    }

    #[test]
    fn sequence_catalog_json_is_an_error() {
        let root = scratch("seq-catalog-json");
        fs::write(root.join("package.json"), "{\n  \"catalog\": []\n}\n").unwrap();

        let workspace = workspace_with(
            [
                pkg(npm("core"), "", vec![]),
                pkg(npm("app"), "", vec![npm_catalog_dep()]),
            ],
            None,
            Some("package.json"),
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("0.2.0"))]),
        )
        .expect_err("sequence catalog json");
        assert!(err.to_string().contains("package.json"), "{err}");
        assert!(err.to_string().contains("not an object"), "{err}");
    }

    #[test]
    fn empty_catalogs_yaml_only_names_the_yaml() {
        let root = scratch("empty-catalogs-yaml-only");
        fs::write(root.join("pnpm-workspace.yaml"), "catalogs: {}\n").unwrap();

        let workspace = workspace_with(
            [
                pkg(npm("core"), "", vec![]),
                pkg(npm("app"), "", vec![npm_catalog_dep()]),
            ],
            None,
            Some("pnpm-workspace.yaml"),
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("0.2.0"))]),
        )
        .expect_err("empty catalogs");
        assert!(err.to_string().contains("pnpm-workspace.yaml"), "{err}");
        assert!(err.to_string().contains("catalog/core"), "{err}");
        assert!(err.to_string().contains("does not exist"), "{err}");
    }

    #[test]
    fn empty_catalogs_rewrite_names_the_yaml() {
        let root = scratch("empty-catalogs-rewrite");
        fs::write(root.join("pnpm-workspace.yaml"), "catalogs: {}\n").unwrap();
        fs::write(
            root.join("package.json"),
            "{\n  \"catalog\": {\n    \"core\": \"^0.1.0\"\n  }\n}\n",
        )
        .unwrap();

        let workspace = workspace_with(
            [
                pkg(npm("core"), "", vec![]),
                pkg(npm("app"), "", vec![npm_catalog_dep()]),
            ],
            None,
            Some("pnpm-workspace.yaml"),
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("0.2.0"))]),
        )
        .expect_err("empty catalogs");
        assert!(err.to_string().contains("pnpm-workspace.yaml"), "{err}");
        assert!(err.to_string().contains("catalog/core"), "{err}");
        assert!(err.to_string().contains("does not exist"), "{err}");
        assert!(fs::read_to_string(root.join("package.json"))
            .unwrap()
            .contains("\"^0.1.0\""));
    }

    #[test]
    fn empty_catalog_rewrite_names_the_yaml() {
        let root = scratch("empty-catalog-rewrite");
        fs::write(root.join("pnpm-workspace.yaml"), "catalog: {}\n").unwrap();

        let workspace = workspace_with(
            [
                pkg(npm("core"), "", vec![]),
                pkg(npm("app"), "", vec![npm_catalog_dep()]),
            ],
            None,
            Some("pnpm-workspace.yaml"),
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("0.2.0"))]),
        )
        .expect_err("empty catalog");
        assert!(err.to_string().contains("pnpm-workspace.yaml"), "{err}");
        assert!(err.to_string().contains("catalog/core"), "{err}");
        assert!(err.to_string().contains("does not exist"), "{err}");
    }

    #[test]
    fn invalid_utf8_member_names_the_file() {
        let root = scratch("utf8-member");
        fs::create_dir_all(root.join("crates/app")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = \"^0.1.0\"\n",
        )
        .unwrap();
        fs::write(root.join("crates/app/Cargo.toml"), [0xff, 0xfe, b'c']).unwrap();

        let workspace = workspace_with(
            [
                pkg(cargo("core"), "crates/core", vec![]),
                pkg(cargo("app"), "crates/app", vec![cargo_core_dep()]),
            ],
            Some(""),
            None,
        );

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(cargo("core"), v("0.2.0"))]),
        )
        .expect_err("utf8");
        assert!(
            err.to_string().contains(&repo_path_display(
                &Path::new("crates/app").join("Cargo.toml"),
            )),
            "{err}"
        );
        assert!(err.to_string().contains("failed to read"), "{err}");
    }
}
