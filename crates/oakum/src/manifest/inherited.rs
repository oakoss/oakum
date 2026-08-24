//! Open inherited pins in workspace and catalog files, not member inherit lines.
//!
//! `catalog:` and Cargo `workspace = true` with no `version` key skip in
//! [`super::rewrite_dependency`]. The published range lives in
//! `[workspace.dependencies]` or a catalog file.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use semver::Version;

use crate::plan::{DeclaredRange, Dependency, Ecosystem, Package, PackageId, Workspace};

use super::catalog::{rewrite_catalog_json, rewrite_catalog_yaml, CatalogYamlError};
use super::json::JsonEditError;
use super::rewrite::{cargo_dependency_inherits, RewriteError};

/// Both catalog fields may be `Some`; [`rewrite_inherited_pins`] uses yaml
/// and ignores json (not an error).
#[derive(Clone, Copy, Debug, Default)]
pub struct InheritedSources<'a> {
    pub workspace_toml: Option<&'a str>,
    pub catalog_yaml: Option<&'a str>,
    pub catalog_json: Option<&'a str>,
}

/// `None` means that file was not opened for a pin.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InheritedRewrites {
    workspace_toml: Option<String>,
    catalog_yaml: Option<String>,
    catalog_json: Option<String>,
}

impl InheritedRewrites {
    #[must_use]
    pub fn workspace_toml(&self) -> Option<&str> {
        self.workspace_toml.as_deref()
    }

    #[must_use]
    pub fn catalog_yaml(&self) -> Option<&str> {
        self.catalog_yaml.as_deref()
    }

    #[must_use]
    pub fn catalog_json(&self) -> Option<&str> {
        self.catalog_json.as_deref()
    }
}

/// # Errors
///
/// Returns [`InheritedError`] when a required source is missing, a rewrite
/// fails, or two edges demand different text for the same pin.
pub fn rewrite_inherited_pins(
    workspace: &Workspace,
    new_versions: &BTreeMap<PackageId, Version>,
    members: &BTreeMap<PackageId, &str>,
    sources: InheritedSources<'_>,
) -> Result<InheritedRewrites, InheritedError> {
    let mut cargo_pins = BTreeMap::<String, String>::new();
    let mut catalog_pins = BTreeMap::<(Option<String>, String), String>::new();

    for id in new_versions.keys() {
        for (dependent, dep) in workspace.dependents(id) {
            collect_inherited(
                dependent,
                dep,
                new_versions,
                members,
                &mut cargo_pins,
                &mut catalog_pins,
            )?;
        }
    }

    let mut out = InheritedRewrites::default();
    if !cargo_pins.is_empty() {
        let mut text = sources
            .workspace_toml
            .ok_or(InheritedError::MissingWorkspaceToml)?
            .to_owned();
        for (declared_as, range) in cargo_pins {
            text = super::rewrite_workspace_dependency(&text, &declared_as, &range)?;
        }
        out.workspace_toml = Some(text);
    }

    if !catalog_pins.is_empty() {
        apply_catalog_pins(&mut out, sources, catalog_pins)?;
    }
    Ok(out)
}

/// # Errors
///
/// Returns [`InheritedError`] when a member manifest is missing or not TOML.
pub fn inheriting_cargo_dependents<'a>(
    workspace: &'a Workspace,
    new_versions: &BTreeMap<PackageId, Version>,
    members: &BTreeMap<PackageId, &str>,
) -> Result<Vec<&'a Package>, InheritedError> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for id in new_versions.keys() {
        for (dependent, dep) in workspace.dependents(id) {
            if cargo_member_inherits(dependent, dep, new_versions, members)?
                && seen.insert(dependent.id().clone())
            {
                out.push(dependent);
            }
        }
    }
    Ok(out)
}

