//! `_config.toml` schema: kebab-case keys, unknown fields refused (ADR-0004 / ADR-0007).
//!
//! Pure parse of a string. The CLI opens the file; this module does not.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};

use semver::{Version, VersionReq};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::plan::{BuildResolution, Package, ResolvesDependenciesAt, Versioning};
use crate::template::TemplateSource;

/// Shown on `versioning` in `_schema.json` (ADR-0022): editors surface this
/// where someone decides how a package reaches `1.0.0`.
pub const VERSIONING_DESCRIPTION: &str = "\
How a breaking change file moves a version. `zero-major` (default) maps a major \
file to a minor bump while the package is below 1.0.0, matching SemVer 2.0 §4. \
`semver` is how a project releases 1.0.0: the next major file produces 1.0.0, \
and the key is then inert for that package. Override per package under `[packages.<name>]` \
so one crate can graduate without taking the rest of the workspace with it.";

/// Exact version grammar (semver.org): no leading zeros; numeric pre-release ids cannot be `01`.
pub const TOOL_VERSION_PATTERN: &str = "\
^(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\
(?:-((?:0|[1-9][0-9]*|[0-9]*[a-zA-Z-][0-9A-Za-z-]*)(?:\\.(?:0|[1-9][0-9]*|[0-9]*[a-zA-Z-][0-9A-Za-z-]*))*))?\
(?:\\+([0-9A-Za-z-]+(?:\\.[0-9A-Za-z-]+)*))?$";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OakumConfig {
    tool_version: Option<Version>,
    change_files: bool,
    conventional_commits: bool,
    versioning: Versioning,
    pr_status: PrStatus,
    tag_format: Option<TemplateSource>,
    commit_message: Option<TemplateSource>,
    title: Option<TemplateSource>,
    template: Option<TemplateSource>,
    private_packages: PrivatePackages,
    include: Vec<String>,
    exclude: Vec<String>,
    packages: BTreeMap<String, PackageConfig>,
}

/// Opt-in for versioning/tagging packages that are not registry-publishable (ADR-0027).
/// Axes are independent: version without tag, or tag without version, are both valid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PrivatePackages {
    version: bool,
    tag: bool,
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
    extra_files: Vec<ExtraFile>,
    /// Stored for a post-v0 publish slot; never executed in v0 (ADR-0011 / ADR-0012).
    publish_command: Option<String>,
}

/// A declared version write outside the package manifest (ADR-0033).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtraFile {
    path: String,
    format: ExtraFileFormat,
    key: String,
}

/// Wire formats `version` can rewrite at a declared key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtraFileFormat {
    Json,
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
            private_packages: PrivatePackages::default(),
            include: Vec::new(),
            exclude: Vec::new(),
            packages: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn tool_version(&self) -> Option<&Version> {
        self.tool_version.as_ref()
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
    pub fn tag_format(&self) -> Option<&TemplateSource> {
        self.tag_format.as_ref()
    }

    #[must_use]
    pub fn commit_message(&self) -> Option<&TemplateSource> {
        self.commit_message.as_ref()
    }

    #[must_use]
    pub fn title(&self) -> Option<&TemplateSource> {
        self.title.as_ref()
    }

    #[must_use]
    pub fn template(&self) -> Option<&TemplateSource> {
        self.template.as_ref()
    }

    pub fn template_sources(&self) -> impl Iterator<Item = (&'static str, &TemplateSource)> {
        [
            ("tag-format", self.tag_format.as_ref()),
            ("commit-message", self.commit_message.as_ref()),
            ("title", self.title.as_ref()),
            ("template", self.template.as_ref()),
        ]
        .into_iter()
        .filter_map(|(key, source)| source.map(|source| (key, source)))
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
            .unwrap_or(self.versioning)
    }

    #[must_use]
    pub fn resolves_dependencies_at(&self, package: &str) -> Option<ResolvesDependenciesAt> {
        self.packages
            .get(package)
            .and_then(|pkg| pkg.resolves_dependencies_at)
    }

    #[must_use]
    pub fn extra_files_for(&self, package: &str) -> &[ExtraFile] {
        self.packages
            .get(package)
            .map_or(&[], PackageConfig::extra_files)
    }

    #[must_use]
    pub fn private_packages(&self) -> PrivatePackages {
        self.private_packages
    }

    #[must_use]
    pub fn include(&self) -> &[String] {
        &self.include
    }

    #[must_use]
    pub fn exclude(&self) -> &[String] {
        &self.exclude
    }

    /// Include (empty = all), then exclude.
    #[must_use]
    pub fn selected(&self, package_name: &str) -> bool {
        let included = self.include.is_empty() || self.include.iter().any(|n| n == package_name);
        included && !self.exclude.iter().any(|n| n == package_name)
    }

    /// Selected and (publishable or `private-packages.version`).
    #[must_use]
    pub fn version_managed(&self, package: &Package) -> bool {
        self.version_managed_name(&package.id().name, package.publishable())
    }

    /// Selected and (publishable or `private-packages.tag`).
    #[must_use]
    pub fn tag_managed(&self, package: &Package) -> bool {
        self.tag_managed_name(&package.id().name, package.publishable())
    }

    /// Same as [`Self::version_managed`] when only a name and publishability flag are available.
    #[must_use]
    pub fn version_managed_name(&self, package_name: &str, publishable: bool) -> bool {
        self.selected(package_name) && (publishable || self.private_packages.version)
    }

    /// Same as [`Self::tag_managed`] when only a name and publishability flag are available.
    #[must_use]
    pub fn tag_managed_name(&self, package_name: &str, publishable: bool) -> bool {
        self.selected(package_name) && (publishable || self.private_packages.tag)
    }

    #[must_use]
    pub fn publish_command_for(&self, package: &str) -> Option<&str> {
        self.packages
            .get(package)
            .and_then(PackageConfig::publish_command)
    }

    /// Call after discovery: [`parse`] has no workspace, so include/exclude
    /// names are not checked there.
    ///
    /// # Errors
    ///
    /// Any include or exclude entry not present in `known`.
    pub fn validate_selection_names<'a>(
        &self,
        known: impl IntoIterator<Item = &'a str>,
    ) -> Result<(), ParseError> {
        let known: HashSet<&str> = known.into_iter().collect();
        for name in self.include.iter().chain(self.exclude.iter()) {
            if !known.contains(name.as_str()) {
                return Err(ParseError::new(ParseErrorKind::UnknownSelectionName(
                    name.clone(),
                )));
            }
        }
        Ok(())
    }
}

