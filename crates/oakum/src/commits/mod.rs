//! Map conventional (and plain) commit messages to package bump intent.
//!
//! Pure: no git, no filesystem. The CLI gathers commit text; this module decides
//! bump levels and aggregates highest-wins per package ([ADR-0029] /
//! `okm-j1r`). Case-insensitive type compares come from `git-conventional` 1.1.0.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use git_conventional::{Commit, Type};

use crate::changeset::{resolve_package_name, UnknownReason};
use crate::plan::{BumpFile, BumpLevel, Workspace};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitContribution {
    package: String,
    level: BumpLevel,
    summary: String,
}

impl CommitContribution {
    /// Returns `None` when `package` is empty or `level` is not a release level.
    #[must_use]
    pub fn new(package: String, level: BumpLevel, summary: String) -> Option<Self> {
        if package.is_empty() || !level.is_release() {
            return None;
        }
        Some(Self {
            package,
            level,
            summary,
        })
    }

    #[must_use]
    pub fn package(&self) -> &str {
        &self.package
    }

    #[must_use]
    pub fn level(&self) -> BumpLevel {
        self.level
    }

    #[must_use]
    pub fn summary(&self) -> &str {
        &self.summary
    }
}

/// Highest-wins aggregation across many contributions, plus a changelog note body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AggregatedIntent {
    entries: Vec<(String, BumpLevel)>,
    note: String,
}

impl AggregatedIntent {
    #[must_use]
    pub fn entries(&self) -> &[(String, BumpLevel)] {
        &self.entries
    }

    #[must_use]
    pub fn note(&self) -> &str {
        &self.note
    }
}

/// Result of mapping one commit message before optional path fallback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessageIntent {
    Contributions(Vec<CommitContribution>),
    /// No usable scope (or non-conventional): caller maps paths at this level.
    PathFallback {
        level: BumpLevel,
        summary: String,
    },
}

impl MessageIntent {
    fn path_fallback(level: BumpLevel, summary: String) -> Self {
        debug_assert!(level.is_release());
        Self::PathFallback {
            level: if level.is_release() {
                level
            } else {
                BumpLevel::Patch
            },
            summary,
        }
    }

    fn contributions(contrib: CommitContribution) -> Self {
        Self::Contributions(Vec::from([contrib]))
    }
}

/// Ambiguous scopes error. Missing/unknown scopes and non-conventional messages
/// become [`MessageIntent::PathFallback`] so the CLI can attribute by path.
///
/// # Errors
///
/// Returns an error when the conventional scope matches more than one workspace package.
pub fn message_intent(message: &str, workspace: &Workspace) -> Result<MessageIntent, String> {
    let Ok(commit) = Commit::parse(message.trim()) else {
        return Ok(MessageIntent::path_fallback(
            BumpLevel::Patch,
            first_line(message),
        ));
    };
    let level = bump_level_for(&commit);
    let summary = String::from(commit.description());
    let Some(scope) = commit.scope() else {
        return Ok(MessageIntent::path_fallback(level, summary));
    };
    match resolve_package_name(scope.as_str(), workspace) {
        Ok(id) => {
            let Some(contrib) = CommitContribution::new(id.name.clone(), level, summary) else {
                return Ok(MessageIntent::path_fallback(
                    BumpLevel::Patch,
                    String::from(commit.description()),
                ));
            };
            Ok(MessageIntent::contributions(contrib))
        }
        Err(UnknownReason::Missing) => Ok(MessageIntent::path_fallback(level, summary)),
        Err(UnknownReason::Ambiguous) => Err(format!(
            "commit scope `{scope}` matches more than one workspace package"
        )),
    }
}

/// Path-fallback cases yield an empty list; prefer [`message_intent`] when that
/// level must be preserved.
///
/// # Errors
///
/// Returns an error when the conventional scope matches more than one workspace package.
pub fn contributions_from_message(
    message: &str,
    workspace: &Workspace,
) -> Result<Vec<CommitContribution>, String> {
    match message_intent(message, workspace)? {
        MessageIntent::Contributions(c) => Ok(c),
        MessageIntent::PathFallback { .. } => Ok(Vec::new()),
    }
}