fn cargo_member_inherits(
    dependent: &Package,
    dep: &Dependency,
    new_versions: &BTreeMap<PackageId, Version>,
    members: &BTreeMap<PackageId, &str>,
) -> Result<bool, InheritedError> {
    if dependent.id().ecosystem != Ecosystem::Cargo
        || !matches!(dep.range, DeclaredRange::Plain(_))
        || !new_versions.contains_key(&dep.on)
    {
        return Ok(false);
    }
    let member = members
        .get(dependent.id())
        .ok_or_else(|| InheritedError::MissingMember(dependent.id().clone()))?;
    cargo_dependency_inherits(member, dep).map_err(|source| InheritedError::Member {
        package: dependent.id().clone(),
        source,
    })
}

fn collect_inherited(
    dependent: &Package,
    dep: &Dependency,
    new_versions: &BTreeMap<PackageId, Version>,
    members: &BTreeMap<PackageId, &str>,
    cargo_pins: &mut BTreeMap<String, String>,
    catalog_pins: &mut BTreeMap<(Option<String>, String), String>,
) -> Result<(), InheritedError> {
    let Some(new) = new_versions.get(&dep.on) else {
        return Ok(());
    };
    match &dep.range {
        DeclaredRange::Catalog { name, bounds } => {
            let Some(range) = bounds.retargeted(new) else {
                return Err(InheritedError::NotRetargetable {
                    package: dep.on.clone(),
                });
            };
            insert_pin(
                catalog_pins,
                (name.clone(), dep.declared_as.clone()),
                range,
                catalog_pin_name(name.as_deref(), &dep.declared_as),
            )?;
            Ok(())
        }
        DeclaredRange::Plain(bounds) if dependent.id().ecosystem == Ecosystem::Cargo => {
            if !cargo_member_inherits(dependent, dep, new_versions, members)? {
                return Ok(());
            }
            let Some(range) = bounds.retargeted(new) else {
                return Err(InheritedError::NotRetargetable {
                    package: dep.on.clone(),
                });
            };
            insert_pin(
                cargo_pins,
                dep.declared_as.clone(),
                range,
                dep.declared_as.clone(),
            )
        }
        _ => Ok(()),
    }
}

fn catalog_pin_name(name: Option<&str>, package: &str) -> String {
    match name {
        Some(name) => format!("catalog:{name}/{package}"),
        None => format!("catalog:{package}"),
    }
}

fn insert_pin<K: Ord>(
    pins: &mut BTreeMap<K, String>,
    key: K,
    range: String,
    pin: String,
) -> Result<(), InheritedError> {
    if let Some(existing) = pins.get(&key) {
        if existing != &range {
            return Err(InheritedError::ConflictingPin {
                pin,
                left: existing.clone(),
                right: range,
            });
        }
        return Ok(());
    }
    pins.insert(key, range);
    Ok(())
}

fn apply_catalog_pins(
    out: &mut InheritedRewrites,
    sources: InheritedSources<'_>,
    pins: BTreeMap<(Option<String>, String), String>,
) -> Result<(), InheritedError> {
    if let Some(src) = sources.catalog_yaml {
        let mut text = src.to_owned();
        for ((name, package), range) in pins {
            text = rewrite_catalog_yaml(&text, name.as_deref(), &package, &range)?;
        }
        out.catalog_yaml = Some(text);
        return Ok(());
    }
    if let Some(src) = sources.catalog_json {
        let mut text = src.to_owned();
        for ((name, package), range) in pins {
            text = rewrite_catalog_json(&text, name.as_deref(), &package, &range)?;
        }
        out.catalog_json = Some(text);
        return Ok(());
    }
    Err(InheritedError::MissingCatalogFile)
}

#[derive(Debug)]
pub enum InheritedError {
    Rewrite(RewriteError),
    CatalogYaml(CatalogYamlError),
    CatalogJson(JsonEditError),
    MissingWorkspaceToml,
    MissingCatalogFile,
    MissingMember(PackageId),
    Member {
        package: PackageId,
        source: RewriteError,
    },
    ConflictingPin {
        pin: String,
        left: String,
        right: String,
    },
    NotRetargetable {
        package: PackageId,
    },
}

impl From<RewriteError> for InheritedError {
    fn from(err: RewriteError) -> Self {
        Self::Rewrite(err)
    }
}

