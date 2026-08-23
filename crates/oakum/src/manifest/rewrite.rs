//! Apply [`DeclaredRange::retargeted_text`] to a dependent's manifest.
//!
//! Development edges use the same path as runtime edges (ADR-0008). Tracking
//! tokens, `catalog:`, and path-only lines are left alone. Cargo
//! `workspace = true` with no `version` key is inheritance, not a pin.

use semver::Version;
use toml_edit::{DocumentMut, Item};

use crate::plan::workspace::{DeclaredRange, Dependency, Ecosystem};

use super::{set_preserving_decor, JsonEditError};

/// # Errors
///
/// Returns [`RewriteError`] when any one rewrite fails.
pub fn rewrite_dependencies(
    ecosystem: Ecosystem,
    text: &str,
    deps: &[Dependency],
    new_versions: &std::collections::BTreeMap<crate::plan::workspace::PackageId, Version>,
) -> Result<String, RewriteError> {
    let mut out = text.to_owned();
    for dep in deps {
        let Some(new) = new_versions.get(&dep.on) else {
            continue;
        };
        if let Some(next) = rewrite_dependency(ecosystem, &out, dep, new)? {
            out = next;
        }
    }
    Ok(out)
}

/// Returns `Ok(None)` when the line must not change.
///
/// # Errors
///
/// Returns [`RewriteError`] when the document is missing the section or key,
/// or is not valid TOML/JSONC.
pub fn rewrite_dependency(
    ecosystem: Ecosystem,
    text: &str,
    dep: &Dependency,
    new_version: &Version,
) -> Result<Option<String>, RewriteError> {
    let Some(new_range) = dep.range.retargeted_text(new_version) else {
        return Ok(None);
    };
    match ecosystem {
        Ecosystem::Cargo => rewrite_cargo(text, dep, &new_range),
        Ecosystem::Npm => rewrite_npm(text, dep, &new_range),
    }
}

fn rewrite_cargo(
    text: &str,
    dep: &Dependency,
    new_range: &str,
) -> Result<Option<String>, RewriteError> {
    let mut doc: DocumentMut = text.parse().map_err(RewriteError::Toml)?;
    let item = cargo_dep_item(&mut doc, dep)?;
    if cargo_is_inherited(item) {
        return Ok(None);
    }
    set_cargo_version(item, new_range)?;
    Ok(Some(doc.to_string()))
}

fn rewrite_npm(
    text: &str,
    dep: &Dependency,
    new_range: &str,
) -> Result<Option<String>, RewriteError> {
    let section = dep
        .kind
        .section(Ecosystem::Npm)
        .ok_or(RewriteError::NoSection)?;
    let path = [section, dep.declared_as.as_str()];
    let current = match crate::manifest::json::json_string(text, &path) {
        Ok(value) => value,
        Err(JsonEditError::Missing { path }) if path == section => {
            return Err(RewriteError::MissingSection);
        }
        Err(err) => return Err(err.into()),
    };
    let value = npm_spec(dep, new_range, &current);
    let out = crate::manifest::json::replace_json_string(text, &path, &value)?;
    Ok(Some(out))
}

/// `"core": "npm:core@^1.0.0"` must keep `npm:`; pnpm uses that form to pin
/// the registry instead of workspace linking.
fn npm_spec(dep: &Dependency, new_range: &str, current: &str) -> String {
    if current.starts_with("npm:") {
        return format!("npm:{}@{new_range}", dep.on.name);
    }
    if let Some(rest) = current.strip_prefix("workspace:") {
        if rest.contains('@') {
            let range = new_range.strip_prefix("workspace:").unwrap_or(new_range);
            return format!("workspace:{}@{range}", dep.on.name);
        }
        return new_range.to_owned();
    }
    if dep.declared_as == dep.on.name {
        return new_range.to_owned();
    }
    match &dep.range {
        DeclaredRange::Workspace(_) => {
            let range = new_range.strip_prefix("workspace:").unwrap_or(new_range);
            format!("workspace:{}@{range}", dep.on.name)
        }
        DeclaredRange::Plain(_) => format!("npm:{}@{new_range}", dep.on.name),
        _ => new_range.to_owned(),
    }
}

