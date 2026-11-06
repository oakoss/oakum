//! The serde mirror of `_config.toml`, converted into the types callers read.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::plan::{BuildResolution, ResolvesDependenciesAt, Versioning};
use crate::template::TemplateSource;

use super::{ExtraFile, ExtraFileFormat, PackageConfig, ParseErrorKind, PrStatus, PrivatePackages};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ConfigFile {
    /// Unread (ADR-0038): any value parses, so retiring the key fails no repository.
    #[serde(default, rename = "tool-version")]
    pub(super) tool_version: Option<toml::Spanned<toml::Value>>,
    #[serde(default = "default_true", rename = "change-files")]
    pub(super) change_files: bool,
    #[serde(default = "default_true", rename = "conventional-commits")]
    pub(super) conventional_commits: bool,
    #[serde(default, rename = "versioning")]
    pub(super) versioning: VersioningWire,
    #[serde(default, rename = "pr-status")]
    pub(super) pr_status: PrStatus,
    #[serde(default, rename = "tag-format")]
    pub(super) tag_format: Option<TemplateSource>,
    #[serde(default, rename = "commit-message")]
    pub(super) commit_message: Option<TemplateSource>,
    #[serde(default)]
    pub(super) title: Option<TemplateSource>,
    #[serde(default)]
    pub(super) template: Option<TemplateSource>,
    #[serde(default, rename = "private-packages")]
    pub(super) private_packages: PrivatePackagesFile,
    #[serde(default)]
    pub(super) include: Vec<String>,
    #[serde(default)]
    pub(super) exclude: Vec<String>,
    #[serde(default)]
    pub(super) packages: BTreeMap<String, PackageConfigFile>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PrivatePackagesFile {
    #[serde(default)]
    version: bool,
    #[serde(default)]
    tag: bool,
}

impl From<PrivatePackagesFile> for PrivatePackages {
    fn from(value: PrivatePackagesFile) -> Self {
        Self {
            version: value.version,
            tag: value.tag,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PackageConfigFile {
    #[serde(default)]
    versioning: Option<VersioningWire>,
    #[serde(default, rename = "resolves-dependencies-at")]
    resolves_dependencies_at: Option<ResolvesDependenciesAtWire>,
    #[serde(default, rename = "extra-files")]
    extra_files: Vec<ExtraFileFile>,
    #[serde(default, rename = "publish-command")]
    publish_command: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExtraFileFile {
    path: String,
    format: ExtraFileFormatWire,
    key: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ExtraFileFormatWire {
    Json,
}

impl PackageConfigFile {
    pub(super) fn into_package(self) -> Result<PackageConfig, ParseErrorKind> {
        let mut extra_files = Vec::with_capacity(self.extra_files.len());
        for entry in self.extra_files {
            let path = entry.path.trim();
            let relative = path.strip_prefix('/').unwrap_or(path);
            let normalized = if path.starts_with('/') {
                match lexical_normalize_strict(relative) {
                    Ok(normalized) => normalized,
                    Err(()) => {
                        return Err(ParseErrorKind::InvalidExtraFile(String::from(
                            "path must stay inside the repository",
                        )));
                    }
                }
            } else {
                lexical_normalize_extra_path(relative)
            };
            if path.is_empty() || path == "/" || normalized.as_os_str().is_empty() {
                return Err(ParseErrorKind::InvalidExtraFile(String::from(
                    "path must be a non-empty relative path (not `/` alone)",
                )));
            }
            let key = entry.key.trim();
            if key.is_empty() {
                return Err(ParseErrorKind::InvalidExtraFile(String::from(
                    "key must not be empty",
                )));
            }
            crate::manifest::parse_write_key_path(key)
                .map_err(|err| ParseErrorKind::InvalidExtraFile(err.to_string()))?;
            extra_files.push(ExtraFile {
                path: path.to_owned(),
                format: ExtraFileFormat::from(entry.format),
                key: key.to_owned(),
            });
        }
        let publish_command = match self.publish_command {
            None => None,
            Some(raw) => {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    return Err(ParseErrorKind::EmptyPublishCommand);
                }
                Some(trimmed.to_owned())
            }
        };
        Ok(PackageConfig {
            versioning: self.versioning.map(Versioning::from),
            resolves_dependencies_at: self
                .resolves_dependencies_at
                .map(ResolvesDependenciesAt::from),
            extra_files,
            publish_command,
        })
    }
}

impl From<ExtraFileFormatWire> for ExtraFileFormat {
    fn from(value: ExtraFileFormatWire) -> Self {
        match value {
            ExtraFileFormatWire::Json => Self::Json,
        }
    }
}

/// Collapse `.` / `..` for empty-path checks on package-relative declarations.
/// Escape after joining the package directory belongs to `version` (ADR-0033).
fn lexical_normalize_extra_path(path: &str) -> PathBuf {
    let mut out = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
            Component::Normal(s) => out.push(s),
        }
    }
    out
}

/// Root-relative declarations cannot use unmatched `..` (same rule as `version`).
fn lexical_normalize_strict(path: &str) -> Result<PathBuf, ()> {
    let mut out = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::ParentDir => {
                if !out.pop() {
                    return Err(());
                }
            }
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
            Component::Normal(s) => out.push(s),
        }
    }
    Ok(out)
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
pub(super) enum VersioningWire {
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