impl PrivatePackages {
    #[must_use]
    pub fn version(&self) -> bool {
        self.version
    }

    #[must_use]
    pub fn tag(&self) -> bool {
        self.tag
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

    #[must_use]
    pub fn extra_files(&self) -> &[ExtraFile] {
        &self.extra_files
    }

    #[must_use]
    pub fn publish_command(&self) -> Option<&str> {
        self.publish_command.as_deref()
    }
}

impl ExtraFile {
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    #[must_use]
    pub fn format(&self) -> ExtraFileFormat {
        self.format
    }

    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }
}

#[derive(Debug)]
pub struct ParseError {
    kind: ParseErrorKind,
    location: Option<(usize, usize)>,
}

impl ParseError {
    fn new(kind: ParseErrorKind) -> Self {
        Self {
            kind,
            location: None,
        }
    }

    fn at(kind: ParseErrorKind, text: &str, offset: usize) -> Self {
        Self {
            kind,
            location: Some(line_and_column(text, offset)),
        }
    }
}

#[derive(Debug)]
enum ParseErrorKind {
    BothIntentMechanismsDisabled,
    DuplicateKey,
    InvalidSyntax,
    InvalidToolVersion,
    InvalidValue,
    InvalidExtraFile(String),
    EmptySelectionName,
    EmptyPublishCommand,
    UnknownSelectionName(String),
    TemplateDoesNotExecute,
    MissingToolVersion,
    ToolVersionRequirement,
    UnknownKey,
}

impl ParseErrorKind {
    fn message(&self) -> Cow<'_, str> {
        match self {
            Self::BothIntentMechanismsDisabled => Cow::Borrowed(
                "both `change-files` and `conventional-commits` are disabled; enable one so the plan has intent to read (ADR-0019 / ADR-0029)",
            ),
            Self::DuplicateKey => Cow::Borrowed("duplicate configuration key"),
            Self::InvalidSyntax => Cow::Borrowed("invalid TOML syntax"),
            Self::InvalidToolVersion => Cow::Borrowed("`tool-version` is not a version"),
            Self::InvalidValue => Cow::Borrowed("invalid configuration value"),
            Self::InvalidExtraFile(reason) => {
                Cow::Owned(format!("invalid extra-files entry: {reason}"))
            }
            Self::EmptySelectionName => {
                Cow::Borrowed(
                    "`include` / `exclude` entries must be non-empty package names with no leading or trailing whitespace",
                )
            }
            Self::EmptyPublishCommand => Cow::Borrowed("`publish-command` must not be empty"),
            Self::UnknownSelectionName(name) => {
                Cow::Owned(format!("unknown package name in include/exclude: {name}"))
            }
            Self::TemplateDoesNotExecute => {
                Cow::Borrowed("templates render; they do not execute (ADR-0006)")
            }
            Self::MissingToolVersion => Cow::Borrowed("missing required `tool-version`"),
            Self::ToolVersionRequirement => {
                Cow::Borrowed("`tool-version` must be an exact version, not a version requirement")
            }
            Self::UnknownKey => Cow::Borrowed("unknown configuration key"),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some((line, column)) = self.location {
            write!(
                f,
                "TOML parse error at line {line}, column {column}: {}",
                self.kind.message()
            )
        } else {
            f.write_str(&self.kind.message())
        }
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
    let file: ConfigFile =
        toml::from_str(text).map_err(|error| structured_toml_error(text, &error))?;
    let tool_version = parse_exact_version(file.tool_version.get_ref())
        .map_err(|kind| ParseError::at(kind, text, file.tool_version.span().start))?;
    if !file.change_files && !file.conventional_commits {
        return Err(ParseError::new(
            ParseErrorKind::BothIntentMechanismsDisabled,
        ));
    }
    let include = nonempty_package_names(file.include).map_err(ParseError::new)?;
    let exclude = nonempty_package_names(file.exclude).map_err(ParseError::new)?;
    Ok(OakumConfig {
        tool_version: Some(tool_version),
        change_files: file.change_files,
        conventional_commits: file.conventional_commits,
        versioning: Versioning::from(file.versioning),
        pr_status: file.pr_status,
        tag_format: nonempty_template(file.tag_format),
        commit_message: nonempty_template(file.commit_message),
        title: nonempty_template(file.title),
        template: file.template,
        private_packages: PrivatePackages::from(file.private_packages),
        include,
        exclude,
        packages: file
            .packages
            .into_iter()
            .map(|(name, pkg)| {
                let pkg = pkg.into_package()?;
                Ok((name, pkg))
            })
            .collect::<Result<BTreeMap<_, _>, ParseErrorKind>>()
            .map_err(ParseError::new)?,
    })
}

fn nonempty_package_names(names: Vec<String>) -> Result<Vec<String>, ParseErrorKind> {
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(ParseErrorKind::EmptySelectionName);
        }
        if trimmed != name {
            return Err(ParseErrorKind::EmptySelectionName);
        }
        out.push(name);
    }
    Ok(out)
}

