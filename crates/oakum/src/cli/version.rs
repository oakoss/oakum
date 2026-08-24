//! `version` opens workspace and catalog files for inherited pins.
//!
//! Member `workspace = true` / `catalog:` lines are not rewritten.

#![allow(dead_code, reason = "the version verb calls this once it lands")]

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use cap_std::fs::Dir;
use oakum::manifest::{
    inheriting_cargo_dependents, json_has_catalog_table, rewrite_inherited_pins,
    yaml_has_catalog_table, InheritedSources,
};
use oakum::plan::{DeclaredRange, Ecosystem, Package, PackageId, Workspace};
use semver::Version;

use super::config::{open_read_only, write_file_via_rename};

const CARGO_TOML: &str = "Cargo.toml";
const PNPM_WORKSPACE: &str = "pnpm-workspace.yaml";
const YARNRC: &str = ".yarnrc.yml";
const PACKAGE_JSON: &str = "package.json";

enum CatalogSource {
    Yaml { path: PathBuf, text: String },
    Json { path: PathBuf, text: String },
}

/// # Errors
///
/// Returns when a required file cannot be read or a writer fails.
pub(super) fn apply_inherited_pins(
    dir: &Dir,
    workspace: &Workspace,
    new_versions: &BTreeMap<PackageId, Version>,
) -> Result<(), Box<dyn std::error::Error>> {
    if new_versions.is_empty() {
        return Ok(());
    }

    let mut member_texts = BTreeMap::new();
    if needs_workspace(workspace, new_versions) {
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
                        format!("{} is missing", path.display()),
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

    let inheritors = inheriting_cargo_dependents(workspace, new_versions, &members)
        .map_err(|err| rewrite_err(err, workspace, None, None))?;
    let workspace_file = if inheritors.is_empty() {
        None
    } else {
        cargo_workspace_toml(dir, inheritors)?
    };
    let catalog_pkgs = catalog_dependents(workspace, new_versions);
    let catalog = if catalog_pkgs.is_empty() {
        None
    } else {
        catalog_source(dir, catalog_pkgs)?
    };
    let (catalog_yaml, catalog_json) = match &catalog {
        Some(CatalogSource::Yaml { text, .. }) => (Some(text.as_str()), None),
        Some(CatalogSource::Json { text, .. }) => (None, Some(text.as_str())),
        None => (None, None),
    };

    let rewritten = rewrite_inherited_pins(
        workspace,
        new_versions,
        &members,
        InheritedSources {
            workspace_toml: workspace_file.as_ref().map(|(_, text)| text.as_str()),
            catalog_yaml,
            catalog_json,
        },
    )
    .map_err(|err| rewrite_err(err, workspace, workspace_file.as_ref(), catalog.as_ref()))?;

    if let (Some((path, original)), Some(text)) = (&workspace_file, rewritten.workspace_toml()) {
        write_file_via_rename(dir, path, text)?;
        if let Err(err) = write_catalog(
            dir,
            catalog.as_ref(),
            rewritten.catalog_yaml(),
            rewritten.catalog_json(),
        ) {
            return Err(restore_workspace(dir, path, original, err));
        }
        return Ok(());
    }
    write_catalog(
        dir,
        catalog.as_ref(),
        rewritten.catalog_yaml(),
        rewritten.catalog_json(),
    )
}

fn write_catalog(
    dir: &Dir,
    catalog: Option<&CatalogSource>,
    yaml: Option<&str>,
    json: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let ((Some(CatalogSource::Yaml { path, .. }), Some(text), _)
    | (Some(CatalogSource::Json { path, .. }), _, Some(text))) = (catalog, yaml, json)
    else {
        return Ok(());
    };
    write_file_via_rename(dir, path, text)
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
        Some(path) => format!("{}: {err}", path.display()).into(),
        None => err.into(),
    }
}

fn restore_workspace(
    dir: &Dir,
    path: &Path,
    original: &str,
    err: Box<dyn std::error::Error>,
) -> Box<dyn std::error::Error> {
    match write_file_via_rename(dir, path, original) {
        Ok(()) => err,
        Err(restore_err) => format!(
            "{err}; restoring {} also failed ({restore_err})",
            path.display()
        )
        .into(),
    }
}

