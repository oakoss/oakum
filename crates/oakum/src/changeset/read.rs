//! Reading rules for `.changeset/*.md` (`okm-wnp`).
//!
//! Spec (`docs/specs/bump-files.md`): every `.md` directly in the directory is a
//! bump file except four instruction names; subdirectories and non-`.md` files
//! are not candidates; a malformed body is reported by path and skipped; an
//! unknown package name is an error naming the file and the name.
//!
//! This module does not touch the filesystem. Callers list the directory and
//! pass `(file_name, body)` pairs — keeping ADR-0002's I/O marker count at one
//! (`discover`) until a command boundary owns the path.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::plan::{BumpFile, PackageId, Workspace};

use super::format::{parse, ParseError};

/// Exact-match instruction files `@changesets/read` v3 skips (case-sensitive).
const EXACT_SKIP: &[&str] = &["AGENTS.md", "CLAUDE.md", "GEMINI.md"];

/// True when `file_name` is one of the four instruction names that are not bump
/// files: `README.md` case-insensitively, and `AGENTS.md` / `CLAUDE.md` /
/// `GEMINI.md` by exact string equality.
#[must_use]
pub fn skipped_instruction_name(file_name: &str) -> bool {
    if file_name.eq_ignore_ascii_case("README.md") {
        return true;
    }
    EXACT_SKIP.contains(&file_name)
}

/// True when `file_name` is a direct `.changeset/` candidate: a bare `.md`
/// name (case-sensitive suffix, no path separators) that is not an instruction skip.
#[must_use]
pub fn is_bump_file_name(file_name: &str) -> bool {
    if file_name.contains('/') || file_name.contains('\\') {
        return false;
    }
    // `@changesets/read` and knope both use case-sensitive `.md` checks.
    #[expect(
        clippy::case_sensitive_file_extension_comparisons,
        reason = "foreign readers match `.md` case-sensitively; `.MD` must not be a bump file"
    )]
    let is_md = file_name.ends_with(".md");
    is_md && !skipped_instruction_name(file_name)
}

/// Why a bump-file body could not become a planner [`BumpFile`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    Malformed(MalformedBumpFile),
    UnknownPackage(UnknownPackage),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(err) => write!(f, "{err}"),
            Self::UnknownPackage(err) => write!(f, "{err}"),
        }
    }
}

impl core::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Malformed(err) => Some(&err.error),
            Self::UnknownPackage(err) => Some(err),
        }
    }
}

/// Why a frontmatter key did not resolve to exactly one workspace package.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnknownReason {
    /// No package with this name.
    Missing,
    /// More than one package shares the name (typically across ecosystems).
    Ambiguous,
}

/// A frontmatter key that is not exactly one package in the workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownPackage {
    pub file: String,
    pub name: String,
    pub reason: UnknownReason,
}

impl fmt::Display for UnknownPackage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.reason {
            UnknownReason::Missing => write!(
                f,
                "bump file `{}` names package `{}`, which is not in the workspace",
                self.file, self.name
            ),
            UnknownReason::Ambiguous => write!(
                f,
                "bump file `{}` names package `{}`, which matches more than one workspace package",
                self.file, self.name
            ),
        }
    }
}

impl core::error::Error for UnknownPackage {}

/// Hard failure from [`load_bump_files`]: an unknown package, plus every
/// malformed candidate seen in the same pass (so soft failures are not dropped).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadAbort {
    pub unknown: UnknownPackage,
    pub malformed: Vec<MalformedBumpFile>,
}

impl fmt::Display for LoadAbort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.unknown)?;
        for report in &self.malformed {
            write!(f, "; also {report}")?;
        }
        Ok(())
    }
}

impl core::error::Error for LoadAbort {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        Some(&self.unknown)
    }
}

/// A candidate whose body failed the intersection grammar; the run continues.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MalformedBumpFile {
    pub file: String,
    pub error: ParseError,
}

impl fmt::Display for MalformedBumpFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bump file `{}`: {}", self.file, self.error)
    }
}

/// Result of loading every bump-file candidate from already-read bodies.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use]
pub struct LoadedBumpFiles {
    pub files: Vec<BumpFile>,
    pub malformed: Vec<MalformedBumpFile>,
}

/// Parse `body`, resolve package names against `workspace`, and build a
/// [`BumpFile`] whose `id` is `file_name`.
///
/// # Errors
///
/// [`LoadError::Malformed`] when the body is outside the intersection grammar
/// (always includes `file_name`). [`LoadError::UnknownPackage`] when a key is
/// missing from the workspace or matches more than one package.
pub fn resolve_bump_file(
    file_name: impl Into<String>,
    body: &str,
    workspace: &Workspace,
) -> Result<BumpFile, LoadError> {
    let file_name = file_name.into();
    let change = parse(body).map_err(|error| {
        LoadError::Malformed(MalformedBumpFile {
            file: file_name.clone(),
            error,
        })
    })?;
    let mut entries = Vec::with_capacity(change.entries().len());
    for (name, level) in change.entries() {
        let id = match resolve_package_name(name, workspace) {
            Ok(id) => id,
            Err(reason) => {
                return Err(LoadError::UnknownPackage(UnknownPackage {
                    file: file_name,
                    name: String::from(name),
                    reason,
                }));
            }
        };
        entries.push((id, *level));
    }
    Ok(BumpFile {
        id: file_name,
        entries,
        note: String::from(change.note()),
    })
}

