//! `_config.toml` schema: kebab-case keys, unknown fields refused (ADR-0004 / ADR-0007).
//!
//! Pure parse of a string. The CLI opens the file; this module does not.

use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;

use crate::plan::{BuildResolution, ResolvesDependenciesAt, Versioning};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OakumConfig {
    tool_version: Option<String>,
    change_files: bool,
    conventional_commits: bool,
    versioning: Versioning,
    pr_status: PrStatus,
    tag_format: Option<String>,
    commit_message: Option<String>,
    title: Option<String>,
    template: Option<String>,
    packages: BTreeMap<String, PackageConfig>,
}

/// Presentation channel for pull-request status (ADR-0015). The exit-code gate
/// is not configurable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PrStatus {
    Comment,
    Summary,
    #[default]
    Both,
    None,
}

/// Keyed by the name the manifest declares.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PackageConfig {
    versioning: Option<Versioning>,
    resolves_dependencies_at: Option<ResolvesDependenciesAt>,
}

impl OakumConfig {
    /// Both intent mechanisms on; every other key at its documented default.
    #[must_use]
    pub fn defaults() -> Self {
        Self {
            tool_version: None,
            change_files: true,
            conventional_commits: true,
            versioning: Versioning::ZeroMajor,
            pr_status: PrStatus::Both,
            tag_format: None,
            commit_message: None,
            title: None,
            template: None,
            packages: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn tool_version(&self) -> Option<&str> {
        self.tool_version.as_deref()
    }

    #[must_use]
    pub fn change_files(&self) -> bool {
        self.change_files
    }

    #[must_use]
    pub fn conventional_commits(&self) -> bool {
        self.conventional_commits
    }

    #[must_use]
    pub fn versioning(&self) -> Versioning {
        self.versioning
    }

    #[must_use]
    pub fn pr_status(&self) -> PrStatus {
        self.pr_status
    }

    #[must_use]
    pub fn tag_format(&self) -> Option<&str> {
        self.tag_format.as_deref()
    }

    #[must_use]
    pub fn commit_message(&self) -> Option<&str> {
        self.commit_message.as_deref()
    }

    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    #[must_use]
    pub fn template(&self) -> Option<&str> {
        self.template.as_deref()
    }

    #[must_use]
    pub fn packages(&self) -> &BTreeMap<String, PackageConfig> {
        &self.packages
    }

    #[must_use]
    pub fn versioning_for(&self, package: &str) -> Versioning {
        self.packages
            .get(package)
            .and_then(|pkg| pkg.versioning)
            .map_or(self.versioning, |v| v)
    }

    #[must_use]
    pub fn resolves_dependencies_at(&self, package: &str) -> Option<ResolvesDependenciesAt> {
        self.packages
            .get(package)
            .and_then(|pkg| pkg.resolves_dependencies_at)
    }
}

impl PackageConfig {
    #[must_use]
    pub fn versioning(&self) -> Option<Versioning> {
        self.versioning
    }

    #[must_use]
    pub fn resolves_dependencies_at(&self) -> Option<ResolvesDependenciesAt> {
        self.resolves_dependencies_at
    }
}

#[derive(Debug)]
pub struct ParseError {
    message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ParseError {}

/// `tool-version` is required; omitted preference keys take their defaults.
///
/// # Errors
///
/// Invalid TOML, a missing required key, a value outside the allowed enums, or a
/// key not in the schema.
pub fn parse(text: &str) -> Result<OakumConfig, ParseError> {
    let file: ConfigFile = toml::from_str(text).map_err(|err| ParseError {
        message: err.to_string(),
    })?;
    if !file.change_files && !file.conventional_commits {
        return Err(ParseError {
            message: String::from(
                "both `change-files` and `conventional-commits` are disabled; enable one so the plan has intent to read (ADR-0019 / ADR-0029)",
            ),
        });
    }
    Ok(OakumConfig {
        tool_version: Some(file.tool_version),
        change_files: file.change_files,
        conventional_commits: file.conventional_commits,
        versioning: Versioning::from(file.versioning),
        pr_status: file.pr_status,
        tag_format: nonempty(file.tag_format),
        commit_message: nonempty(file.commit_message),
        title: nonempty(file.title),
        template: nonempty(file.template),
        packages: file
            .packages
            .into_iter()
            .map(|(name, pkg)| (name, pkg.into_package()))
            .collect(),
    })
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.is_empty())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    #[serde(rename = "tool-version")]
    tool_version: String,
    #[serde(default = "default_true", rename = "change-files")]
    change_files: bool,
    #[serde(default = "default_true", rename = "conventional-commits")]
    conventional_commits: bool,
    #[serde(default, rename = "versioning")]
    versioning: VersioningWire,
    #[serde(default, rename = "pr-status")]
    pr_status: PrStatus,
    #[serde(default, rename = "tag-format")]
    tag_format: Option<String>,
    #[serde(default, rename = "commit-message")]
    commit_message: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    template: Option<String>,
    #[serde(default)]
    packages: BTreeMap<String, PackageConfigFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PackageConfigFile {
    #[serde(default)]
    versioning: Option<VersioningWire>,
    #[serde(default, rename = "resolves-dependencies-at")]
    resolves_dependencies_at: Option<ResolvesDependenciesAtWire>,
}

impl PackageConfigFile {
    fn into_package(self) -> PackageConfig {
        PackageConfig {
            versioning: self.versioning.map(Versioning::from),
            resolves_dependencies_at: self
                .resolves_dependencies_at
                .map(ResolvesDependenciesAt::from),
        }
    }
}

/// Wire form: only `"build"` is declarable (ADR-0009). Install/binary stay derived.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ResolvesDependenciesAtWire {
    Build,
}

impl From<ResolvesDependenciesAtWire> for ResolvesDependenciesAt {
    fn from(value: ResolvesDependenciesAtWire) -> Self {
        match value {
            ResolvesDependenciesAtWire::Build => Self::Build(BuildResolution::Declared),
        }
    }
}

fn default_true() -> bool {
    true
}

/// Wire form so serde can refuse unknown strings without putting serde on `plan`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum VersioningWire {
    #[default]
    ZeroMajor,
    Semver,
}

impl From<VersioningWire> for Versioning {
    fn from(value: VersioningWire) -> Self {
        match value {
            VersioningWire::ZeroMajor => Self::ZeroMajor,
            VersioningWire::Semver => Self::Semver,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omitted_preference_keys_take_defaults() {
        let cfg = parse("tool-version = \"0.0.0\"\n").expect("parse");
        assert_eq!(cfg.tool_version(), Some("0.0.0"));
        assert!(cfg.change_files());
        assert!(cfg.conventional_commits());
        assert_eq!(cfg.versioning(), Versioning::ZeroMajor);
        assert_eq!(cfg.pr_status(), PrStatus::Both);
        assert!(cfg.packages().is_empty());
    }

    #[test]
    fn unknown_key_is_an_error() {
        let err = parse("tool-version = \"0.0.0\"\ngit-user = \"x\"\n").expect_err("unknown");
        assert!(
            err.to_string().contains("git-user"),
            "error should name the key: {err}"
        );
    }

    #[test]
    fn snake_case_key_is_unknown() {
        let err = parse("tool-version = \"0.0.0\"\nchange_files = false\n").expect_err("snake");
        assert!(
            err.to_string().contains("change_files"),
            "error should name the snake_case key: {err}"
        );
    }

    #[test]
    fn versioning_and_package_overrides() {
        let text = r#"
tool-version = "0.0.0"
versioning = "zero-major"
pr-status = "summary"
tag-format = "v{{version}}"
commit-message = "release {{version}}"
title = "Release"
template = "changelog.md"

[packages.core]
versioning = "semver"
resolves-dependencies-at = "build"
"#;
        let cfg = parse(text).expect("parse");
        assert_eq!(cfg.versioning(), Versioning::ZeroMajor);
        assert_eq!(cfg.versioning_for("core"), Versioning::Semver);
        assert_eq!(cfg.versioning_for("other"), Versioning::ZeroMajor);
        assert_eq!(
            cfg.resolves_dependencies_at("core"),
            Some(ResolvesDependenciesAt::Build(BuildResolution::Declared))
        );
        assert_eq!(cfg.pr_status(), PrStatus::Summary);
        assert_eq!(cfg.tag_format(), Some("v{{version}}"));
        assert_eq!(cfg.commit_message(), Some("release {{version}}"));
        assert_eq!(cfg.title(), Some("Release"));
        assert_eq!(cfg.template(), Some("changelog.md"));
    }

    #[test]
    fn unknown_package_key_is_an_error() {
        let err = parse("tool-version = \"0.0.0\"\n\n[packages.core]\npublish = true\n")
            .expect_err("unknown package key");
        assert!(
            err.to_string().contains("publish"),
            "error should name the key: {err}"
        );
    }

    #[test]
    fn invalid_versioning_value_is_an_error() {
        let err = parse("tool-version = \"0.0.0\"\nversioning = \"calver\"\n")
            .expect_err("bad versioning");
        assert!(err.to_string().contains("calver"), "{err}");
    }

    #[test]
    fn missing_tool_version_is_an_error() {
        let err = parse("change-files = true\n").expect_err("required");
        assert!(
            err.to_string().contains("tool-version"),
            "error should name the key: {err}"
        );
    }

    #[test]
    fn both_intent_mechanisms_off_is_an_error() {
        let err =
            parse("tool-version = \"0.0.0\"\nchange-files = false\nconventional-commits = false\n")
                .expect_err("both off");
        assert!(
            err.to_string().contains("change-files")
                && err.to_string().contains("conventional-commits"),
            "{err}"
        );
    }

    #[test]
    fn invalid_pr_status_is_an_error() {
        let err =
            parse("tool-version = \"0.0.0\"\npr-status = \"checks\"\n").expect_err("bad pr-status");
        assert!(
            err.to_string().contains("pr-status") || err.to_string().contains("checks"),
            "{err}"
        );
    }

    #[test]
    fn defaults_are_both_intent_on_and_unpinned() {
        let cfg = OakumConfig::defaults();
        assert_eq!(cfg.tool_version(), None);
        assert!(cfg.change_files() && cfg.conventional_commits());
        assert_eq!(cfg.versioning(), Versioning::ZeroMajor);
        assert_eq!(cfg.pr_status(), PrStatus::Both);
    }
}
