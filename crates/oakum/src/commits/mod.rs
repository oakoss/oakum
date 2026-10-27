//! Map conventional (and plain) commit messages to package bump intent.
//!
//! Pure: no git, no filesystem. The CLI gathers commit text; this module decides
//! bump levels and which packages each commit names ([ADR-0029] / `okm-j1r`).
//! Case-insensitive type compares come from `git-conventional` 1.1.0.

use alloc::collections::BTreeSet;
use alloc::string::String;

use git_conventional::{Commit, Type};

use crate::changeset::{resolve_package_name, UnknownReason};
use crate::plan::{BumpFile, BumpLevel, PackageId, Workspace};

/// What one commit asks for: a level, the packages it names, and the summary
/// each of their changelogs carries. [`BumpLevel::None`] covers the packages
/// without releasing them, as a `name: none` bump file does (ADR-0028).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitIntent {
    packages: BTreeSet<PackageId>,
    level: BumpLevel,
    summary: String,
}

impl CommitIntent {
    /// Returns `None` when `packages` is empty.
    #[must_use]
    pub fn new(packages: BTreeSet<PackageId>, level: BumpLevel, summary: String) -> Option<Self> {
        if packages.is_empty() {
            return None;
        }
        Some(Self {
            packages,
            level,
            summary,
        })
    }

    #[must_use]
    pub const fn packages(&self) -> &BTreeSet<PackageId> {
        &self.packages
    }

    #[must_use]
    pub const fn level(&self) -> BumpLevel {
        self.level
    }

    #[must_use]
    pub fn summary(&self) -> &str {
        &self.summary
    }

    /// The plan input for this commit alone, so its summary reaches only the
    /// changelogs of the packages it names.
    #[must_use]
    pub fn bump_file(&self, id: String) -> BumpFile {
        BumpFile {
            id,
            entries: self
                .packages
                .iter()
                .map(|package| (package.clone(), self.level))
                .collect(),
            note: format!("- {}", self.summary),
        }
    }
}

/// Result of mapping one commit message before optional path fallback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessageIntent {
    Scoped {
        package: PackageId,
        level: BumpLevel,
        summary: String,
    },
    /// No usable scope (or non-conventional): caller maps paths at this level.
    PathFallback {
        level: BumpLevel,
        summary: String,
        /// A scope that named no single package, for the caller to report
        /// when the paths name none either.
        unresolved_scope: Option<String>,
    },
}

/// Missing/unknown scopes and non-conventional messages become
/// [`MessageIntent::PathFallback`] so the CLI can attribute by path, as does an
/// ambiguous scope on a commit that releases nothing.
///
/// # Errors
///
/// Returns an error when the scope of a releasing commit matches more than one
/// workspace package: which ecosystem releases is not something to guess.
pub fn message_intent(message: &str, workspace: &Workspace) -> Result<MessageIntent, String> {
    let Ok(commit) = Commit::parse(message.trim()) else {
        return Ok(MessageIntent::PathFallback {
            level: BumpLevel::Patch,
            summary: first_line(message),
            unresolved_scope: None,
        });
    };
    let level = bump_level_for(&commit);
    let summary = String::from(commit.description());
    let Some(scope) = commit.scope() else {
        return Ok(MessageIntent::PathFallback {
            level,
            summary,
            unresolved_scope: None,
        });
    };
    match resolve_package_name(scope.as_str(), workspace) {
        Ok(package) => Ok(MessageIntent::Scoped {
            package,
            level,
            summary,
        }),
        Err(UnknownReason::Ambiguous) if level.is_release() => Err(format!(
            "commit scope `{scope}` matches more than one workspace package"
        )),
        Err(UnknownReason::Missing | UnknownReason::Ambiguous) => Ok(MessageIntent::PathFallback {
            level,
            summary,
            unresolved_scope: Some(String::from(scope.as_str())),
        }),
    }
}

/// Packages whose directory is the longest prefix of a changed path. Packages
/// tied at that prefix (a Cargo crate and an npm package in one directory) are
/// all attributed. `package_dirs` are repository-relative directory prefixes
/// (no trailing slash); empty string for the repository-root package.
#[must_use]
pub fn packages_for_paths(
    changed_files: &[String],
    package_dirs: &[(PackageId, String)],
) -> BTreeSet<PackageId> {
    let mut ids = BTreeSet::new();
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
        for (id, dir) in package_dirs {
            if dir.len() == best_len && package_contains(dir, file) {
                ids.insert(id.clone());
            }
        }
    }
    ids
}