/// When a conventional commit has no usable scope (or is not conventional), map
/// `level` onto packages whose directory is the longest prefix of a changed path.
///
/// When multiple packages share that longest prefix (co-located Cargo/npm), every
/// tied package is attributed. `package_dirs` are repository-relative directory
/// prefixes (no trailing slash); empty string for the repository-root package.
#[must_use]
pub fn contributions_from_paths(
    changed_files: &[String],
    package_dirs: &[(String, String)],
    level: BumpLevel,
    summary: &str,
) -> Vec<CommitContribution> {
    if !level.is_release() {
        return Vec::new();
    }
    let mut out_levels: BTreeMap<String, BumpLevel> = BTreeMap::new();
    for file in changed_files {
        let file = file.trim_start_matches("./");
        let mut best_len: Option<usize> = None;
        for (_, dir) in package_dirs {
            if !package_contains(dir, file) {
                continue;
            }
            best_len = Some(best_len.map_or(dir.len(), |n| n.max(dir.len())));
        }
        let Some(best_len) = best_len else {
            continue;
        };
        for (name, dir) in package_dirs {
            if dir.len() != best_len || !package_contains(dir, file) {
                continue;
            }
            let entry = out_levels.entry(name.clone()).or_insert(BumpLevel::None);
            if level > *entry {
                *entry = level;
            }
        }
    }
    out_levels
        .into_iter()
        .filter_map(|(package, lvl)| CommitContribution::new(package, lvl, String::from(summary)))
        .collect()
}

#[must_use]
pub fn aggregate(contributions: &[CommitContribution]) -> AggregatedIntent {
    let mut levels: BTreeMap<String, BumpLevel> = BTreeMap::new();
    let mut notes: Vec<String> = Vec::new();
    for c in contributions {
        if !c.level.is_release() {
            continue;
        }
        let entry = levels.entry(c.package.clone()).or_insert(BumpLevel::None);
        if c.level > *entry {
            *entry = c.level;
        }
        notes.push(format!("- {}: {}", c.package, c.summary));
    }
    let entries: Vec<(String, BumpLevel)> = levels
        .into_iter()
        .filter(|(_, level)| level.is_release())
        .collect();
    AggregatedIntent {
        entries,
        note: notes.join("\n"),
    }
}

/// Planner [`BumpFile`] from aggregated commit intent (commits-only plan).
///
/// # Errors
///
/// When a package name is missing from or ambiguous in `workspace`.
pub fn to_bump_file(
    intent: &AggregatedIntent,
    workspace: &Workspace,
    id: String,
) -> Result<BumpFile, String> {
    let mut entries = Vec::with_capacity(intent.entries().len());
    for (name, level) in intent.entries() {
        let package_id = resolve_package_name(name, workspace).map_err(|reason| match reason {
            UnknownReason::Missing => {
                format!("commit-derived package `{name}` is not in the workspace")
            }
            UnknownReason::Ambiguous => {
                format!("commit-derived package `{name}` matches more than one workspace package")
            }
        })?;
        entries.push((package_id, *level));
    }
    Ok(BumpFile {
        id,
        entries,
        note: String::from(intent.note()),
    })
}

#[must_use]
pub fn bump_level_for(commit: &Commit<'_>) -> BumpLevel {
    if commit.breaking() {
        return BumpLevel::Major;
    }
    if commit.type_() == Type::FEAT {
        BumpLevel::Minor
    } else {
        BumpLevel::Patch
    }
}

fn first_line(message: &str) -> String {
    String::from(message.lines().next().unwrap_or("").trim())
}