impl CatalogSource {
    fn path(&self) -> &Path {
        match self {
            Self::Yaml { path, .. } | Self::Json { path, .. } => path,
        }
    }
}

fn needs_workspace(workspace: &Workspace, new_versions: &BTreeMap<PackageId, Version>) -> bool {
    new_versions.keys().any(|id| {
        workspace.dependents(id).any(|(dependent, dep)| {
            dependent.id().ecosystem == Ecosystem::Cargo
                && matches!(dep.range, DeclaredRange::Plain(_))
        })
    })
}

fn catalog_dependents<'a>(
    workspace: &'a Workspace,
    new_versions: &BTreeMap<PackageId, Version>,
) -> Vec<&'a Package> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for id in new_versions.keys() {
        for (dependent, dep) in workspace.dependents(id) {
            if matches!(dep.range, DeclaredRange::Catalog { .. })
                && seen.insert(dependent.id().clone())
            {
                out.push(dependent);
            }
        }
    }
    out
}

fn cargo_toml_path(package: &Package) -> PathBuf {
    let dir = package.manifest_dir();
    if dir.is_empty() {
        PathBuf::from(CARGO_TOML)
    } else {
        Path::new(dir).join(CARGO_TOML)
    }
}

fn cargo_workspace_toml<'a>(
    dir: &Dir,
    packages: impl IntoIterator<Item = &'a Package>,
) -> Result<Option<(PathBuf, String)>, Box<dyn std::error::Error>> {
    let mut found = None;
    for package in packages {
        for ancestor in dir_ancestors(package.manifest_dir()) {
            let path = ancestor.join(CARGO_TOML);
            let Some(text) = read_text(dir, &path)? else {
                continue;
            };
            if !cargo_workspace_table(&text, &path)? {
                continue;
            }
            match &found {
                None => found = Some((path, text)),
                Some((existing, _)) if existing != &path => {
                    return Err(duplicate_source("Cargo workspace", existing, &path));
                }
                Some(_) => {}
            }
            break;
        }
    }
    Ok(found)
}

fn catalog_source<'a>(
    dir: &Dir,
    packages: impl IntoIterator<Item = &'a Package>,
) -> Result<Option<CatalogSource>, Box<dyn std::error::Error>> {
    let mut found = None;
    for package in packages {
        for ancestor in dir_ancestors(package.manifest_dir()) {
            let Some(candidate) = catalog_in_dir(dir, &ancestor)? else {
                continue;
            };
            record_catalog(&mut found, candidate)?;
            break;
        }
    }
    Ok(found)
}

fn record_catalog(
    found: &mut Option<CatalogSource>,
    candidate: CatalogSource,
) -> Result<(), Box<dyn std::error::Error>> {
    match found {
        None => *found = Some(candidate),
        Some(existing) if existing.path() != candidate.path() => {
            return Err(duplicate_source(
                "catalog file",
                existing.path(),
                candidate.path(),
            ));
        }
        Some(_) => {}
    }
    Ok(())
}

fn catalog_in_dir(
    dir: &Dir,
    ancestor: &Path,
) -> Result<Option<CatalogSource>, Box<dyn std::error::Error>> {
    for name in [PNPM_WORKSPACE, YARNRC] {
        let path = ancestor.join(name);
        if let Some(text) = read_text(dir, &path)? {
            match yaml_has_catalog_table(&text) {
                Ok(true) => return Ok(Some(CatalogSource::Yaml { path, text })),
                Ok(false) => {}
                Err(err) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("{} is not a catalog YAML file: {err}", path.display()),
                    )
                    .into());
                }
            }
        }
    }
    let path = ancestor.join(PACKAGE_JSON);
    if let Some(text) = read_text(dir, &path)? {
        match json_has_catalog_table(&text) {
            Ok(true) => return Ok(Some(CatalogSource::Json { path, text })),
            Ok(false) => {}
            Err(err) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} is not a catalog JSON file: {err}", path.display()),
                )
                .into());
            }
        }
    }
    Ok(None)
}

fn cargo_workspace_table(text: &str, path: &Path) -> Result<bool, Box<dyn std::error::Error>> {
    let doc = text.parse::<toml_edit::DocumentMut>().map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is not valid TOML: {err}", path.display()),
        )
    })?;
    Ok(doc
        .get("workspace")
        .is_some_and(toml_edit::Item::is_table_like))
}