fn structured_toml_error(text: &str, error: &toml::de::Error) -> ParseError {
    let kind = match error.message() {
        message if message.starts_with("unknown field") => ParseErrorKind::UnknownKey,
        message if message.starts_with("missing field `tool-version`") => {
            ParseErrorKind::MissingToolVersion
        }
        message
            if message.starts_with("duplicate field") || message.starts_with("duplicate key") =>
        {
            ParseErrorKind::DuplicateKey
        }
        message if message.contains("do not execute") => ParseErrorKind::TemplateDoesNotExecute,
        message
            if message.starts_with("invalid type")
                || message.starts_with("invalid value")
                || message.starts_with("unknown variant")
                || message.contains("`file` is empty")
                || message.contains("template table needs")
                || message.contains("unknown template table key") =>
        {
            ParseErrorKind::InvalidValue
        }
        _ => ParseErrorKind::InvalidSyntax,
    };
    match error.span() {
        Some(span) => ParseError::at(kind, text, span.start),
        None => ParseError::new(kind),
    }
}

fn line_and_column(text: &str, offset: usize) -> (usize, usize) {
    let prefix = &text.as_bytes()[..offset.min(text.len())];
    let line = prefix.split(|byte| *byte == b'\n').count();
    let current_line = prefix
        .rsplit(|byte| *byte == b'\n')
        .next()
        .unwrap_or_default();
    let column = String::from_utf8_lossy(current_line).chars().count() + 1;
    (line, column)
}

fn nonempty_template(value: Option<TemplateSource>) -> Option<TemplateSource> {
    match value {
        Some(TemplateSource::Inline(body)) if body.is_empty() => None,
        other => other,
    }
}

fn extra_files_schema() -> Value {
    json!({
        "type": "array",
        "description": "Declared version writes outside the package manifest (ADR-0033).",
        "items": {
            "type": "object",
            "additionalProperties": false,
            "required": ["path", "format", "key"],
            "properties": {
                "path": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Leading `/` is repository-root relative; otherwise relative to the package manifest directory.",
                },
                "format": {
                    "type": "string",
                    "enum": ["json"],
                    "description": "Document format. v1 ships json only.",
                },
                "key": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Dotted key path. `{field=value}` selects a unique array element (ADR-0033).",
                },
            },
        },
    })
}

fn template_source_schema(description: &str) -> Value {
    json!({
        "description": description,
        "oneOf": [
            { "type": "string" },
            {
                "type": "object",
                "additionalProperties": false,
                "required": ["file"],
                "properties": {
                    "file": {
                        "type": "string",
                        "minLength": 1,
                        "pattern": r".*\S.*",
                    },
                },
            },
        ],
    })
}

fn parse_exact_version(raw: &str) -> Result<Version, ParseErrorKind> {
    // `1.0.0` is also a valid VersionReq (`^1.0.0`). Try Version first.
    match Version::parse(raw) {
        Ok(version) => Ok(version),
        Err(_) if VersionReq::parse(raw).is_ok() => Err(ParseErrorKind::ToolVersionRequirement),
        Err(_) => Err(ParseErrorKind::InvalidToolVersion),
    }
}

/// Returns `text` with the `tool-version` value replaced, preserving every
/// other byte — comments, ordering, and whitespace included. `upgrade` owns
/// exactly this key (ADR-0023), so the rewrite is a span splice rather than
/// a re-serialization.
///
/// # Errors
///
/// Fails if `text` is not a valid config, or if the spliced result does not
/// re-parse to the intended version.
pub fn set_tool_version(text: &str, version: &Version) -> Result<String, ParseError> {
    let file: ConfigFile =
        toml::from_str(text).map_err(|error| structured_toml_error(text, &error))?;
    let span = file.tool_version.span();
    let updated = format!("{}\"{version}\"{}", &text[..span.start], &text[span.end..]);
    let reparsed = parse(&updated)?;
    if reparsed.tool_version() != Some(version) {
        return Err(ParseError::new(ParseErrorKind::InvalidToolVersion));
    }
    Ok(updated)
}

