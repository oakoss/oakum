//! `_config.toml` schema: kebab-case keys, unknown fields refused (ADR-0004 / ADR-0007).
//!
//! Pure parse of a string. The CLI opens the file; this module does not.

use std::collections::BTreeMap;
use std::fmt;

use semver::{Version, VersionReq};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::plan::{BuildResolution, ResolvesDependenciesAt, Versioning};
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
    TemplateDoesNotExecute,
    MissingToolVersion,
    ToolVersionRequirement,
    UnknownKey,
}

impl ParseErrorKind {
    fn message(&self) -> &'static str {
        match self {
            Self::BothIntentMechanismsDisabled => {
                "both `change-files` and `conventional-commits` are disabled; enable one so the plan has intent to read (ADR-0019 / ADR-0029)"
            }
            Self::DuplicateKey => "duplicate configuration key",
            Self::InvalidSyntax => "invalid TOML syntax",
            Self::InvalidToolVersion => "`tool-version` is not a version",
            Self::InvalidValue => "invalid configuration value",
            Self::TemplateDoesNotExecute => {
                "templates render; they do not execute (ADR-0006)"
            }
            Self::MissingToolVersion => "missing required `tool-version`",
            Self::ToolVersionRequirement => {
                "`tool-version` must be an exact version, not a version requirement"
            }
            Self::UnknownKey => "unknown configuration key",
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
            f.write_str(self.kind.message())
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
        packages: file
            .packages
            .into_iter()
            .map(|(name, pkg)| (name, pkg.into_package()))
            .collect(),
    })
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
        assert_eq!(
            cfg.tool_version(),
            Some(&Version::parse("0.0.0").expect("0.0.0"))
        );
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
                "packages",
                "pr-status",
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