impl From<CatalogYamlError> for InheritedError {
    fn from(err: CatalogYamlError) -> Self {
        Self::CatalogYaml(err)
    }
}

impl From<JsonEditError> for InheritedError {
    fn from(err: JsonEditError) -> Self {
        Self::CatalogJson(err)
    }
}

impl fmt::Display for InheritedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rewrite(err) => write!(f, "{err}"),
            Self::CatalogYaml(err) => write!(f, "{err}"),
            Self::CatalogJson(err) => write!(f, "{err}"),
            Self::MissingWorkspaceToml => {
                f.write_str("inherited workspace pin needs the root Cargo.toml")
            }
            Self::MissingCatalogFile => f.write_str("inherited catalog pin needs a catalog file"),
            Self::MissingMember(id) => {
                write!(
                    f,
                    "inherited workspace pin is missing the member manifest for {id}"
                )
            }
            Self::Member { package, source } => {
                write!(f, "inherited pin member {package}: {source}")
            }
            Self::ConflictingPin { pin, left, right } => {
                write!(
                    f,
                    "inherited pin `{pin}` would rewrite to both `{left}` and `{right}`"
                )
            }
            Self::NotRetargetable { package } => {
                write!(
                    f,
                    "inherited pin for {package} is not a single-operator range"
                )
            }
        }
    }
}