/// JSON Schema for `.changeset/_config.toml`. `init` / `upgrade` will write it
/// next to the file; taplo reads it via `#:schema ./_schema.json`.
#[must_use]
pub fn schema() -> Value {
    let versioning = json!({
        "type": "string",
        "enum": ["zero-major", "semver"],
        "description": VERSIONING_DESCRIPTION,
    });
    let mut versioning_root = versioning.clone();
    versioning_root["default"] = json!("zero-major");
    json!({
        "$schema": "https://json-schema.org/draft-07/schema#",
        "title": "oakum _config.toml",
        "type": "object",
        "additionalProperties": false,
        "required": ["tool-version"],
        "if": {
            "properties": {
                "change-files": { "const": false },
                "conventional-commits": { "const": false },
            },
            "required": ["change-files", "conventional-commits"],
        },
        "then": false,
        "properties": {
            "tool-version": {
                "type": "string",
                "pattern": TOOL_VERSION_PATTERN,
                "description": "Exact oakum version allowed to run this repository (never a range). A mismatch refuses in both directions; `upgrade` is the exception (ADR-0007).",
            },
            "change-files": {
                "type": "boolean",
                "default": true,
                "description": "When true, the plan reads `.changeset/*.md`.",
            },
            "conventional-commits": {
                "type": "boolean",
                "default": true,
                "description": "When true, commits can feed `generate`, and the plan if change-files is off (ADR-0029).",
            },
            "versioning": versioning_root,
            "pr-status": {
                "type": "string",
                "enum": ["comment", "summary", "both", "none"],
                "default": "both",
                "description": "Pull-request presentation. `none` silences comment and summary; the exit-code gate is not configurable (ADR-0015).",
            },
            "tag-format": template_source_schema(
                "Tag oakum writes. A string is inline; `{ file = \"path\" }` loads a repository-relative file. Existing tags are derived, not configured (ADR-0004).",
            ),
            "commit-message": template_source_schema(
                "Commit message for the version commit. A string is inline; `{ file = \"path\" }` loads a repository-relative file. Templates render; they do not execute (ADR-0006).",
            ),
            "title": template_source_schema(
                "Title for the version pull request. A string is inline; `{ file = \"path\" }` loads a repository-relative file. One template per surface, with conditionals in the body (ADR-0015).",
            ),
            "template": template_source_schema(
                "Changelog template. A string is inline; `{ file = \"path\" }` loads a repository-relative file. Templates render; they do not execute (ADR-0006).",
            ),
            "private-packages": {
                "type": "object",
                "additionalProperties": false,
                "default": { "version": false, "tag": false },
                "description": "Opt-in to version and/or tag packages that are not registry-publishable. Axes are independent (ADR-0027).",
                "properties": {
                    "version": {
                        "type": "boolean",
                        "default": false,
                        "description": "When true, unpublishable packages still receive version bumps and changelogs.",
                    },
                    "tag": {
                        "type": "boolean",
                        "default": false,
                        "description": "When true, unpublishable packages still receive git tags.",
                    },
                },
            },
            "include": {
                "type": "array",
                "items": { "type": "string", "minLength": 1, "pattern": r".*\S.*" },
                "default": [],
                "description": "Exact package names to manage. Empty means all packages; then exclude removes (ADR-0027).",
            },
            "exclude": {
                "type": "array",
                "items": { "type": "string", "minLength": 1, "pattern": r".*\S.*" },
                "default": [],
                "description": "Exact package names to leave alone after include filtering (ADR-0027).",
            },
            "packages": {
                "type": "object",
                "description": "Per-package overrides, keyed by the name the manifest declares.",
                "additionalProperties": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "versioning": versioning,
                        "resolves-dependencies-at": {
                            "type": "string",
                            "enum": ["build"],
                            "description": "Declare that this library bundles dependencies into the published artifact. Binaries are derived; `install` is not configurable (ADR-0009).",
                        },
                        "extra-files": extra_files_schema(),
                        "publish-command": {
                            "type": "string",
                            "minLength": 1,
                            "pattern": r".*\S.*",
                            "description": "Stored publish command for a post-v0 registry slot. Never executed in v0 (ADR-0011 / ADR-0012).",
                        },
                    },
                },
            },
        },
    })
}