fn cargo_dep_item<'a>(
    doc: &'a mut DocumentMut,
    dep: &Dependency,
) -> Result<&'a mut Item, RewriteError> {
    let section = dep
        .kind
        .section(Ecosystem::Cargo)
        .ok_or(RewriteError::NoSection)?;
    let table = match dep.target.as_deref() {
        Some(target) => doc
            .get_mut("target")
            .and_then(|item| item.get_mut(target))
            .and_then(|item| item.get_mut(section)),
        None => doc.get_mut(section),
    }
    .ok_or(RewriteError::MissingSection)?;
    table
        .get_mut(dep.declared_as.as_str())
        .ok_or(RewriteError::MissingKey)
}

fn cargo_is_inherited(item: &Item) -> bool {
    version_workspace_true(item) || (item_workspace_true(item) && item.get("version").is_none())
}

fn item_workspace_true(item: &Item) -> bool {
    if let Some(table) = item.as_table() {
        return table.get("workspace").and_then(Item::as_bool) == Some(true);
    }
    if let Some(table) = item.as_inline_table() {
        return table.get("workspace").and_then(toml_edit::Value::as_bool) == Some(true);
    }
    false
}

fn version_workspace_true(item: &Item) -> bool {
    item.get("version").is_some_and(item_workspace_true)
}

fn set_cargo_version(item: &mut Item, new_range: &str) -> Result<(), RewriteError> {
    if item.as_str().is_some() {
        set_preserving_decor(item, new_range);
        return Ok(());
    }
    if item.get("version").is_some() {
        set_preserving_decor(&mut item["version"], new_range);
        return Ok(());
    }
    Err(RewriteError::MissingKey)
}

#[derive(Debug)]
pub enum RewriteError {
    Toml(toml_edit::TomlError),
    Json(JsonEditError),
    NoSection,
    MissingSection,
    MissingKey,
}

impl From<JsonEditError> for RewriteError {
    fn from(err: JsonEditError) -> Self {
        match err {
            JsonEditError::Missing { .. } => Self::MissingKey,
            other => Self::Json(other),
        }
    }
}

impl core::fmt::Display for RewriteError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Toml(err) => write!(f, "{err}"),
            Self::Json(err) => write!(f, "{err}"),
            Self::NoSection => f.write_str("this dependency kind has no section in this ecosystem"),
            Self::MissingSection => f.write_str("manifest is missing the dependency section"),
            Self::MissingKey => f.write_str("manifest is missing the dependency key"),
        }
    }
}

