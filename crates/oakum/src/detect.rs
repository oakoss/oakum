//! Detect another release tool so `init` can name `migrate` (`okm-0s5`).
//!
//! Pure: callers list paths and pass file bodies. I/O stays on the CLI side of
//! ADR-0002. Markers are from `docs/specs/init.md`. `None` on a body means the
//! file is absent, not unread. The caller must not invoke [`detect`] after a
//! failed read.

use crate::changeset::listing_contains_bump_file;

/// A surveyed release tool `init` must not overwrite.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReleaseTool {
    Knope,
    Changesets,
    Bumpy,
    ReleasePlease,
    ReleasePlz,
    SemanticRelease,
    NxRelease,
}

impl ReleaseTool {
    /// Spec table name, used when telling the user to run `oakum migrate`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Knope => "knope",
            Self::Changesets => "changesets",
            Self::Bumpy => "bumpy",
            Self::ReleasePlease => "release-please",
            Self::ReleasePlz => "release-plz",
            Self::SemanticRelease => "semantic-release",
            Self::NxRelease => "nx release",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detection {
    tool: ReleaseTool,
    evidence: String,
}

impl Detection {
    fn new(tool: ReleaseTool, evidence: impl Into<String>) -> Self {
        Self {
            tool,
            evidence: evidence.into(),
        }
    }

    #[must_use]
    pub fn tool(&self) -> ReleaseTool {
        self.tool
    }

    #[must_use]
    pub fn evidence(&self) -> &str {
        &self.evidence
    }
}

/// Why a marker file could not be classified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DetectError {
    Json { source: String, message: String },
    Toml { source: String, message: String },
}

impl core::fmt::Display for DetectError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Json { source, message } => {
                write!(f, "`{source}` is not valid JSON: {message}")
            }
            Self::Toml { source, message } => {
                write!(f, "`{source}` is not valid TOML: {message}")
            }
        }
    }
}

impl core::error::Error for DetectError {}

/// A parse error must not drop tools already found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DetectReport {
    pub detections: Vec<Detection>,
    pub errors: Vec<DetectError>,
}

/// `None` on `cargo_toml` / `package_json` / `nx_json` means the file is absent.
pub struct DetectInput<'a> {
    /// Repo-relative paths that exist (`knope.toml`, `.changeset/feat.md`, …).
    pub relative_paths: &'a [&'a str],
    /// Direct children of `.changeset/` that are regular files, if that
    /// directory exists.
    pub changeset_names: Option<&'a [&'a str]>,
    pub cargo_toml: Option<&'a str>,
    pub package_json: Option<&'a str>,
    pub nx_json: Option<&'a str>,
}

/// Every matching tool, in spec-table order, one row per tool.
#[must_use]
pub fn detect(input: &DetectInput<'_>) -> DetectReport {
    let mut found = Vec::new();
    let mut errors = Vec::new();
    push_unique(
        &mut found,
        path_hit(input.relative_paths, "knope.toml", ReleaseTool::Knope),
    );
    push_unique(&mut found, changesets_hit(input));
    push_unique(
        &mut found,
        path_hit(
            input.relative_paths,
            ".bumpy/_config.json",
            ReleaseTool::Bumpy,
        ),
    );
    push_unique(&mut found, release_please_hit(input.relative_paths));
    match release_plz_hit(input) {
        Ok(hit) => push_unique(&mut found, hit),
        Err(err) => errors.push(err),
    }
    match semantic_release_hit(input) {
        Ok(hit) => push_unique(&mut found, hit),
        Err(err) => errors.push(err),
    }
    match nx_release_hit(input.nx_json) {
        Ok(hit) => push_unique(&mut found, hit),
        Err(err) => errors.push(err),
    }
    DetectReport {
        detections: found,
        errors,
    }
}

fn push_unique(found: &mut Vec<Detection>, hit: Option<Detection>) {
    let Some(hit) = hit else {
        return;
    };
    if found.iter().any(|existing| existing.tool() == hit.tool()) {
        return;
    }
    found.push(hit);
}

fn path_hit(paths: &[&str], marker: &str, tool: ReleaseTool) -> Option<Detection> {
    paths
        .contains(&marker)
        .then(|| Detection::new(tool, marker))
}