fn package_contains(package_dir: &str, file: &str) -> bool {
    if package_dir.is_empty() {
        // Root package owns paths that are not under a nested package directory;
        // longest-prefix selection in [`contributions_from_paths`] prefers longer dirs.
        return true;
    }
    file == package_dir || file.starts_with(&format!("{package_dir}/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Ecosystem, Package, PackageId, ResolvesDependenciesAt, Workspace};
    use semver::Version;

    fn workspace(names: &[&str]) -> Workspace {
        let packages: Vec<Package> = names
            .iter()
            .map(|name| {
                Package::new(
                    PackageId::new(Ecosystem::Cargo, *name),
                    Version::new(0, 1, 0),
                    ResolvesDependenciesAt::Install,
                    true,
                    Vec::new(),
                )
            })
            .collect();
        Workspace::new(packages).expect("workspace")
    }

    #[test]
    fn feat_scope_is_minor() {
        let ws = workspace(&["demo"]);
        let got = contributions_from_message("feat(demo): add thing\n", &ws).expect("ok");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].level(), BumpLevel::Minor);
        assert_eq!(got[0].package(), "demo");
    }

    #[test]
    fn breaking_is_major() {
        let ws = workspace(&["demo"]);
        let got = contributions_from_message("feat(demo)!: break it\n", &ws).expect("ok");
        assert_eq!(got[0].level(), BumpLevel::Major);
    }

    #[test]
    fn breaking_change_footer_is_major() {
        let ws = workspace(&["demo"]);
        let msg = "feat(demo): x\n\nBREAKING CHANGE: api\n";
        let got = contributions_from_message(msg, &ws).expect("ok");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].level(), BumpLevel::Major);
    }

    #[test]
    fn fix_is_patch() {
        let ws = workspace(&["demo"]);
        let got = contributions_from_message("fix(demo): bug\n", &ws).expect("ok");
        assert_eq!(got[0].level(), BumpLevel::Patch);
    }

    #[test]
    fn feat_case_insensitive() {
        let ws = workspace(&["demo"]);
        let got = contributions_from_message("Feat(demo): caps\n", &ws).expect("ok");
        assert_eq!(got[0].level(), BumpLevel::Minor);
    }

    #[test]
    fn unknown_scope_is_path_fallback_with_level() {
        let ws = workspace(&["demo"]);
        let intent = message_intent("feat(other): x\n", &ws).expect("ok");
        match intent {
            MessageIntent::PathFallback { level, .. } => assert_eq!(level, BumpLevel::Minor),
            MessageIntent::Contributions(_) => panic!("expected path fallback"),
        }
        assert!(contributions_from_message("feat(other): x\n", &ws)
            .expect("ok")
            .is_empty());
    }

    #[test]
    fn ambiguous_scope_is_error() {
        let packages = Vec::from([
            Package::new(
                PackageId::new(Ecosystem::Cargo, "shared"),
                Version::new(0, 1, 0),
                ResolvesDependenciesAt::Install,
                true,
                Vec::new(),
            ),
            Package::new(
                PackageId::new(Ecosystem::Npm, "shared"),
                Version::new(1, 0, 0),
                ResolvesDependenciesAt::Install,
                true,
                Vec::new(),
            ),
        ]);
        let ws = Workspace::new(packages).expect("workspace");
        let err = message_intent("feat(shared): x\n", &ws).expect_err("ambiguous");
        assert!(err.contains("more than one"), "{err}");
        let err = contributions_from_message("feat(shared): x\n", &ws).expect_err("ambiguous");
        assert!(err.contains("more than one"), "{err}");
    }

    #[test]
    fn unscoped_conventional_is_path_fallback_with_level() {
        let ws = workspace(&["demo"]);
        let intent = message_intent("feat: no scope\n", &ws).expect("ok");
        match intent {
            MessageIntent::PathFallback { level, .. } => assert_eq!(level, BumpLevel::Minor),
            MessageIntent::Contributions(_) => panic!("expected path fallback"),
        }
        assert!(contributions_from_message("feat: no scope\n", &ws)
            .expect("ok")
            .is_empty());
    }

    #[test]
    fn unscoped_breaking_is_path_fallback_major() {
        let ws = workspace(&["demo"]);
        let intent = message_intent("feat!: break\n", &ws).expect("ok");
        match intent {
            MessageIntent::PathFallback { level, .. } => assert_eq!(level, BumpLevel::Major),
            MessageIntent::Contributions(_) => panic!("expected path fallback"),
        }
    }

    #[test]
    fn aggregate_highest_wins() {
        let contribs = [
            CommitContribution::new(String::from("demo"), BumpLevel::Patch, String::from("a"))
                .expect("contrib"),
            CommitContribution::new(String::from("demo"), BumpLevel::Minor, String::from("b"))
                .expect("contrib"),
        ];
        let agg = aggregate(&contribs);
        assert_eq!(agg.entries(), &[(String::from("demo"), BumpLevel::Minor)]);
        assert!(agg.note().contains("demo: a"));
        assert!(agg.note().contains("demo: b"));
    }

    #[test]
    fn paths_map_to_package_dir() {
        let dirs = [(String::from("core"), String::from("crates/core"))];
        let files = [String::from("crates/core/src/lib.rs")];
        let got = contributions_from_paths(&files, &dirs, BumpLevel::Patch, "touch");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].package(), "core");
    }

    #[test]
    fn nested_package_wins_longest_prefix() {
        let dirs = [
            (String::from("app"), String::from("crates/app")),
            (String::from("app-core"), String::from("crates/app/core")),
        ];
        let files = [String::from("crates/app/core/src/lib.rs")];
        let got = contributions_from_paths(&files, &dirs, BumpLevel::Minor, "nested");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].package(), "app-core");
        assert_eq!(got[0].level(), BumpLevel::Minor);
    }

    #[test]
    fn root_package_does_not_steal_nested_paths() {
        let dirs = [
            (String::from("root"), String::new()),
            (String::from("core"), String::from("crates/core")),
        ];
        let files = [String::from("crates/core/src/lib.rs")];
        let got = contributions_from_paths(&files, &dirs, BumpLevel::Patch, "nested");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].package(), "core");
    }

    #[test]
    fn colocated_packages_at_same_prefix_both_attribute() {
        let dirs = [
            (String::from("oakum"), String::from("crates/oakum")),
            (String::from("@oakoss/oakum"), String::from("crates/oakum")),
        ];
        let files = [String::from("crates/oakum/src/lib.rs")];
        let got = contributions_from_paths(&files, &dirs, BumpLevel::Minor, "polyglot");
        let mut names: Vec<&str> = got.iter().map(CommitContribution::package).collect();
        names.sort_unstable();
        assert_eq!(names, vec!["@oakoss/oakum", "oakum"]);
        assert!(got.iter().all(|c| c.level() == BumpLevel::Minor));
    }

    #[test]
    fn to_bump_file_resolves_package_ids() {
        let ws = workspace(&["demo"]);
        let intent = aggregate(&[CommitContribution::new(
            String::from("demo"),
            BumpLevel::Minor,
            String::from("add thing"),
        )
        .expect("contrib")]);
        let file = to_bump_file(&intent, &ws, String::from("commits")).expect("bump file");
        assert_eq!(file.id, "commits");
        assert_eq!(file.entries.len(), 1);
        assert_eq!(file.entries[0].0.name, "demo");
        assert_eq!(file.entries[0].1, BumpLevel::Minor);
        assert!(file.note.contains("demo: add thing"));
    }

    #[test]
    fn to_bump_file_missing_package_errors() {
        let ws = workspace(&["demo"]);
        let intent = aggregate(&[CommitContribution::new(
            String::from("ghost"),
            BumpLevel::Patch,
            String::from("x"),
        )
        .expect("contrib")]);
        let err = to_bump_file(&intent, &ws, String::from("commits")).expect_err("missing");
        assert!(err.contains("not in the workspace"), "{err}");
    }

    #[test]
    fn to_bump_file_ambiguous_package_errors() {
        let packages = Vec::from([
            Package::new(
                PackageId::new(Ecosystem::Cargo, "shared"),
                Version::new(0, 1, 0),
                ResolvesDependenciesAt::Install,
                true,
                Vec::new(),
            ),
            Package::new(
                PackageId::new(Ecosystem::Npm, "shared"),
                Version::new(1, 0, 0),
                ResolvesDependenciesAt::Install,
                true,
                Vec::new(),
            ),
        ]);
        let ws = Workspace::new(packages).expect("workspace");
        let intent = aggregate(&[CommitContribution::new(
            String::from("shared"),
            BumpLevel::Minor,
            String::from("x"),
        )
        .expect("contrib")]);
        let err = to_bump_file(&intent, &ws, String::from("commits")).expect_err("ambiguous");
        assert!(err.contains("more than one"), "{err}");
    }
}