impl std::error::Error for RewriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Toml(err) => Some(err),
            Self::Json(err) => Some(err),
            Self::NoSection | Self::MissingSection | Self::MissingKey => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use semver::Version;

    use crate::plan::bounds::Bounds;
    use crate::plan::workspace::{
        DeclaredRange, Dependency, DependencyKind, Ecosystem, PackageId, Tracking,
    };

    use super::{rewrite_dependencies, rewrite_dependency};

    fn cargo_dep(kind: DependencyKind, name: &str, range: &str) -> Dependency {
        Dependency {
            on: PackageId::new(Ecosystem::Cargo, name),
            kind,
            declared_as: name.to_owned(),
            target: None,
            range: DeclaredRange::Plain(Bounds::from_cargo_text(range).expect("range")),
        }
    }

    fn npm_dep(kind: DependencyKind, name: &str, range: DeclaredRange) -> Dependency {
        Dependency {
            on: PackageId::new(Ecosystem::Npm, name),
            kind,
            declared_as: name.to_owned(),
            target: None,
            range,
        }
    }

    fn v(text: &str) -> Version {
        Version::parse(text).expect("version")
    }

    #[test]
    fn cargo_string_keeps_trailing_comment() {
        let src = "[dependencies]\ncore = \"^0.1.0\"   # keep\n";
        let out = rewrite_dependency(
            Ecosystem::Cargo,
            src,
            &cargo_dep(DependencyKind::Normal, "core", "^0.1.0"),
            &v("0.2.0"),
        )
        .expect("rewrite")
        .expect("changed");
        assert_eq!(out, "[dependencies]\ncore = \"^0.2.0\"   # keep\n");
    }

    #[test]
    fn cargo_target_table_is_rewritten() {
        let src = "[target.'cfg(unix)'.dependencies]\ncore = \"^0.1.0\"\n";
        let mut dep = cargo_dep(DependencyKind::Normal, "core", "^0.1.0");
        dep.target = Some("cfg(unix)".into());
        let out = rewrite_dependency(Ecosystem::Cargo, src, &dep, &v("0.2.0"))
            .expect("rewrite")
            .expect("changed");
        assert_eq!(
            out,
            "[target.'cfg(unix)'.dependencies]\ncore = \"^0.2.0\"\n"
        );
    }

    #[test]
    fn cargo_dev_edge_is_rewritten() {
        let src = "[dev-dependencies]\ncore = \"^0.1.0\"\n";
        let out = rewrite_dependency(
            Ecosystem::Cargo,
            src,
            &cargo_dep(DependencyKind::Development, "core", "^0.1.0"),
            &v("0.2.0"),
        )
        .expect("rewrite")
        .expect("changed");
        assert_eq!(out, "[dev-dependencies]\ncore = \"^0.2.0\"\n");
    }

    #[test]
    fn cargo_build_edge_is_rewritten() {
        let src = "[build-dependencies]\ncore = \"^0.1.0\"\n";
        let out = rewrite_dependency(
            Ecosystem::Cargo,
            src,
            &cargo_dep(DependencyKind::Build, "core", "^0.1.0"),
            &v("0.2.0"),
        )
        .expect("rewrite")
        .expect("changed");
        assert_eq!(out, "[build-dependencies]\ncore = \"^0.2.0\"\n");
    }

    #[test]
    fn cargo_table_version_keeps_path() {
        let src = "[dependencies]\ncore = { version = \"^0.1.0\", path = \"../core\" }\n";
        let out = rewrite_dependency(
            Ecosystem::Cargo,
            src,
            &cargo_dep(DependencyKind::Normal, "core", "^0.1.0"),
            &v("0.2.0"),
        )
        .expect("rewrite")
        .expect("changed");
        assert_eq!(
            out,
            "[dependencies]\ncore = { version = \"^0.2.0\", path = \"../core\" }\n"
        );
    }

    #[test]
    fn cargo_workspace_true_is_left_alone() {
        let src = "[dependencies]\ncore = { workspace = true }\n";
        let out = rewrite_dependency(
            Ecosystem::Cargo,
            src,
            &cargo_dep(DependencyKind::Normal, "core", "^0.1.0"),
            &v("0.2.0"),
        )
        .expect("rewrite");
        assert_eq!(out, None);
    }

    #[test]
    fn cargo_workspace_true_with_version_is_rewritten() {
        let src = "[dependencies]\ncore = { workspace = true, version = \"^0.1.0\" }\n";
        let out = rewrite_dependency(
            Ecosystem::Cargo,
            src,
            &cargo_dep(DependencyKind::Normal, "core", "^0.1.0"),
            &v("0.2.0"),
        )
        .expect("rewrite")
        .expect("changed");
        assert_eq!(
            out,
            "[dependencies]\ncore = { workspace = true, version = \"^0.2.0\" }\n"
        );
    }

    #[test]
    fn tracking_range_is_left_alone() {
        for (spec, tracking) in [
            ("workspace:^", Tracking::Caret),
            ("workspace:*", Tracking::Exact),
            ("workspace:~", Tracking::Tilde),
        ] {
            let src = format!("{{\n  \"dependencies\": {{\n    \"core\": \"{spec}\"\n  }}\n}}\n");
            let dep = npm_dep(
                DependencyKind::Normal,
                "core",
                DeclaredRange::WorkspaceTracking(tracking),
            );
            let out = rewrite_dependency(Ecosystem::Npm, &src, &dep, &v("0.2.0")).expect("rewrite");
            assert_eq!(out, None, "{spec}");
        }
    }

    #[test]
    fn npm_dev_edge_is_rewritten() {
        let src = "{\n  \"devDependencies\": {\n    \"core\": \"^0.1.0\"\n  }\n}\n";
        let dep = npm_dep(
            DependencyKind::Development,
            "core",
            DeclaredRange::Plain(Bounds::from_npm_text("^0.1.3").expect("npm")),
        );
        let out = rewrite_dependency(Ecosystem::Npm, src, &dep, &v("0.2.0"))
            .expect("rewrite")
            .expect("changed");
        assert_eq!(
            out,
            "{\n  \"devDependencies\": {\n    \"core\": \"^0.2.0\"\n  }\n}\n"
        );
    }

    #[test]
    fn npm_peer_edge_is_rewritten() {
        let src = "{\n  \"peerDependencies\": {\n    \"core\": \"^0.1.0\"\n  }\n}\n";
        let dep = npm_dep(
            DependencyKind::Peer,
            "core",
            DeclaredRange::Plain(Bounds::from_npm_text("^0.1.0").expect("npm")),
        );
        let out = rewrite_dependency(Ecosystem::Npm, src, &dep, &v("0.2.0"))
            .expect("rewrite")
            .expect("changed");
        assert_eq!(
            out,
            "{\n  \"peerDependencies\": {\n    \"core\": \"^0.2.0\"\n  }\n}\n"
        );
    }

    #[test]
    fn npm_optional_edge_is_rewritten() {
        let src = "{\n  \"optionalDependencies\": {\n    \"core\": \"^0.1.0\"\n  }\n}\n";
        let dep = npm_dep(
            DependencyKind::Optional,
            "core",
            DeclaredRange::Plain(Bounds::from_npm_text("^0.1.0").expect("npm")),
        );
        let out = rewrite_dependency(Ecosystem::Npm, src, &dep, &v("0.2.0"))
            .expect("rewrite")
            .expect("changed");
        assert_eq!(
            out,
            "{\n  \"optionalDependencies\": {\n    \"core\": \"^0.2.0\"\n  }\n}\n"
        );
    }

    #[test]
    fn npm_workspace_prefix_survives() {
        let src = "{\n  \"dependencies\": {\n    \"core\": \"workspace:^0.1.3\"\n  }\n}\n";
        let dep = npm_dep(
            DependencyKind::Normal,
            "core",
            DeclaredRange::Workspace(Bounds::from_npm_text("^0.1.3").expect("npm")),
        );
        let out = rewrite_dependency(Ecosystem::Npm, src, &dep, &v("0.2.0"))
            .expect("rewrite")
            .expect("changed");
        assert_eq!(
            out,
            "{\n  \"dependencies\": {\n    \"core\": \"workspace:^0.2.0\"\n  }\n}\n"
        );
    }

    #[test]
    fn npm_alias_keeps_protocol_and_target() {
        let src = "{\n  \"dependencies\": {\n    \"core-alias\": \"npm:core@^1.2.3\"\n  }\n}\n";
        let mut dep = npm_dep(
            DependencyKind::Normal,
            "core",
            DeclaredRange::Plain(Bounds::from_npm_text("^1.2.3").expect("npm")),
        );
        dep.declared_as = "core-alias".into();
        let out = rewrite_dependency(Ecosystem::Npm, src, &dep, &v("2.0.0"))
            .expect("rewrite")
            .expect("changed");
        assert_eq!(
            out,
            "{\n  \"dependencies\": {\n    \"core-alias\": \"npm:core@^2.0.0\"\n  }\n}\n"
        );
    }

    #[test]
    fn cargo_path_only_table_is_missing_key() {
        let src = "[dependencies]\ncore = { path = \"../core\" }\n";
        let err = rewrite_dependency(
            Ecosystem::Cargo,
            src,
            &cargo_dep(DependencyKind::Normal, "core", "^0.1.0"),
            &v("0.2.0"),
        )
        .expect_err("path-only");
        assert!(matches!(err, super::RewriteError::MissingKey));
    }

    #[test]
    fn cargo_version_workspace_true_is_left_alone() {
        let src = "[dependencies]\ncore = { version.workspace = true }\n";
        let out = rewrite_dependency(
            Ecosystem::Cargo,
            src,
            &cargo_dep(DependencyKind::Normal, "core", "^0.1.0"),
            &v("0.2.0"),
        )
        .expect("rewrite");
        assert_eq!(out, None);
    }

    #[test]
    fn npm_missing_key_is_an_error() {
        let src = "{\n  \"dependencies\": {\n    \"other\": \"^0.1.0\"\n  }\n}\n";
        let dep = npm_dep(
            DependencyKind::Normal,
            "core",
            DeclaredRange::Plain(Bounds::from_npm_text("^0.1.0").expect("npm")),
        );
        let err = rewrite_dependency(Ecosystem::Npm, src, &dep, &v("0.2.0")).expect_err("missing");
        assert!(matches!(err, super::RewriteError::MissingKey));
    }

    #[test]
    fn npm_missing_section_is_an_error() {
        let src = "{\n  \"name\": \"app\"\n}\n";
        let dep = npm_dep(
            DependencyKind::Normal,
            "core",
            DeclaredRange::Plain(Bounds::from_npm_text("^0.1.0").expect("npm")),
        );
        let err = rewrite_dependency(Ecosystem::Npm, src, &dep, &v("0.2.0")).expect_err("section");
        assert!(matches!(err, super::RewriteError::MissingSection));
    }

    #[test]
    fn npm_same_key_protocol_is_kept() {
        let src = "{\n  \"dependencies\": {\n    \"core\": \"npm:core@^1.2.3\"\n  }\n}\n";
        let dep = npm_dep(
            DependencyKind::Normal,
            "core",
            DeclaredRange::Plain(Bounds::from_npm_text("^1.2.3").expect("npm")),
        );
        let out = rewrite_dependency(Ecosystem::Npm, src, &dep, &v("2.0.0"))
            .expect("rewrite")
            .expect("changed");
        assert_eq!(
            out,
            "{\n  \"dependencies\": {\n    \"core\": \"npm:core@^2.0.0\"\n  }\n}\n"
        );
    }

    #[test]
    fn npm_same_key_workspace_name_is_kept() {
        let src = "{\n  \"dependencies\": {\n    \"core\": \"workspace:core@^1.2.3\"\n  }\n}\n";
        let dep = npm_dep(
            DependencyKind::Normal,
            "core",
            DeclaredRange::Workspace(Bounds::from_npm_text("^1.2.3").expect("npm")),
        );
        let out = rewrite_dependency(Ecosystem::Npm, src, &dep, &v("2.0.0"))
            .expect("rewrite")
            .expect("changed");
        assert_eq!(
            out,
            "{\n  \"dependencies\": {\n    \"core\": \"workspace:core@^2.0.0\"\n  }\n}\n"
        );
    }

    #[test]
    fn npm_workspace_alias_keeps_protocol_and_target() {
        let src =
            "{\n  \"dependencies\": {\n    \"core-alias\": \"workspace:core@^1.2.3\"\n  }\n}\n";
        let mut dep = npm_dep(
            DependencyKind::Normal,
            "core",
            DeclaredRange::Workspace(Bounds::from_npm_text("^1.2.3").expect("npm")),
        );
        dep.declared_as = "core-alias".into();
        let out = rewrite_dependency(Ecosystem::Npm, src, &dep, &v("2.0.0"))
            .expect("rewrite")
            .expect("changed");
        assert_eq!(
            out,
            "{\n  \"dependencies\": {\n    \"core-alias\": \"workspace:core@^2.0.0\"\n  }\n}\n"
        );
    }

    #[test]
    fn rewrite_dependencies_accumulates_and_skips_unmapped() {
        let src = "{\n  \"dependencies\": {\n    \"core\": \"^0.1.0\",\n    \"lib\": \"^0.1.0\",\n    \"other\": \"^0.1.0\"\n  }\n}\n";
        let deps = [
            npm_dep(
                DependencyKind::Normal,
                "core",
                DeclaredRange::Plain(Bounds::from_npm_text("^0.1.0").expect("npm")),
            ),
            npm_dep(
                DependencyKind::Normal,
                "lib",
                DeclaredRange::Plain(Bounds::from_npm_text("^0.1.0").expect("npm")),
            ),
            npm_dep(
                DependencyKind::Normal,
                "other",
                DeclaredRange::Plain(Bounds::from_npm_text("^0.1.0").expect("npm")),
            ),
        ];
        let new_versions = [
            (PackageId::new(Ecosystem::Npm, "core"), v("0.2.0")),
            (PackageId::new(Ecosystem::Npm, "lib"), v("0.3.0")),
        ]
        .into_iter()
        .collect();
        let out = rewrite_dependencies(Ecosystem::Npm, src, &deps, &new_versions).expect("rewrite");
        assert_eq!(
            out,
            "{\n  \"dependencies\": {\n    \"core\": \"^0.2.0\",\n    \"lib\": \"^0.3.0\",\n    \"other\": \"^0.1.0\"\n  }\n}\n"
        );
    }
}