fn duplicate_source(kind: &str, left: &Path, right: &Path) -> Box<dyn std::error::Error> {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "inherited pins found more than one {kind} ({} and {})",
            left.display(),
            right.display()
        ),
    )
    .into()
}

fn dir_ancestors(manifest_dir: &str) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut current = if manifest_dir.is_empty() {
        PathBuf::new()
    } else {
        PathBuf::from(manifest_dir)
    };
    loop {
        dirs.push(current.clone());
        if current.as_os_str().is_empty() || !current.pop() {
            if !dirs.iter().any(|dir| dir.as_os_str().is_empty()) {
                dirs.push(PathBuf::new());
            }
            break;
        }
    }
    dirs
}

fn read_text(dir: &Dir, path: &Path) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let mut file = match open_read_only(dir, path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(io::Error::new(
                err.kind(),
                format!("failed to open {}: {err}", path.display()),
            )
            .into());
        }
    };
    let meta = file.metadata().map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("failed to inspect {}: {err}", path.display()),
        )
    })?;
    if !meta.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is not a regular file", path.display()),
        )
        .into());
    }
    let mut text = String::new();
    file.read_to_string(&mut text).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("failed to read {}: {err}", path.display()),
        )
    })?;
    Ok(Some(text))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;

    use cap_std::fs::Dir;
    use oakum::plan::{
        Bounds, DeclaredRange, Dependency, DependencyKind, Ecosystem, Package, PackageId,
        ResolvesDependenciesAt, Workspace,
    };
    use semver::Version;

    use super::apply_inherited_pins;

    fn scratch(label: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("oakum-inherited-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
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

        let workspace = Workspace::new([
            pkg(cargo("core"), "rust/crates/core", vec![]),
            pkg(
                cargo("app"),
                "rust/crates/app",
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

        let workspace = Workspace::new([
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
        ])
        .expect("workspace");

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

        let workspace = Workspace::new([
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
        ])
        .expect("workspace");

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

        let workspace = Workspace::new([
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
        ])
        .expect("workspace");

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

        let workspace = Workspace::new([
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
        ])
        .expect("workspace");

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

        let workspace = Workspace::new([
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
        ])
        .expect("workspace");

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

        let workspace = Workspace::new([
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
        ])
        .expect("workspace");

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

        let workspace = Workspace::new([
            pkg(npm("core"), "packages/core", vec![]),
            pkg(npm("app"), "packages/app", vec![npm_catalog_dep()]),
        ])
        .expect("workspace");

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

        let workspace = Workspace::new([
            pkg(npm("core"), "", vec![]),
            pkg(npm("app"), "", vec![npm_catalog_dep()]),
        ])
        .expect("workspace");

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
    fn two_catalog_files_are_an_error() {
        let root = scratch("two-catalog");
        fs::create_dir_all(root.join("js/packages/app")).unwrap();
        fs::create_dir_all(root.join("other/packages/cli")).unwrap();
        let js = "catalog:\n  core: '^0.1.0'\n";
        let other = "catalog:\n  core: '^0.1.0'\n";
        fs::write(root.join("js/pnpm-workspace.yaml"), js).unwrap();
        fs::write(root.join("other/pnpm-workspace.yaml"), other).unwrap();

        let workspace = Workspace::new([
            pkg(npm("core"), "js/packages/core", vec![]),
            pkg(npm("app"), "js/packages/app", vec![npm_catalog_dep()]),
            pkg(npm("cli"), "other/packages/cli", vec![npm_catalog_dep()]),
        ])
        .expect("workspace");

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("0.2.0"))]),
        )
        .expect_err("two catalogs");
        assert!(
            err.to_string().contains("more than one catalog file"),
            "{err}"
        );
        assert_eq!(
            fs::read_to_string(root.join("js/pnpm-workspace.yaml")).unwrap(),
            js
        );
        assert_eq!(
            fs::read_to_string(root.join("other/pnpm-workspace.yaml")).unwrap(),
            other
        );
    }

    #[test]
    fn two_cargo_workspaces_are_an_error() {
        let root = scratch("two-cargo");
        fs::create_dir_all(root.join("rust/crates/app")).unwrap();
        fs::create_dir_all(root.join("go/crates/app")).unwrap();
        let rust = "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = \"^0.1.0\"\n";
        let go = "[workspace]\nmembers = [\"crates/app\"]\n[workspace.dependencies]\ncore = \"^0.1.0\"\n";
        fs::write(root.join("rust/Cargo.toml"), rust).unwrap();
        fs::write(root.join("go/Cargo.toml"), go).unwrap();
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

        let workspace = Workspace::new([
            pkg(cargo("core"), "rust/crates/core", vec![]),
            pkg(cargo("app"), "rust/crates/app", vec![cargo_core_dep()]),
            pkg(cargo("cli"), "go/crates/app", vec![cargo_core_dep()]),
        ])
        .expect("workspace");

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(cargo("core"), v("0.2.0"))]),
        )
        .expect_err("two workspaces");
        assert!(
            err.to_string().contains("more than one Cargo workspace"),
            "{err}"
        );
        assert_eq!(
            fs::read_to_string(root.join("rust/Cargo.toml")).unwrap(),
            rust
        );
        assert_eq!(fs::read_to_string(root.join("go/Cargo.toml")).unwrap(), go);
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

        let workspace = Workspace::new([
            pkg(cargo("core"), "rust/crates/core", vec![]),
            pkg(cargo("app"), "rust/crates/app", vec![cargo_core_dep()]),
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

        let workspace = Workspace::new([
            pkg(cargo("core"), "rust/crates/core", vec![]),
            pkg(cargo("app"), "rust/crates/app", vec![cargo_core_dep()]),
        ])
        .expect("workspace");

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
        Workspace::new([
            pkg(cargo("core"), "rust/crates/core", vec![]),
            pkg(cargo("app"), "rust/crates/app", vec![cargo_core_dep()]),
            pkg(npm("core"), "js/packages/core", vec![]),
            pkg(npm("app"), "js/packages/app", vec![npm_catalog_dep()]),
        ])
        .expect("workspace")
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

        let workspace = Workspace::new([
            pkg(npm("core"), "", vec![]),
            pkg(npm("app"), "", vec![npm_catalog_dep()]),
        ])
        .expect("workspace");

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

        let workspace = Workspace::new([
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
        ])
        .expect("workspace");

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

        let workspace = Workspace::new([
            pkg(cargo("core"), "crates/core", vec![]),
            pkg(cargo("app"), "crates/app", vec![cargo_core_dep()]),
            pkg(npm("js"), "js", vec![]),
            pkg(npm("other"), "other", vec![]),
        ])
        .expect("workspace");

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

        let workspace = Workspace::new([
            pkg(npm("core"), "", vec![]),
            pkg(npm("app"), "", vec![npm_catalog_dep()]),
            pkg(cargo("rust"), "rust", vec![]),
            pkg(cargo("go"), "go", vec![]),
        ])
        .expect("workspace");

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

        let workspace = Workspace::new([
            pkg(cargo("core"), "rust/crates/core", vec![]),
            pkg(cargo("app"), "rust/crates/app", vec![cargo_core_dep()]),
        ])
        .expect("workspace");

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(cargo("core"), v("0.2.0"))]),
        )
        .expect_err("bad toml");
        assert!(err.to_string().contains("not valid TOML"), "{err}");
        assert!(err.to_string().contains("rust/Cargo.toml"), "{err}");
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

        let workspace = Workspace::new([
            pkg(npm("core"), "packages/core", vec![]),
            pkg(npm("app"), "packages/app", vec![npm_catalog_dep()]),
        ])
        .expect("workspace");

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("0.2.0"))]),
        )
        .expect_err("bad json");
        assert!(err.to_string().contains("not a catalog JSON file"), "{err}");
        assert!(
            err.to_string().contains("packages/app/package.json"),
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

        let workspace = Workspace::new([
            pkg(cargo("core"), "crates/core", vec![]),
            pkg(cargo("app"), "crates/app", vec![cargo_core_dep()]),
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
        let workspace = Workspace::new([
            pkg(cargo("core"), "crates/core", vec![]),
            pkg(cargo("app"), "crates/app", vec![dep]),
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

        let workspace = Workspace::new([
            pkg(npm("core"), "", vec![]),
            pkg(npm("app"), "", vec![npm_catalog_dep()]),
        ])
        .expect("workspace");

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("0.2.0"))]),
        )
        .expect_err("bad yaml");
        assert!(err.to_string().contains("not a catalog YAML file"), "{err}");
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

        let workspace = Workspace::new([
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
        ])
        .expect("workspace");

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

        let workspace = Workspace::new([
            pkg(npm("core"), "js/packages/core", vec![]),
            pkg(npm("app"), "js/packages/app", vec![npm_catalog_dep()]),
            pkg(npm("other"), "other", vec![]),
        ])
        .expect("workspace");

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

        let workspace = Workspace::new([
            pkg(npm("core"), "", vec![]),
            pkg(npm("app"), "", vec![npm_catalog_dep()]),
        ])
        .expect("workspace");

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("0.2.0"))]),
        )
        .expect_err("sequence catalog");
        assert!(err.to_string().contains("pnpm-workspace.yaml"), "{err}");
        assert!(err.to_string().contains("not a catalog YAML file"), "{err}");
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

        let workspace = Workspace::new([
            pkg(cargo("core"), "rust/crates/core", vec![]),
            pkg(cargo("app"), "rust/crates/app", vec![cargo_core_dep()]),
            pkg(cargo("cli"), "go/crates/cli", vec![cargo_core_dep()]),
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

        let workspace = Workspace::new([
            pkg(cargo("core"), "crates/core", vec![]),
            pkg(cargo("app"), "crates/app", vec![cargo_core_dep()]),
        ])
        .expect("workspace");

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(cargo("core"), v("0.2.0"))]),
        )
        .expect_err("member");
        assert!(err.to_string().contains("app"), "{err}");
        assert!(err.to_string().contains("crates/app/Cargo.toml"), "{err}");
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

        let workspace = Workspace::new([
            pkg(npm("core"), "", vec![]),
            pkg(npm("app"), "", vec![npm_catalog_dep()]),
        ])
        .expect("workspace");

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("0.2.0"))]),
        )
        .expect_err("mixed catalog");
        assert!(err.to_string().contains("package.json"), "{err}");
        assert!(err.to_string().contains("not a catalog JSON file"), "{err}");
    }

    #[test]
    fn sequence_catalog_json_is_an_error() {
        let root = scratch("seq-catalog-json");
        fs::write(root.join("package.json"), "{\n  \"catalog\": []\n}\n").unwrap();

        let workspace = Workspace::new([
            pkg(npm("core"), "", vec![]),
            pkg(npm("app"), "", vec![npm_catalog_dep()]),
        ])
        .expect("workspace");

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("0.2.0"))]),
        )
        .expect_err("sequence catalog json");
        assert!(err.to_string().contains("package.json"), "{err}");
        assert!(err.to_string().contains("not a catalog JSON file"), "{err}");
    }

    #[test]
    fn empty_catalog_rewrite_names_the_yaml() {
        let root = scratch("empty-catalog-rewrite");
        fs::write(root.join("pnpm-workspace.yaml"), "catalog: {}\n").unwrap();

        let workspace = Workspace::new([
            pkg(npm("core"), "", vec![]),
            pkg(npm("app"), "", vec![npm_catalog_dep()]),
        ])
        .expect("workspace");

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(npm("core"), v("0.2.0"))]),
        )
        .expect_err("empty catalog");
        assert!(err.to_string().contains("pnpm-workspace.yaml"), "{err}");
        assert!(err.to_string().contains("not a rewriteable"), "{err}");
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

        let workspace = Workspace::new([
            pkg(cargo("core"), "crates/core", vec![]),
            pkg(cargo("app"), "crates/app", vec![cargo_core_dep()]),
        ])
        .expect("workspace");

        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = apply_inherited_pins(
            &dir,
            &workspace,
            &BTreeMap::from([(cargo("core"), v("0.2.0"))]),
        )
        .expect_err("utf8");
        assert!(err.to_string().contains("crates/app/Cargo.toml"), "{err}");
        assert!(err.to_string().contains("failed to read"), "{err}");
    }
}