/// `feat`, `fix` and a breaking change release; every other type releases
/// nothing, as knope and release-please read the convention.
#[must_use]
pub fn bump_level_for(commit: &Commit<'_>) -> BumpLevel {
    if commit.breaking() {
        BumpLevel::Major
    } else if commit.type_() == Type::FEAT {
        BumpLevel::Minor
    } else if commit.type_() == Type::FIX {
        BumpLevel::Patch
    } else {
        BumpLevel::None
    }
}

fn first_line(message: &str) -> String {
    String::from(message.lines().next().unwrap_or("").trim())
}

fn package_contains(package_dir: &str, file: &str) -> bool {
    if package_dir.is_empty() {
        // Root package owns paths that are not under a nested package directory;
        // longest-prefix selection in [`packages_for_paths`] prefers longer dirs.
        return true;
    }
    file == package_dir || file.starts_with(&format!("{package_dir}/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Ecosystem, Package, PackageId, ResolvesDependenciesAt, Workspace};
    use alloc::vec::Vec;
    use semver::Version;

    fn cargo(name: &str) -> PackageId {
        PackageId::new(Ecosystem::Cargo, name)
    }

    fn workspace(names: &[&str]) -> Workspace {
        let packages: Vec<Package> = names
            .iter()
            .map(|name| {
                Package::new(
                    cargo(name),
                    Version::new(0, 1, 0),
                    ResolvesDependenciesAt::Install,
                    true,
                    Vec::new(),
                )
            })
            .collect();
        Workspace::new(packages).expect("workspace")
    }

    fn scoped(message: &str, ws: &Workspace) -> (PackageId, BumpLevel) {
        match message_intent(message, ws).expect("ok") {
            MessageIntent::Scoped { package, level, .. } => (package, level),
            MessageIntent::PathFallback { .. } => panic!("expected a scoped intent"),
        }
    }

    fn fallback_level(message: &str, ws: &Workspace) -> BumpLevel {
        match message_intent(message, ws).expect("ok") {
            MessageIntent::PathFallback { level, .. } => level,
            MessageIntent::Scoped { .. } => panic!("expected path fallback"),
        }
    }

    #[test]
    fn feat_scope_is_minor() {
        let ws = workspace(&["demo"]);
        assert_eq!(
            scoped("feat(demo): add thing\n", &ws),
            (cargo("demo"), BumpLevel::Minor)
        );
    }

    #[test]
    fn breaking_is_major() {
        let ws = workspace(&["demo"]);
        assert_eq!(scoped("feat(demo)!: break it\n", &ws).1, BumpLevel::Major);
    }

    #[test]
    fn breaking_change_footer_is_major() {
        let ws = workspace(&["demo"]);
        let msg = "feat(demo): x\n\nBREAKING CHANGE: api\n";
        assert_eq!(scoped(msg, &ws).1, BumpLevel::Major);
    }

    #[test]
    fn fix_is_patch() {
        let ws = workspace(&["demo"]);
        assert_eq!(scoped("fix(demo): bug\n", &ws).1, BumpLevel::Patch);
    }

    #[test]
    fn only_feat_fix_and_breaking_release() {
        let ws = workspace(&["demo"]);
        for message in [
            "docs(demo): x",
            "chore(demo): x",
            "ci: x",
            "refactor(demo): x",
        ] {
            let level = match message_intent(message, &ws).expect("ok") {
                MessageIntent::Scoped { level, .. } | MessageIntent::PathFallback { level, .. } => {
                    level
                }
            };
            assert_eq!(level, BumpLevel::None, "{message}");
        }
        assert_eq!(scoped("chore(demo)!: x\n", &ws).1, BumpLevel::Major);
        let footer = "chore(demo): x\n\nBREAKING CHANGE: api\n";
        assert_eq!(scoped(footer, &ws).1, BumpLevel::Major);
        assert_eq!(fallback_level("Update the readme\n", &ws), BumpLevel::Patch);
    }

    #[test]
    fn feat_case_insensitive() {
        let ws = workspace(&["demo"]);
        assert_eq!(scoped("Feat(demo): caps\n", &ws).1, BumpLevel::Minor);
    }

    fn unresolved_scope(message: &str, ws: &Workspace) -> Option<String> {
        match message_intent(message, ws).expect("ok") {
            MessageIntent::PathFallback {
                unresolved_scope, ..
            } => unresolved_scope,
            MessageIntent::Scoped { .. } => panic!("expected path fallback"),
        }
    }

    #[test]
    fn unknown_scope_is_path_fallback_with_level() {
        let ws = workspace(&["demo"]);
        assert_eq!(fallback_level("feat(other): x\n", &ws), BumpLevel::Minor);
        assert_eq!(
            unresolved_scope("feat(other): x\n", &ws).as_deref(),
            Some("other")
        );
        assert_eq!(
            unresolved_scope("fix(demo,other): x\n", &ws).as_deref(),
            Some("demo,other")
        );
        assert_eq!(unresolved_scope("feat: x\n", &ws), None);
        assert_eq!(unresolved_scope("Update the readme\n", &ws), None);
    }

    fn shared_in_two_ecosystems() -> Workspace {
        let packages = Vec::from([
            Package::new(
                cargo("shared"),
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
        Workspace::new(packages).expect("workspace")
    }

    #[test]
    fn ambiguous_scope_on_a_release_is_error() {
        let ws = shared_in_two_ecosystems();
        let err = message_intent("feat(shared): x\n", &ws).expect_err("ambiguous");
        assert!(err.contains("more than one"), "{err}");
    }

    /// Nothing releases, so there is no ecosystem to guess.
    #[test]
    fn ambiguous_scope_releasing_nothing_falls_back_to_paths() {
        let ws = shared_in_two_ecosystems();
        assert_eq!(fallback_level("chore(shared): x\n", &ws), BumpLevel::None);
    }

    #[test]
    fn unscoped_conventional_is_path_fallback_with_level() {
        let ws = workspace(&["demo"]);
        assert_eq!(fallback_level("feat: no scope\n", &ws), BumpLevel::Minor);
    }

    #[test]
    fn unscoped_breaking_is_path_fallback_major() {
        let ws = workspace(&["demo"]);
        assert_eq!(fallback_level("feat!: break\n", &ws), BumpLevel::Major);
    }

    #[test]
    fn paths_map_to_package_dir() {
        let dirs = [(cargo("core"), String::from("crates/core"))];
        let files = [String::from("crates/core/src/lib.rs")];
        let got = packages_for_paths(&files, &dirs);
        assert_eq!(got, BTreeSet::from([cargo("core")]));
    }

    #[test]
    fn nested_package_wins_longest_prefix() {
        let dirs = [
            (cargo("app"), String::from("crates/app")),
            (cargo("app-core"), String::from("crates/app/core")),
        ];
        let files = [String::from("crates/app/core/src/lib.rs")];
        let got = packages_for_paths(&files, &dirs);
        assert_eq!(got, BTreeSet::from([cargo("app-core")]));
    }

    #[test]
    fn root_package_does_not_steal_nested_paths() {
        let dirs = [
            (cargo("root"), String::new()),
            (cargo("core"), String::from("crates/core")),
        ];
        let files = [String::from("crates/core/src/lib.rs")];
        let got = packages_for_paths(&files, &dirs);
        assert_eq!(got, BTreeSet::from([cargo("core")]));
    }

    #[test]
    fn colocated_packages_at_same_prefix_both_attribute() {
        let npm = PackageId::new(Ecosystem::Npm, "oakum");
        let dirs = [
            (cargo("oakum"), String::from("crates/oakum")),
            (npm.clone(), String::from("crates/oakum")),
        ];
        let files = [String::from("crates/oakum/src/lib.rs")];
        let got = packages_for_paths(&files, &dirs);
        assert_eq!(got, BTreeSet::from([cargo("oakum"), npm]));
    }

    #[test]
    fn an_intent_needs_a_package() {
        let one = BTreeSet::from([cargo("demo")]);
        assert!(CommitIntent::new(BTreeSet::new(), BumpLevel::Patch, String::new()).is_none());
        assert!(CommitIntent::new(one.clone(), BumpLevel::None, String::new()).is_some());
        assert!(CommitIntent::new(one, BumpLevel::Patch, String::new()).is_some());
    }

    #[test]
    fn a_commit_bump_file_names_its_packages_at_its_level() {
        let npm = PackageId::new(Ecosystem::Npm, "demo");
        let intent = CommitIntent::new(
            BTreeSet::from([cargo("demo"), npm.clone()]),
            BumpLevel::Minor,
            String::from("add thing"),
        )
        .expect("intent");
        let file = intent.bump_file(String::from("abc123"));
        assert_eq!(file.id, "abc123");
        assert_eq!(
            file.entries,
            Vec::from([(npm, BumpLevel::Minor), (cargo("demo"), BumpLevel::Minor)])
        );
        assert_eq!(file.note, "- add thing");
    }
}