/// Apply reading rules to `(file_name, body)` pairs already loaded from disk.
///
/// Non-candidates (wrong extension, nested paths, instruction skips) are
/// ignored. Malformed bodies are collected and skipped. Unknown package names
/// abort the load after the full pass, retaining every malformed report.
///
/// # Errors
///
/// Returns [`LoadAbort`] when any candidate names a package absent from or
/// ambiguous in `workspace`.
pub fn load_bump_files<'a>(
    files: impl IntoIterator<Item = (&'a str, &'a str)>,
    workspace: &Workspace,
) -> Result<LoadedBumpFiles, LoadAbort> {
    let mut loaded = Vec::new();
    let mut malformed = Vec::new();
    let mut unknown = None;
    for (file_name, body) in files {
        if !is_bump_file_name(file_name) {
            continue;
        }
        match resolve_bump_file(file_name, body, workspace) {
            Ok(file) => {
                if unknown.is_none() {
                    loaded.push(file);
                }
            }
            Err(LoadError::Malformed(report)) => malformed.push(report),
            Err(LoadError::UnknownPackage(err)) => {
                if unknown.is_none() {
                    unknown = Some(err);
                }
            }
        }
    }
    if let Some(unknown) = unknown {
        return Err(LoadAbort { unknown, malformed });
    }
    Ok(LoadedBumpFiles {
        files: loaded,
        malformed,
    })
}