impl std::error::Error for InheritedError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Rewrite(err) | Self::Member { source: err, .. } => Some(err),
            Self::CatalogYaml(err) => Some(err),
            Self::CatalogJson(err) => Some(err),
            Self::MissingWorkspaceToml
            | Self::MissingCatalogFile
            | Self::MissingMember(_)
            | Self::ConflictingPin { .. }
            | Self::NotRetargetable { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        inheriting_cargo_dependents, rewrite_inherited_pins, InheritedError, InheritedSources,
    };
    use crate::plan::{
        Bounds, DeclaredRange, Dependency, DependencyKind, Ecosystem, Package, PackageId,
        ResolvesDependenciesAt, Tracking, Workspace,
    };
    use semver::Version;
    use std::collections::BTreeMap;

    fn v(text: &str) -> Version {
        Version::parse(text).expect("version")
    }

    fn cargo(name: &str) -> PackageId {
        PackageId::new(Ecosystem::Cargo, name)
    }

    fn npm(name: &str) -> PackageId {
        PackageId::new(Ecosystem::Npm, name)
    }

    fn pkg(id: PackageId, deps: Vec<Dependency>) -> Package {
        Package::new(
            id,
            Version::new(0, 1, 0),
            ResolvesDependenciesAt::Install,
            true,
            deps,
        )
    }

    fn cargo_plain(name: &str, range: &str) -> Dependency {
        Dependency {
            on: cargo(name),
            kind: DependencyKind::Normal,
            declared_as: name.to_owned(),
            target: None,
            range: DeclaredRange::Plain(Bounds::from_cargo_text(range).expect("range")),
        }
    }

    fn catalog(name: Option<&str>, package: &str, range: &str) -> Dependency {
        Dependency {
            on: npm(package),
            kind: DependencyKind::Normal,
            declared_as: package.to_owned(),
            target: None,
            range: DeclaredRange::Catalog {
                name: name.map(str::to_owned),
                bounds: Bounds::from_npm_text(range).expect("range"),
            },
        }
    }

    fn versions(pairs: &[(&PackageId, &str)]) -> BTreeMap<PackageId, Version> {
        pairs
            .iter()
            .map(|(id, ver)| ((*id).clone(), v(ver)))
            .collect()
    }

    #[test]
    fn cargo_inherit_rewrites_workspace_dependencies_once() {
        let workspace = Workspace::new([
            pkg(cargo("core"), vec![]),
            pkg(cargo("app"), vec![cargo_plain("core", "^0.1.0")]),
            pkg(cargo("cli"), vec![cargo_plain("core", "^0.1.0")]),
        ])
        .expect("workspace");
        let member = "[dependencies]\ncore = { workspace = true }\n";
        let members = BTreeMap::from([(cargo("app"), member), (cargo("cli"), member)]);
        let root = "[workspace.dependencies]\ncore = \"^0.1.0\"   # keep\nother = \"1.0.0\"\n";
        let out = rewrite_inherited_pins(
            &workspace,
            &versions(&[(&cargo("core"), "0.2.0")]),
            &members,
            InheritedSources {
                workspace_toml: Some(root),
                ..InheritedSources::default()
            },
        )
        .expect("rewrite");
        assert_eq!(
            out.workspace_toml(),
            Some("[workspace.dependencies]\ncore = \"^0.2.0\"   # keep\nother = \"1.0.0\"\n")
        );
        assert_eq!(out.catalog_yaml(), None);
    }

    #[test]
    fn cargo_member_pin_does_not_rewrite_workspace_table() {
        let workspace = Workspace::new([
            pkg(cargo("core"), vec![]),
            pkg(cargo("app"), vec![cargo_plain("core", "^0.1.0")]),
        ])
        .expect("workspace");
        let member = "[dependencies]\ncore = \"^0.1.0\"\n";
        let members = BTreeMap::from([(cargo("app"), member)]);
        let root = "[workspace.dependencies]\ncore = \"^0.1.0\"\n";
        let out = rewrite_inherited_pins(
            &workspace,
            &versions(&[(&cargo("core"), "0.2.0")]),
            &members,
            InheritedSources {
                workspace_toml: Some(root),
                ..InheritedSources::default()
            },
        )
        .expect("rewrite");
        assert_eq!(out.workspace_toml(), None);
    }

    #[test]
    fn cargo_workspace_true_with_member_version_rewrites_table() {
        let workspace = Workspace::new([
            pkg(cargo("core"), vec![]),
            pkg(cargo("app"), vec![cargo_plain("core", "^0.1.0")]),
        ])
        .expect("workspace");
        let member = "[dependencies]\ncore = { workspace = true, version = \"^0.1.0\" }\n";
        let members = BTreeMap::from([(cargo("app"), member)]);
        let root = "[workspace.dependencies]\ncore = \"^0.1.0\"\n";
        let out = rewrite_inherited_pins(
            &workspace,
            &versions(&[(&cargo("core"), "0.2.0")]),
            &members,
            InheritedSources {
                workspace_toml: Some(root),
                ..InheritedSources::default()
            },
        )
        .expect("rewrite");
        assert_eq!(
            out.workspace_toml(),
            Some("[workspace.dependencies]\ncore = \"^0.2.0\"\n")
        );
    }

    #[test]
    fn cargo_version_workspace_true_rewrites_table() {
        let workspace = Workspace::new([
            pkg(cargo("core"), vec![]),
            pkg(cargo("app"), vec![cargo_plain("core", "^0.1.0")]),
        ])
        .expect("workspace");
        let member = "[dependencies]\ncore = { version.workspace = true }\n";
        let members = BTreeMap::from([(cargo("app"), member)]);
        let root = "[workspace.dependencies]\ncore = \"^0.1.0\"\n";
        let out = rewrite_inherited_pins(
            &workspace,
            &versions(&[(&cargo("core"), "0.2.0")]),
            &members,
            InheritedSources {
                workspace_toml: Some(root),
                ..InheritedSources::default()
            },
        )
        .expect("rewrite");
        assert_eq!(
            out.workspace_toml(),
            Some("[workspace.dependencies]\ncore = \"^0.2.0\"\n")
        );
    }

    #[test]
    fn cargo_dev_inherit_rewrites_workspace_table() {
        let mut dep = cargo_plain("core", "^0.1.0");
        dep.kind = DependencyKind::Development;
        let workspace = Workspace::new([pkg(cargo("core"), vec![]), pkg(cargo("app"), vec![dep])])
            .expect("workspace");
        let member = "[dev-dependencies]\ncore = { workspace = true }\n";
        let members = BTreeMap::from([(cargo("app"), member)]);
        let root = "[workspace.dependencies]\ncore = \"^0.1.0\"\n";
        let out = rewrite_inherited_pins(
            &workspace,
            &versions(&[(&cargo("core"), "0.2.0")]),
            &members,
            InheritedSources {
                workspace_toml: Some(root),
                ..InheritedSources::default()
            },
        )
        .expect("rewrite");
        assert_eq!(
            out.workspace_toml(),
            Some("[workspace.dependencies]\ncore = \"^0.2.0\"\n")
        );
    }

    #[test]
    fn catalog_default_passes_none() {
        let workspace = Workspace::new([
            pkg(npm("core"), vec![]),
            pkg(npm("app"), vec![catalog(None, "core", "^0.1.0")]),
        ])
        .expect("workspace");
        let yaml = "catalog:\n  core: '^0.1.0'   # keep\n";
        let out = rewrite_inherited_pins(
            &workspace,
            &versions(&[(&npm("core"), "0.2.0")]),
            &BTreeMap::new(),
            InheritedSources {
                catalog_yaml: Some(yaml),
                ..InheritedSources::default()
            },
        )
        .expect("rewrite");
        assert_eq!(
            out.catalog_yaml(),
            Some("catalog:\n  core: '^0.2.0'   # keep\n")
        );
        assert_eq!(out.catalog_json(), None);
    }

    #[test]
    fn catalog_named_passes_the_catalog_name() {
        let workspace = Workspace::new([
            pkg(npm("core"), vec![]),
            pkg(npm("app"), vec![catalog(Some("pinned"), "core", "1.0.0")]),
        ])
        .expect("workspace");
        let yaml = "catalog:\n  other: '^9.0.0'\ncatalogs:\n  pinned:\n    core: '1.0.0'\n";
        let out = rewrite_inherited_pins(
            &workspace,
            &versions(&[(&npm("core"), "2.0.0")]),
            &BTreeMap::new(),
            InheritedSources {
                catalog_yaml: Some(yaml),
                ..InheritedSources::default()
            },
        )
        .expect("rewrite");
        assert_eq!(
            out.catalog_yaml(),
            Some("catalog:\n  other: '^9.0.0'\ncatalogs:\n  pinned:\n    core: '2.0.0'\n")
        );
    }

    #[test]
    fn catalog_json_when_no_yaml() {
        let workspace = Workspace::new([
            pkg(npm("core"), vec![]),
            pkg(npm("app"), vec![catalog(None, "core", "^0.1.0")]),
        ])
        .expect("workspace");
        let json = "{\n  \"catalog\": {\n    \"core\": \"^0.1.0\"\n  }\n}\n";
        let out = rewrite_inherited_pins(
            &workspace,
            &versions(&[(&npm("core"), "0.2.0")]),
            &BTreeMap::new(),
            InheritedSources {
                catalog_json: Some(json),
                ..InheritedSources::default()
            },
        )
        .expect("rewrite");
        assert_eq!(
            out.catalog_json(),
            Some("{\n  \"catalog\": {\n    \"core\": \"^0.2.0\"\n  }\n}\n")
        );
        assert_eq!(out.catalog_yaml(), None);
    }

    #[test]
    fn two_catalog_dependents_rewrite_once() {
        let workspace = Workspace::new([
            pkg(npm("core"), vec![]),
            pkg(npm("app"), vec![catalog(None, "core", "^0.1.0")]),
            pkg(npm("cli"), vec![catalog(None, "core", "^0.1.0")]),
        ])
        .expect("workspace");
        let yaml = "catalog:\n  core: '^0.1.0'\n";
        let out = rewrite_inherited_pins(
            &workspace,
            &versions(&[(&npm("core"), "0.2.0")]),
            &BTreeMap::new(),
            InheritedSources {
                catalog_yaml: Some(yaml),
                ..InheritedSources::default()
            },
        )
        .expect("rewrite");
        assert_eq!(out.catalog_yaml(), Some("catalog:\n  core: '^0.2.0'\n"));
    }

    #[test]
    fn conflicting_catalog_ranges_are_an_error() {
        let app = catalog(None, "core", "^0.1.0");
        let mut cli = catalog(None, "core", "~0.1.0");
        cli.kind = DependencyKind::Development;
        let workspace = Workspace::new([
            pkg(npm("core"), vec![]),
            pkg(npm("app"), vec![app]),
            pkg(npm("cli"), vec![cli]),
        ])
        .expect("workspace");
        let err = rewrite_inherited_pins(
            &workspace,
            &versions(&[(&npm("core"), "0.2.0")]),
            &BTreeMap::new(),
            InheritedSources {
                catalog_yaml: Some("catalog:\n  core: '^0.1.0'\n"),
                ..InheritedSources::default()
            },
        )
        .expect_err("conflict");
        match err {
            InheritedError::ConflictingPin { pin, left, right } => {
                assert_eq!(pin, "catalog:core");
                assert_eq!(left, "^0.2.0");
                assert_eq!(right, "~0.2.0");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unretargetable_catalog_is_an_error() {
        let workspace = Workspace::new([
            pkg(npm("core"), vec![]),
            pkg(npm("app"), vec![catalog(None, "core", "1.0.0 || 2.0.0")]),
        ])
        .expect("workspace");
        let err = rewrite_inherited_pins(
            &workspace,
            &versions(&[(&npm("core"), "0.2.0")]),
            &BTreeMap::new(),
            InheritedSources {
                catalog_yaml: Some("catalog:\n  core: '1.0.0 || 2.0.0'\n"),
                ..InheritedSources::default()
            },
        )
        .expect_err("range");
        match err {
            InheritedError::NotRetargetable { package } => assert_eq!(package, npm("core")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unretargetable_cargo_inherit_is_an_error() {
        let workspace = Workspace::new([
            pkg(cargo("core"), vec![]),
            pkg(cargo("app"), vec![cargo_plain("core", ">=0.1.0, <0.2.0")]),
        ])
        .expect("workspace");
        let member = "[dependencies]\ncore = { workspace = true }\n";
        let err = rewrite_inherited_pins(
            &workspace,
            &versions(&[(&cargo("core"), "0.2.0")]),
            &BTreeMap::from([(cargo("app"), member)]),
            InheritedSources {
                workspace_toml: Some("[workspace.dependencies]\ncore = \">=0.1.0, <0.2.0\"\n"),
                ..InheritedSources::default()
            },
        )
        .expect_err("range");
        match err {
            InheritedError::NotRetargetable { package } => assert_eq!(package, cargo("core")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn workspace_tracking_is_not_a_catalog_pin() {
        let dep = Dependency {
            on: npm("core"),
            kind: DependencyKind::Normal,
            declared_as: "core".into(),
            target: None,
            range: DeclaredRange::WorkspaceTracking(Tracking::Exact),
        };
        let workspace = Workspace::new([pkg(npm("core"), vec![]), pkg(npm("app"), vec![dep])])
            .expect("workspace");
        let out = rewrite_inherited_pins(
            &workspace,
            &versions(&[(&npm("core"), "0.2.0")]),
            &BTreeMap::new(),
            InheritedSources {
                catalog_yaml: Some("catalog:\n  core: '^0.1.0'\n"),
                ..InheritedSources::default()
            },
        )
        .expect("rewrite");
        assert_eq!(out.catalog_yaml(), None);
        assert_eq!(out.workspace_toml(), None);
    }

    #[test]
    fn cargo_inherit_without_root_is_missing() {
        let workspace = Workspace::new([
            pkg(cargo("core"), vec![]),
            pkg(cargo("app"), vec![cargo_plain("core", "^0.1.0")]),
        ])
        .expect("workspace");
        let member = "[dependencies]\ncore = { workspace = true }\n";
        let err = rewrite_inherited_pins(
            &workspace,
            &versions(&[(&cargo("core"), "0.2.0")]),
            &BTreeMap::from([(cargo("app"), member)]),
            InheritedSources::default(),
        )
        .expect_err("missing");
        assert!(matches!(err, InheritedError::MissingWorkspaceToml));
    }

    #[test]
    fn catalog_without_file_is_missing() {
        let workspace = Workspace::new([
            pkg(npm("core"), vec![]),
            pkg(npm("app"), vec![catalog(None, "core", "^0.1.0")]),
        ])
        .expect("workspace");
        let err = rewrite_inherited_pins(
            &workspace,
            &versions(&[(&npm("core"), "0.2.0")]),
            &BTreeMap::new(),
            InheritedSources::default(),
        )
        .expect_err("missing");
        assert!(matches!(err, InheritedError::MissingCatalogFile));
    }

    #[test]
    fn cargo_inherit_without_member_text_is_missing() {
        let workspace = Workspace::new([
            pkg(cargo("core"), vec![]),
            pkg(cargo("app"), vec![cargo_plain("core", "^0.1.0")]),
        ])
        .expect("workspace");
        let err = rewrite_inherited_pins(
            &workspace,
            &versions(&[(&cargo("core"), "0.2.0")]),
            &BTreeMap::new(),
            InheritedSources {
                workspace_toml: Some("[workspace.dependencies]\ncore = \"^0.1.0\"\n"),
                ..InheritedSources::default()
            },
        )
        .expect_err("member");
        assert!(matches!(err, InheritedError::MissingMember(_)));
    }

    #[test]
    fn malformed_member_names_the_package() {
        let workspace = Workspace::new([
            pkg(cargo("core"), vec![]),
            pkg(cargo("app"), vec![cargo_plain("core", "^0.1.0")]),
        ])
        .expect("workspace");
        let err = rewrite_inherited_pins(
            &workspace,
            &versions(&[(&cargo("core"), "0.2.0")]),
            &BTreeMap::from([(cargo("app"), "not toml")]),
            InheritedSources {
                workspace_toml: Some("[workspace.dependencies]\ncore = \"^0.1.0\"\n"),
                ..InheritedSources::default()
            },
        )
        .expect_err("toml");
        let message = err.to_string();
        assert!(message.contains("app"), "{message}");
        match err {
            InheritedError::Member { package, .. } => assert_eq!(package, cargo("app")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn inheriting_cargo_dependents_skips_own_pins() {
        let workspace = Workspace::new([
            pkg(cargo("core"), vec![]),
            pkg(cargo("app"), vec![cargo_plain("core", "^0.1.0")]),
            pkg(cargo("cli"), vec![cargo_plain("core", "^0.1.0")]),
            pkg(cargo("other"), vec![]),
        ])
        .expect("workspace");
        let members = BTreeMap::from([
            (
                cargo("app"),
                "[dependencies]\ncore = { workspace = true }\n",
            ),
            (cargo("cli"), "[dependencies]\ncore = \"^0.1.0\"\n"),
        ]);
        let found = inheriting_cargo_dependents(
            &workspace,
            &versions(&[(&cargo("core"), "0.2.0")]),
            &members,
        )
        .expect("inherit");
        assert_eq!(
            found.iter().map(|package| package.id()).collect::<Vec<_>>(),
            vec![&cargo("app")]
        );
    }

    #[test]
    fn cargo_target_inherit_rewrites_workspace_table() {
        let mut dep = cargo_plain("core", "^0.1.0");
        dep.target = Some("cfg(unix)".into());
        let workspace = Workspace::new([pkg(cargo("core"), vec![]), pkg(cargo("app"), vec![dep])])
            .expect("workspace");
        let member = "[target.'cfg(unix)'.dependencies]\ncore = { workspace = true }\n";
        let out = rewrite_inherited_pins(
            &workspace,
            &versions(&[(&cargo("core"), "0.2.0")]),
            &BTreeMap::from([(cargo("app"), member)]),
            InheritedSources {
                workspace_toml: Some("[workspace.dependencies]\ncore = \"^0.1.0\"\n"),
                ..InheritedSources::default()
            },
        )
        .expect("rewrite");
        assert_eq!(
            out.workspace_toml(),
            Some("[workspace.dependencies]\ncore = \"^0.2.0\"\n")
        );
    }
}