/// Pretty JSON Schema plus a trailing newline, the bytes `init` writes.
///
/// # Panics
///
/// Never: `schema()` is a `serde_json::Value`, which always serializes.
#[must_use]
pub fn schema_json() -> String {
    let mut body = serde_json::to_string_pretty(&schema()).expect("schema is valid JSON");
    body.push('\n');
    body
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    #[serde(rename = "tool-version")]
    tool_version: toml::Spanned<String>,
    #[serde(default = "default_true", rename = "change-files")]
    change_files: bool,
    #[serde(default = "default_true", rename = "conventional-commits")]
    conventional_commits: bool,
    #[serde(default, rename = "versioning")]
    versioning: VersioningWire,
    #[serde(default, rename = "pr-status")]
    pr_status: PrStatus,
    #[serde(default, rename = "tag-format")]
    tag_format: Option<TemplateSource>,
    #[serde(default, rename = "commit-message")]
    commit_message: Option<TemplateSource>,
    #[serde(default)]
    title: Option<TemplateSource>,
    #[serde(default)]
    template: Option<TemplateSource>,
    #[serde(default, rename = "private-packages")]
    private_packages: PrivatePackagesFile,
    #[serde(default)]
    include: Vec<String>,
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(default)]
    packages: BTreeMap<String, PackageConfigFile>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrivatePackagesFile {
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
struct PackageConfigFile {
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
    fn into_package(self) -> Result<PackageConfig, ParseErrorKind> {
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
        assert_eq!(
            cfg.tool_version(),
            Some(&Version::parse("0.0.0").expect("0.0.0"))
        );
        assert!(cfg.change_files());
        assert!(cfg.conventional_commits());
        assert_eq!(cfg.versioning(), Versioning::ZeroMajor);
        assert_eq!(cfg.pr_status(), PrStatus::Both);
        assert_eq!(cfg.private_packages(), PrivatePackages::default());
        assert!(!cfg.private_packages().version());
        assert!(!cfg.private_packages().tag());
        assert!(cfg.include().is_empty());
        assert!(cfg.exclude().is_empty());
        assert!(cfg.packages().is_empty());
    }

    #[test]
    fn unknown_key_is_an_error() {
        let err = parse("tool-version = \"0.0.0\"\ngit-user = \"x\"\n").expect_err("unknown");
        assert!(
            err.to_string().contains("unknown configuration key")
                && !err.to_string().contains("git-user"),
            "{err}"
        );
    }

    #[test]
    fn snake_case_key_is_unknown() {
        let err = parse("tool-version = \"0.0.0\"\nchange_files = false\n").expect_err("snake");
        assert!(
            err.to_string().contains("unknown configuration key")
                && !err.to_string().contains("change_files"),
            "{err}"
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
        assert!(cfg.extra_files_for("core").is_empty());
        assert_eq!(cfg.pr_status(), PrStatus::Summary);
        assert_eq!(
            cfg.tag_format(),
            Some(&TemplateSource::Inline(String::from("v{{version}}")))
        );
        assert_eq!(
            cfg.commit_message(),
            Some(&TemplateSource::Inline(String::from("release {{version}}")))
        );
        assert_eq!(
            cfg.title(),
            Some(&TemplateSource::Inline(String::from("Release")))
        );
        assert_eq!(
            cfg.template(),
            Some(&TemplateSource::Inline(String::from("changelog.md")))
        );
    }

    #[test]
    fn template_file_table_parses() {
        let cfg =
            parse("tool-version = \"0.0.0\"\ntemplate = { file = \"notes.md\" }\n").expect("parse");
        assert_eq!(
            cfg.template(),
            Some(&TemplateSource::File(String::from("notes.md")))
        );
    }

    #[test]
    fn template_empty_file_is_refused() {
        let err =
            parse("tool-version = \"0.0.0\"\ntemplate = { file = \"\" }\n").expect_err("empty");
        assert!(
            err.to_string().contains("invalid configuration value"),
            "{err}"
        );
    }

    #[test]
    fn template_command_table_is_refused() {
        let err = parse("tool-version = \"0.0.0\"\ntemplate = { command = \"pandoc\" }\n")
            .expect_err("command");
        assert!(err.to_string().contains("do not execute"), "{err}");
    }

    #[test]
    fn preference_templates_share_the_template_source_shape() {
        for key in ["tag-format", "commit-message", "title", "template"] {
            let cfg = parse(&format!(
                "tool-version = \"0.0.0\"\n{key} = {{ file = \"notes.md\" }}\n"
            ))
            .unwrap_or_else(|err| panic!("{key} file: {err}"));
            assert_eq!(
                source_for(&cfg, key),
                Some(&TemplateSource::File(String::from("notes.md"))),
                "{key}"
            );

            let err = parse(&format!(
                "tool-version = \"0.0.0\"\n{key} = {{ command = \"pandoc\" }}\n"
            ))
            .expect_err(key);
            assert!(err.to_string().contains("do not execute"), "{key}: {err}");

            for blank in [r#"{ file = "" }"#, r#"{ file = " " }"#] {
                let err =
                    parse(&format!("tool-version = \"0.0.0\"\n{key} = {blank}\n")).expect_err(key);
                assert!(
                    err.to_string().contains("invalid configuration value"),
                    "{key} {blank}: {err}"
                );
            }
        }

        for key in ["tag-format", "commit-message", "title"] {
            let empty = parse(&format!("tool-version = \"0.0.0\"\n{key} = \"\"\n"))
                .unwrap_or_else(|err| panic!("{key} empty: {err}"));
            assert_eq!(source_for(&empty, key), None, "{key}");
        }
    }

    #[test]
    fn inline_tag_format_body_renders() {
        let cfg =
            parse("tool-version = \"0.0.0\"\ntag-format = \"v{{ version }}\"\n").expect("parse");
        let TemplateSource::Inline(body) = cfg.tag_format().expect("set") else {
            panic!("expected an inline tag-format");
        };
        let out = crate::template::render("tag-format", body, json!({ "version": "1.2.3" }))
            .expect("render");
        assert_eq!(out, "v1.2.3");
    }

    fn source_for<'a>(cfg: &'a OakumConfig, key: &str) -> Option<&'a TemplateSource> {
        match key {
            "tag-format" => cfg.tag_format(),
            "commit-message" => cfg.commit_message(),
            "title" => cfg.title(),
            "template" => cfg.template(),
            other => panic!("unknown preference template key {other}"),
        }
    }

    #[test]
    fn unknown_package_key_is_an_error() {
        let err = parse("tool-version = \"0.0.0\"\n\n[packages.core]\npublish = true\n")
            .expect_err("unknown package key");
        assert!(
            err.to_string().contains("unknown configuration key")
                && !err.to_string().contains("publish"),
            "{err}"
        );
    }

    #[test]
    fn package_extra_files_parse() {
        let text = r#"
tool-version = "0.0.0"

[[packages.review-cycle.extra-files]]
path = ".claude-plugin/plugin.json"
format = "json"
key = "version"

[[packages.review-cycle.extra-files]]
path = "/.claude-plugin/marketplace.json"
format = "json"
key = "plugins.{name=review-cycle}.version"
"#;
        let cfg = parse(text).expect("parse");
        let extras = cfg.extra_files_for("review-cycle");
        assert_eq!(extras.len(), 2);
        assert_eq!(extras[0].path(), ".claude-plugin/plugin.json");
        assert_eq!(extras[0].format(), ExtraFileFormat::Json);
        assert_eq!(extras[0].key(), "version");
        assert_eq!(extras[1].path(), "/.claude-plugin/marketplace.json");
        assert_eq!(extras[1].key(), "plugins.{name=review-cycle}.version");
        assert!(cfg.extra_files_for("other").is_empty());
    }

    #[test]
    fn package_extra_files_reject_bad_key_and_format() {
        let bad_key = parse(
            r#"
tool-version = "0.0.0"
[[packages.demo.extra-files]]
path = "x.json"
format = "json"
key = "plugins.{name=}.version"
"#,
        )
        .expect_err("empty match value");
        assert!(
            bad_key.to_string().contains("invalid extra-files entry"),
            "{bad_key}"
        );

        let leaf_match = parse(
            r#"
tool-version = "0.0.0"
[[packages.demo.extra-files]]
path = "x.json"
format = "json"
key = "plugins.{name=review-cycle}"
"#,
        )
        .expect_err("match as leaf");
        assert!(
            leaf_match.to_string().contains("cannot end with"),
            "{leaf_match}"
        );

        let bad_format = parse(
            r#"
tool-version = "0.0.0"
[[packages.demo.extra-files]]
path = "x.json"
format = "toml"
key = "version"
"#,
        )
        .expect_err("toml format");
        assert!(
            bad_format
                .to_string()
                .contains("invalid configuration value"),
            "{bad_format}"
        );
    }

    #[test]
    fn package_extra_files_path_and_trim_rules() {
        let empty_path = parse(
            r#"
tool-version = "0.0.0"
[[packages.demo.extra-files]]
path = ""
format = "json"
key = "version"
"#,
        )
        .expect_err("empty path");
        assert!(
            empty_path.to_string().contains("invalid extra-files entry"),
            "{empty_path}"
        );

        let root_path = parse(
            r#"
tool-version = "0.0.0"
[[packages.demo.extra-files]]
path = "/"
format = "json"
key = "version"
"#,
        )
        .expect_err("root path");
        assert!(
            root_path.to_string().contains("invalid extra-files entry"),
            "{root_path}"
        );

        let empty_norm = parse(
            r#"
tool-version = "0.0.0"
[[packages.demo.extra-files]]
path = "/.."
format = "json"
key = "version"
"#,
        )
        .expect_err("empty after normalize");
        assert!(
            empty_norm.to_string().contains("invalid extra-files entry"),
            "{empty_norm}"
        );

        let root_escape = parse(
            r#"
tool-version = "0.0.0"
[[packages.demo.extra-files]]
path = "/../x.json"
format = "json"
key = "version"
"#,
        )
        .expect_err("root escape");
        assert!(
            root_escape.to_string().contains("inside the repository"),
            "{root_escape}"
        );

        let padded = parse(
            r#"
tool-version = "0.0.0"
[[packages.demo.extra-files]]
path = " /.claude-plugin/marketplace.json"
format = "json"
key = " version "
"#,
        )
        .expect("trim");
        let extras = padded.extra_files_for("demo");
        assert_eq!(extras[0].path(), "/.claude-plugin/marketplace.json");
        assert_eq!(extras[0].key(), "version");

        let dotted_match = parse(
            r#"
tool-version = "0.0.0"
[[packages.demo.extra-files]]
path = "x.json"
format = "json"
key = "plugins.{name=foo.bar}.version"
"#,
        )
        .expect("dotted match value");
        assert_eq!(
            dotted_match.extra_files_for("demo")[0].key(),
            "plugins.{name=foo.bar}.version"
        );
    }

    #[test]
    fn duplicate_key_is_an_error() {
        let text = "tool-version = \"0.0.0\"\ntool-version = \"redaction-canary\"\n";
        let err = parse(text).expect_err("duplicate key");
        assert!(
            err.to_string().contains("duplicate configuration key")
                && err.to_string().contains("line 2, column 1")
                && !err.to_string().contains("redaction-canary"),
            "{err}"
        );
    }

    #[test]
    fn invalid_versioning_value_is_an_error() {
        let err = parse("tool-version = \"0.0.0\"\nversioning = \"calver\"\n")
            .expect_err("bad versioning");
        assert!(
            err.to_string().contains("invalid configuration value"),
            "{err}"
        );
        assert!(!err.to_string().contains("calver"), "{err}");
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
    fn tool_version_range_is_an_error() {
        for req in ["^0.0.0", ">=0.0.0", "*", "1"] {
            let err = parse(&format!("tool-version = \"{req}\"\n")).expect_err(req);
            assert!(err.to_string().contains("exact version"), "{req}: {err}");
        }
        let raw = ">=987654.321.123-redaction-canary";
        let err = parse(&format!("tool-version = \"{raw}\"\n")).expect_err(raw);
        assert!(!err.to_string().contains(raw), "{err}");
    }

    #[test]
    fn tool_version_error_uses_value_span() {
        let text = "# tool-version appears in a comment\ntool-version = \"^0.0.0\"\n";
        let err = parse(text).expect_err("version requirement");
        assert!(err.to_string().contains("line 2, column 16"), "{err}");
    }

    #[test]
    fn tool_version_garbage_is_an_error() {
        let raw = "garbage-version-redaction-canary";
        let err = parse(&format!("tool-version = \"{raw}\"\n")).expect_err("garbage");
        assert!(
            err.to_string().contains("not a version") && !err.to_string().contains(raw),
            "{err}"
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
            err.to_string().contains("invalid configuration value")
                && !err.to_string().contains("checks"),
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
        assert_eq!(cfg.private_packages(), PrivatePackages::default());
        assert!(cfg.include().is_empty());
        assert!(cfg.exclude().is_empty());
    }

    #[test]
    fn private_packages_axes_are_independent() {
        let version_only = parse(
            r#"
tool-version = "0.0.0"
private-packages = { version = true, tag = false }
"#,
        )
        .expect("parse");
        assert!(version_only.private_packages().version());
        assert!(!version_only.private_packages().tag());
        assert!(version_only.version_managed_name("priv", false));
        assert!(!version_only.tag_managed_name("priv", false));
        assert!(version_only.version_managed_name("pub", true));
        assert!(version_only.tag_managed_name("pub", true));

        let tag_only = parse(
            r#"
tool-version = "0.0.0"
private-packages = { version = false, tag = true }
"#,
        )
        .expect("parse");
        assert!(!tag_only.private_packages().version());
        assert!(tag_only.private_packages().tag());
        assert!(!tag_only.version_managed_name("priv", false));
        assert!(tag_only.tag_managed_name("priv", false));
    }

    #[test]
    fn include_exclude_selection_math() {
        let all = parse("tool-version = \"0.0.0\"\n").expect("parse");
        assert!(all.selected("a"));
        assert!(all.selected("b"));

        let included = parse(
            r#"
tool-version = "0.0.0"
include = ["a", "b"]
"#,
        )
        .expect("parse");
        assert!(included.selected("a"));
        assert!(included.selected("b"));
        assert!(!included.selected("c"));

        let excluded = parse(
            r#"
tool-version = "0.0.0"
exclude = ["b"]
"#,
        )
        .expect("parse");
        assert!(excluded.selected("a"));
        assert!(!excluded.selected("b"));

        let both = parse(
            r#"
tool-version = "0.0.0"
include = ["a", "b"]
exclude = ["b"]
"#,
        )
        .expect("parse");
        assert!(both.selected("a"));
        assert!(!both.selected("b"));
        assert!(!both.selected("c"));

        // include does not replace private opt-in
        assert!(!both.version_managed_name("a", false));
        assert!(both.version_managed_name("a", true));
        assert!(!both.version_managed_name("b", true));
    }

    #[test]
    fn empty_include_exclude_names_are_refused() {
        for text in [
            "tool-version = \"0.0.0\"\ninclude = [\"\"]\n",
            "tool-version = \"0.0.0\"\nexclude = [\"\"]\n",
            "tool-version = \"0.0.0\"\ninclude = [\"  \"]\n",
            "tool-version = \"0.0.0\"\nexclude = [\"\\t\"]\n",
            "tool-version = \"0.0.0\"\ninclude = [\" foo \"]\n",
            "tool-version = \"0.0.0\"\nexclude = [\"bar \"]\n",
        ] {
            let err = parse(text).expect_err(text);
            assert!(
                err.to_string().contains("non-empty package names"),
                "{text}: {err}"
            );
        }
    }

    #[test]
    fn unknown_selection_names_are_not_validated_at_parse() {
        let cfg = parse(
            r#"
tool-version = "0.0.0"
include = ["ghost"]
exclude = ["also-ghost"]
"#,
        )
        .expect("parse without workspace");
        assert_eq!(cfg.include(), &["ghost".to_owned()]);
        assert_eq!(cfg.exclude(), &["also-ghost".to_owned()]);

        let err = cfg.validate_selection_names(["real"]).expect_err("unknown");
        assert!(
            err.to_string().contains("unknown package name") && err.to_string().contains("ghost"),
            "{err}"
        );

        cfg.validate_selection_names(["ghost", "also-ghost", "extra"])
            .expect("known");
    }

    #[test]
    fn publish_command_parses_and_rejects_empty() {
        let cfg = parse(
            r#"
tool-version = "0.0.0"

[packages.demo]
publish-command = " pnpm publish "
"#,
        )
        .expect("parse");
        assert_eq!(cfg.publish_command_for("demo"), Some("pnpm publish"));
        assert_eq!(
            cfg.packages()
                .get("demo")
                .and_then(PackageConfig::publish_command),
            Some("pnpm publish")
        );
        assert_eq!(cfg.publish_command_for("other"), None);

        for blank in [r#""""#, r#""  ""#] {
            let err = parse(&format!(
                "tool-version = \"0.0.0\"\n\n[packages.demo]\npublish-command = {blank}\n"
            ))
            .expect_err(blank);
            assert!(
                err.to_string().contains("publish-command")
                    && err.to_string().contains("must not be empty"),
                "{blank}: {err}"
            );
        }
    }

    #[test]
    fn set_tool_version_preserves_every_other_byte() {
        let text = "# pinned by upgrade\ntool-version = \"0.0.9\" # trailing note\n\nchange-files = true\n";
        let updated = set_tool_version(text, &Version::parse("0.1.0").expect("version"))
            .expect("set version");
        assert_eq!(
            updated,
            "# pinned by upgrade\ntool-version = \"0.1.0\" # trailing note\n\nchange-files = true\n"
        );
    }

    #[test]
    fn set_tool_version_survives_prerelease_and_key_order() {
        let text = "versioning = \"semver\"\ntool-version = \"1.0.0-rc.1\"\n";
        let updated = set_tool_version(text, &Version::parse("1.0.0").expect("version"))
            .expect("set version");
        assert_eq!(
            updated,
            "versioning = \"semver\"\ntool-version = \"1.0.0\"\n"
        );
        let cfg = parse(&updated).expect("round trip");
        assert_eq!(
            cfg.tool_version(),
            Some(&Version::parse("1.0.0").expect("version"))
        );
    }

    #[test]
    fn set_tool_version_refuses_invalid_toml() {
        let err = set_tool_version("not toml", &Version::parse("0.1.0").expect("version"))
            .expect_err("invalid");
        assert!(err.to_string().contains("TOML") || err.to_string().contains("tool-version"));
    }

    #[test]
    fn schema_refuses_unknown_keys_and_requires_tool_version() {
        let schema = schema();
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["required"], json!(["tool-version"]));
        assert_eq!(
            schema["properties"]["tool-version"]["pattern"],
            TOOL_VERSION_PATTERN
        );
        let mut keys: Vec<&str> = schema["properties"]
            .as_object()
            .expect("properties")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "change-files",
                "commit-message",
                "conventional-commits",
                "exclude",
                "include",
                "packages",
                "pr-status",
                "private-packages",
                "tag-format",
                "template",
                "title",
                "tool-version",
                "versioning",
            ]
        );
        let pkg = &schema["properties"]["packages"]["additionalProperties"];
        assert_eq!(pkg["additionalProperties"], false);
        assert_eq!(pkg["properties"]["versioning"].get("default"), None);
        assert_eq!(
            pkg["properties"]["resolves-dependencies-at"]["enum"],
            json!(["build"])
        );
        assert_eq!(pkg["properties"]["publish-command"]["minLength"], 1);
        assert_eq!(
            schema["properties"]["private-packages"]["default"],
            json!({ "version": false, "tag": false })
        );
        assert_eq!(schema["properties"]["include"]["default"], json!([]));
        assert_eq!(schema["properties"]["exclude"]["default"], json!([]));
        assert_eq!(schema["then"], false);
        for key in ["tag-format", "commit-message", "title", "template"] {
            let file_form = &schema["properties"][key]["oneOf"][1];
            assert_eq!(file_form["additionalProperties"], false, "{key}");
            assert_eq!(file_form["required"], json!(["file"]), "{key}");
            assert_eq!(file_form["properties"]["file"]["minLength"], 1, "{key}");
            assert_eq!(
                file_form["properties"]["file"]["pattern"], r".*\S.*",
                "{key}"
            );
        }
    }

    #[test]
    fn schema_enums_match_parse() {
        let schema = schema();
        assert_eq!(
            schema["properties"]["versioning"]["enum"],
            json!(["zero-major", "semver"])
        );
        assert_eq!(
            schema["properties"]["pr-status"]["enum"],
            json!(["comment", "summary", "both", "none"])
        );
        for value in ["zero-major", "semver"] {
            parse(&format!(
                "tool-version = \"0.0.0\"\nversioning = \"{value}\"\n"
            ))
            .unwrap_or_else(|err| panic!("{value}: {err}"));
        }
        for value in ["comment", "summary", "both", "none"] {
            parse(&format!(
                "tool-version = \"0.0.0\"\npr-status = \"{value}\"\n"
            ))
            .unwrap_or_else(|err| panic!("{value}: {err}"));
        }
        parse(
            "tool-version = \"0.0.0\"\n\n[packages.core]\nresolves-dependencies-at = \"install\"\n",
        )
        .expect_err("install is derived");
        assert_eq!(
            schema["properties"]["packages"]["additionalProperties"]["properties"]["extra-files"]
                ["items"]["properties"]["format"]["enum"],
            json!(["json"])
        );
    }

    #[test]
    fn schema_versioning_description_states_graduation() {
        let schema = schema();
        let description = schema["properties"]["versioning"]["description"]
            .as_str()
            .expect("description");
        assert_eq!(description, VERSIONING_DESCRIPTION);
        assert!(description.contains("1.0.0"), "{description}");
    }

    #[test]
    fn schema_json_round_trips_to_schema() {
        let body = schema_json();
        let parsed: Value = serde_json::from_str(&body).expect("schema_json is JSON");
        assert_eq!(parsed, schema());
        assert!(body.ends_with('\n'));
        assert_eq!(
            body,
            format!("{}\n", serde_json::to_string_pretty(&schema()).unwrap())
        );
    }
}