/// Resolve a frontmatter name to exactly one [`PackageId`].
///
/// # Errors
///
/// [`UnknownReason::Missing`] when no package has that name;
/// [`UnknownReason::Ambiguous`] when more than one does.
pub fn resolve_package_name(name: &str, workspace: &Workspace) -> Result<PackageId, UnknownReason> {
    let mut matches = workspace
        .packages()
        .filter(|pkg| pkg.id().name == name)
        .map(|pkg| pkg.id().clone());
    let Some(first) = matches.next() else {
        return Err(UnknownReason::Missing);
    };
    match matches.next() {
        Some(_) => Err(UnknownReason::Ambiguous),
        None => Ok(first),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{
        BumpLevel, Ecosystem, Package, PackageId, ResolvesDependenciesAt, Workspace,
    };
    use alloc::string::ToString;
    use alloc::vec;
    use semver::Version;

    fn cargo_pkg(name: &str) -> Package {
        Package::new(
            PackageId::new(Ecosystem::Cargo, name),
            Version::new(0, 1, 0),
            ResolvesDependenciesAt::Install,
            true,
            vec![],
        )
    }

    fn workspace(packages: Vec<Package>) -> Workspace {
        Workspace::new(packages).expect("workspace")
    }

    #[test]
    fn skip_list_matches_changesets_read_v3_asymmetry() {
        assert!(skipped_instruction_name("README.md"));
        assert!(skipped_instruction_name("readme.md"));
        assert!(skipped_instruction_name("ReadMe.MD"));
        assert!(skipped_instruction_name("AGENTS.md"));
        assert!(skipped_instruction_name("CLAUDE.md"));
        assert!(skipped_instruction_name("GEMINI.md"));
        assert!(!skipped_instruction_name("agents.md"));
        assert!(!skipped_instruction_name("claude.md"));
        assert!(!skipped_instruction_name("Claude.md"));
        assert!(!skipped_instruction_name("AGENTS.MD"));
        assert!(!skipped_instruction_name("gemini.md"));
        assert!(!skipped_instruction_name("change.md"));
        assert!(!is_bump_file_name("AGENTS.md"));
        assert!(is_bump_file_name("Claude.md"));
    }

    #[test]
    fn bump_file_name_requires_md_suffix_case_sensitive() {
        assert!(is_bump_file_name("feat.md"));
        assert!(!is_bump_file_name("feat.MD"));
        assert!(!is_bump_file_name("feat.Md"));
        assert!(!is_bump_file_name("_config.toml"));
        assert!(!is_bump_file_name("README.md"));
        assert!(!is_bump_file_name("nested/change.md"));
        assert!(!is_bump_file_name("nested\\change.md"));
        assert!(is_bump_file_name("agents.md"));
    }

    #[test]
    fn resolve_maps_names_and_rejects_unknown() {
        let ws = workspace(vec![cargo_pkg("core"), cargo_pkg("utils")]);
        let body = "---\ncore: minor\nutils: patch\n---\nnote\n";
        let file = resolve_bump_file("change.md", body, &ws).expect("resolve");
        assert_eq!(file.id, "change.md");
        assert_eq!(file.note, "note\n");
        assert_eq!(
            file.entries,
            vec![
                (PackageId::new(Ecosystem::Cargo, "core"), BumpLevel::Minor),
                (PackageId::new(Ecosystem::Cargo, "utils"), BumpLevel::Patch),
            ]
        );

        let err = resolve_bump_file("bad.md", "---\nmissing: patch\n---\n", &ws).unwrap_err();
        assert_eq!(
            err,
            LoadError::UnknownPackage(UnknownPackage {
                file: "bad.md".to_string(),
                name: "missing".to_string(),
                reason: UnknownReason::Missing,
            })
        );
    }

    #[test]
    fn resolve_reports_first_unknown_name_in_file() {
        let ws = workspace(vec![cargo_pkg("core")]);
        let err =
            resolve_bump_file("x.md", "---\ncore: patch\nmissing: minor\n---\n", &ws).unwrap_err();
        assert_eq!(
            err,
            LoadError::UnknownPackage(UnknownPackage {
                file: "x.md".to_string(),
                name: "missing".to_string(),
                reason: UnknownReason::Missing,
            })
        );
    }

    #[test]
    fn resolve_surfaces_malformed_with_file_name() {
        let ws = workspace(vec![cargo_pkg("core")]);
        let err = resolve_bump_file("b.md", "preamble\n---\ncore: patch\n---\n", &ws).unwrap_err();
        match err {
            LoadError::Malformed(report) => {
                assert_eq!(report.file, "b.md");
                assert!(report.to_string().contains("b.md"));
            }
            LoadError::UnknownPackage(_) => panic!("expected malformed"),
        }
    }

    #[test]
    fn load_empty_or_all_skipped_is_ok() {
        let ws = workspace(vec![cargo_pkg("core")]);
        let empty = load_bump_files([], &ws).expect("empty");
        assert!(empty.files.is_empty() && empty.malformed.is_empty());
        let skipped =
            load_bump_files([("README.md", "---\ncore: patch\n---\n")], &ws).expect("skip");
        assert!(skipped.files.is_empty() && skipped.malformed.is_empty());
    }

    #[test]
    fn load_skips_instruction_and_non_md_and_continues_past_malformed() {
        let ws = workspace(vec![cargo_pkg("core")]);
        let good = "---\ncore: patch\n---\n";
        let bad = "preamble\n---\ncore: patch\n---\n";
        let loaded = load_bump_files(
            [
                ("README.md", good),
                ("AGENTS.md", good),
                ("CLAUDE.md", good),
                ("GEMINI.md", good),
                ("_config.toml", good),
                ("nested/x.md", good),
                ("broken.md", bad),
                ("ok.md", good),
            ],
            &ws,
        )
        .expect("load");
        assert_eq!(loaded.files.len(), 1);
        assert_eq!(loaded.files[0].id, "ok.md");
        assert_eq!(loaded.malformed.len(), 1);
        assert_eq!(loaded.malformed[0].file, "broken.md");
    }

    #[test]
    fn load_aborts_on_unknown_package() {
        let ws = workspace(vec![cargo_pkg("core")]);
        let err = load_bump_files([("x.md", "---\nnope: patch\n---\n")], &ws).unwrap_err();
        assert_eq!(err.unknown.name, "nope");
        assert_eq!(err.unknown.file, "x.md");
        assert_eq!(err.unknown.reason, UnknownReason::Missing);
        assert!(err.malformed.is_empty());
    }

    #[test]
    fn load_aborts_on_unknown_even_after_a_good_file_and_keeps_malformed() {
        let ws = workspace(vec![cargo_pkg("core")]);
        let err = load_bump_files(
            [
                ("ok.md", "---\ncore: patch\n---\n"),
                ("broken.md", "preamble\n---\ncore: patch\n---\n"),
                ("bad.md", "---\nnope: patch\n---\n"),
            ],
            &ws,
        )
        .unwrap_err();
        assert_eq!(err.unknown.file, "bad.md");
        assert_eq!(err.malformed.len(), 1);
        assert_eq!(err.malformed[0].file, "broken.md");
        let shown = err.to_string();
        assert!(shown.contains("bad.md") && shown.contains("broken.md"));
    }

    #[test]
    fn unknown_package_display_names_file_and_package() {
        let err = UnknownPackage {
            file: "x.md".into(),
            name: "nope".into(),
            reason: UnknownReason::Missing,
        };
        let s = err.to_string();
        assert!(s.contains("x.md") && s.contains("nope"));
    }

    #[test]
    fn ambiguous_name_across_ecosystems_is_unknown() {
        let ws = workspace(vec![
            cargo_pkg("core"),
            Package::new(
                PackageId::new(Ecosystem::Npm, "core"),
                Version::new(1, 0, 0),
                ResolvesDependenciesAt::Install,
                true,
                vec![],
            ),
        ]);
        let err = resolve_bump_file("x.md", "---\ncore: patch\n---\n", &ws).unwrap_err();
        assert_eq!(
            err,
            LoadError::UnknownPackage(UnknownPackage {
                file: "x.md".to_string(),
                name: "core".to_string(),
                reason: UnknownReason::Ambiguous,
            })
        );
    }
}