fn changesets_hit(input: &DetectInput<'_>) -> Option<Detection> {
    if path_hit(
        input.relative_paths,
        ".changeset/config.json",
        ReleaseTool::Changesets,
    )
    .is_some()
    {
        return Some(Detection::new(
            ReleaseTool::Changesets,
            ".changeset/config.json",
        ));
    }
    let names = input.changeset_names?;
    if listing_contains_bump_file(names.iter().copied()) {
        return Some(Detection::new(ReleaseTool::Changesets, ".changeset/"));
    }
    None
}

fn release_please_hit(paths: &[&str]) -> Option<Detection> {
    path_hit(
        paths,
        "release-please-config.json",
        ReleaseTool::ReleasePlease,
    )
    .or_else(|| {
        path_hit(
            paths,
            ".release-please-manifest.json",
            ReleaseTool::ReleasePlease,
        )
    })
}

fn release_plz_hit(input: &DetectInput<'_>) -> Result<Option<Detection>, DetectError> {
    if let Some(hit) = path_hit(
        input.relative_paths,
        "release-plz.toml",
        ReleaseTool::ReleasePlz,
    )
    .or_else(|| {
        path_hit(
            input.relative_paths,
            ".release-plz.toml",
            ReleaseTool::ReleasePlz,
        )
    }) {
        return Ok(Some(hit));
    }
    let Some(text) = input.cargo_toml else {
        return Ok(None);
    };
    if cargo_toml_has_release_plz(text)? {
        return Ok(Some(Detection::new(ReleaseTool::ReleasePlz, "Cargo.toml")));
    }
    Ok(None)
}

fn semantic_release_hit(input: &DetectInput<'_>) -> Result<Option<Detection>, DetectError> {
    if let Some(name) = input
        .relative_paths
        .iter()
        .copied()
        .find(|path| is_releaserc_name(path))
    {
        return Ok(Some(Detection::new(ReleaseTool::SemanticRelease, name)));
    }
    if let Some(name) = input
        .relative_paths
        .iter()
        .copied()
        .find(|path| is_release_config_name(path))
    {
        return Ok(Some(Detection::new(ReleaseTool::SemanticRelease, name)));
    }
    let Some(text) = input.package_json else {
        return Ok(None);
    };
    if json_has_top_level_release(text, "package.json")? {
        return Ok(Some(Detection::new(
            ReleaseTool::SemanticRelease,
            "package.json",
        )));
    }
    Ok(None)
}

fn nx_release_hit(nx_json: Option<&str>) -> Result<Option<Detection>, DetectError> {
    let Some(text) = nx_json else {
        return Ok(None);
    };
    if json_has_top_level_release(text, "nx.json")? {
        return Ok(Some(Detection::new(ReleaseTool::NxRelease, "nx.json")));
    }
    Ok(None)
}

/// `.releaserc` and `.releaserc.<ext>` at the repository root, not nested.
#[must_use]
pub fn is_releaserc_name(path: &str) -> bool {
    if path.contains('/') || path.contains('\\') {
        return false;
    }
    path == ".releaserc" || path.starts_with(".releaserc.")
}

/// `release.config.{js,cjs,mjs,ts}` at the repository root, not nested.
#[must_use]
pub fn is_release_config_name(path: &str) -> bool {
    if path.contains('/') || path.contains('\\') {
        return false;
    }
    matches!(
        path,
        "release.config.js" | "release.config.cjs" | "release.config.mjs" | "release.config.ts"
    )
}

fn json_has_top_level_release(text: &str, source: &str) -> Result<bool, DetectError> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|err| DetectError::Json {
        source: String::from(source),
        message: err.to_string(),
    })?;
    Ok(value.get("release").is_some())
}

fn cargo_toml_has_release_plz(text: &str) -> Result<bool, DetectError> {
    let value: toml::Value = toml::from_str(text).map_err(|err| DetectError::Toml {
        source: String::from("Cargo.toml"),
        message: err.to_string(),
    })?;
    Ok(metadata_has_release_plz(
        value.get("workspace").and_then(|w| w.get("metadata")),
    ))
}

fn metadata_has_release_plz(metadata: Option<&toml::Value>) -> bool {
    let Some(table) = metadata.and_then(toml::Value::as_table) else {
        return false;
    };
    // Spec writes `release_plz`; the published tool uses `release-plz`.
    table.contains_key("release_plz") || table.contains_key("release-plz")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(
        paths: &'a [&'a str],
        changeset: Option<&'a [&'a str]>,
        cargo: Option<&'a str>,
        package: Option<&'a str>,
        nx: Option<&'a str>,
    ) -> DetectInput<'a> {
        DetectInput {
            relative_paths: paths,
            changeset_names: changeset,
            cargo_toml: cargo,
            package_json: package,
            nx_json: nx,
        }
    }

    fn tools(found: &[Detection]) -> Vec<ReleaseTool> {
        found.iter().map(Detection::tool).collect()
    }

    fn report<'a>(
        paths: &'a [&'a str],
        changeset: Option<&'a [&'a str]>,
        cargo: Option<&'a str>,
        package: Option<&'a str>,
        nx: Option<&'a str>,
    ) -> DetectReport {
        detect(&input(paths, changeset, cargo, package, nx))
    }

    #[test]
    fn empty_repository_is_not_a_migration() {
        let found = report(&[], None, None, None, None);
        assert!(found.detections.is_empty());
        assert!(found.errors.is_empty());
    }

    #[test]
    fn each_path_marker_maps_to_its_tool() {
        let cases = [
            ("knope.toml", ReleaseTool::Knope),
            (".changeset/config.json", ReleaseTool::Changesets),
            (".bumpy/_config.json", ReleaseTool::Bumpy),
            ("release-please-config.json", ReleaseTool::ReleasePlease),
            (".release-please-manifest.json", ReleaseTool::ReleasePlease),
            ("release-plz.toml", ReleaseTool::ReleasePlz),
            (".release-plz.toml", ReleaseTool::ReleasePlz),
            ("release.config.js", ReleaseTool::SemanticRelease),
            ("release.config.cjs", ReleaseTool::SemanticRelease),
            ("release.config.mjs", ReleaseTool::SemanticRelease),
            ("release.config.ts", ReleaseTool::SemanticRelease),
            (".releaserc", ReleaseTool::SemanticRelease),
            (".releaserc.json", ReleaseTool::SemanticRelease),
            (".releaserc.yaml", ReleaseTool::SemanticRelease),
            (".releaserc.yml", ReleaseTool::SemanticRelease),
            (".releaserc.js", ReleaseTool::SemanticRelease),
            (".releaserc.cjs", ReleaseTool::SemanticRelease),
            (".releaserc.mjs", ReleaseTool::SemanticRelease),
        ];
        for (path, tool) in cases {
            let found = report(&[path], None, None, None, None);
            assert_eq!(tools(&found.detections), vec![tool], "{path}");
            assert_eq!(found.detections[0].evidence(), path);
            assert!(found.errors.is_empty(), "{path}");
        }
    }

    #[test]
    fn releaserc_glob_is_root_only() {
        for name in [
            ".releaserc",
            ".releaserc.json",
            ".releaserc.yaml",
            ".releaserc.yml",
            ".releaserc.js",
            ".releaserc.cjs",
            ".releaserc.mjs",
            ".releaserc.ts",
        ] {
            assert!(is_releaserc_name(name), "{name}");
            let found = report(&[name], None, None, None, None);
            assert_eq!(found.detections[0].tool(), ReleaseTool::SemanticRelease);
            assert_eq!(found.detections[0].evidence(), name);
        }
        assert!(!is_releaserc_name("nested/.releaserc"));
        assert!(!is_release_config_name("nested/release.config.js"));
        for name in [
            "release.config.js",
            "release.config.cjs",
            "release.config.mjs",
            "release.config.ts",
        ] {
            assert!(is_release_config_name(name), "{name}");
            let found = report(&[name], None, None, None, None);
            assert_eq!(found.detections[0].tool(), ReleaseTool::SemanticRelease);
            assert_eq!(found.detections[0].evidence(), name);
        }
        let found = report(&[".releaserc.json"], None, None, None, None);
        assert_eq!(found.detections[0].tool(), ReleaseTool::SemanticRelease);
        assert_eq!(found.detections[0].evidence(), ".releaserc.json");
    }

    #[test]
    fn package_json_release_key_is_semantic_release() {
        let found = report(
            &["package.json"],
            None,
            None,
            Some("{\"release\":{}}\n"),
            None,
        );
        assert_eq!(found.detections[0].tool(), ReleaseTool::SemanticRelease);
        assert_eq!(found.detections[0].evidence(), "package.json");
    }

    #[test]
    fn nx_json_release_key_is_nx_release() {
        let found = report(&["nx.json"], None, None, None, Some("{\"release\":{}}\n"));
        assert_eq!(found.detections[0].tool(), ReleaseTool::NxRelease);
        assert_eq!(found.detections[0].evidence(), "nx.json");
    }

    #[test]
    fn cargo_workspace_metadata_underscore_and_hyphen() {
        let underscore = "[workspace]\n[workspace.metadata.release_plz]\n";
        let found = report(&["Cargo.toml"], None, Some(underscore), None, None);
        assert_eq!(found.detections[0].tool(), ReleaseTool::ReleasePlz);
        let hyphen = "[workspace.metadata.release-plz]\n";
        let found = report(&["Cargo.toml"], None, Some(hyphen), None, None);
        assert_eq!(found.detections[0].tool(), ReleaseTool::ReleasePlz);
    }

    #[test]
    fn package_metadata_release_plz_is_not_a_hit() {
        let body = "[package]\nname = \"x\"\nversion = \"0.1.0\"\n[package.metadata.release_plz]\n";
        let found = report(&["Cargo.toml"], None, Some(body), None, None);
        assert!(found.detections.is_empty());
        assert!(found.errors.is_empty());
    }

    #[test]
    fn plain_manifests_are_not_a_migration() {
        let cargo = "[package]\nname = \"x\"\nversion = \"0.1.0\"\n";
        assert!(report(&["Cargo.toml"], None, Some(cargo), None, None)
            .detections
            .is_empty());
        assert!(report(
            &["package.json"],
            None,
            None,
            Some("{\"name\":\"x\"}\n"),
            None
        )
        .detections
        .is_empty());
        assert!(report(
            &["nx.json"],
            None,
            None,
            None,
            Some("{\"$schema\":\"nx\"}\n")
        )
        .detections
        .is_empty());
    }

    #[test]
    fn orphan_bump_files_are_changesets_without_config() {
        let names = ["feat.md"];
        let found = report(&[".changeset/feat.md"], Some(&names), None, None, None);
        assert_eq!(found.detections[0].tool(), ReleaseTool::Changesets);
        assert_eq!(found.detections[0].evidence(), ".changeset/");
    }

    #[test]
    fn instruction_files_alone_are_not_a_migration() {
        let names = ["README.md", "AGENTS.md", "CLAUDE.md", "GEMINI.md"];
        let found = report(&[], Some(&names), None, None, None);
        assert!(found.detections.is_empty());
    }

    #[test]
    fn malformed_package_json_is_an_error_not_a_miss() {
        let found = report(&["package.json"], None, None, Some("{"), None);
        assert!(found.detections.is_empty());
        assert!(matches!(found.errors[0], DetectError::Json { .. }));
    }

    #[test]
    fn malformed_cargo_toml_is_an_error_not_a_miss() {
        let found = report(&["Cargo.toml"], None, Some("["), None, None);
        assert!(found.detections.is_empty());
        assert!(matches!(found.errors[0], DetectError::Toml { .. }));
    }

    #[test]
    fn malformed_nx_json_is_an_error_not_a_miss() {
        let found = report(&["nx.json"], None, None, None, Some("{"));
        assert!(found.detections.is_empty());
        assert!(matches!(found.errors[0], DetectError::Json { .. }));
    }

    #[test]
    fn knope_survives_malformed_package_json() {
        let found = report(&["knope.toml", "package.json"], None, None, Some("{"), None);
        assert_eq!(tools(&found.detections), vec![ReleaseTool::Knope]);
        assert!(matches!(found.errors[0], DetectError::Json { .. }));
    }

    #[test]
    fn two_release_please_files_are_one_row() {
        let found = report(
            &[
                "release-please-config.json",
                ".release-please-manifest.json",
            ],
            None,
            None,
            None,
            None,
        );
        assert_eq!(tools(&found.detections), vec![ReleaseTool::ReleasePlease]);
        assert_eq!(found.detections[0].evidence(), "release-please-config.json");
    }

    #[test]
    fn two_tools_are_both_reported_in_table_order() {
        let found = report(&["knope.toml", "release-plz.toml"], None, None, None, None);
        assert_eq!(
            tools(&found.detections),
            vec![ReleaseTool::Knope, ReleaseTool::ReleasePlz]
        );
    }
}
