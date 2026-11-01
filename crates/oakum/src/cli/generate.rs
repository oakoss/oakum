//! `oakum generate`: derive one bump file from branch commits (ADR-0029 / `okm-j1r`).

use std::collections::{BTreeMap, BTreeSet};

use clap::Args;

use oakum::changeset::PackageSpec;
use oakum::commits::{message_intent, packages_for_paths, CommitIntent, MessageIntent};
use oakum::plan::{BumpLevel, Package, PackageId, Workspace};
use oakum::state::UnresolvedScope;

use super::add::{bump_file_body, discover_workspace, knope_presence, write_bump_file_in};
use super::config::{load_config, require_config, unresolved_scope_line};
use super::deliver_block;
use super::git::{Git, Op};
use super::install_pin::{self, PinPolicy};
use super::repository;
use super::CliError;

#[derive(Debug, Args)]
pub(super) struct GenerateArgs {
    #[arg(long, value_name = "REF", help = super::FROM_MERGE_BASE)]
    from: Option<String>,

    /// Print the planned bump file without writing.
    #[arg(long)]
    dry_run: bool,

    /// Filename stem (slugified). Defaults to a generated name.
    #[arg(long, value_name = "SLUG")]
    name: Option<String>,
}

pub(super) fn run(args: &GenerateArgs) -> Result<(), Box<dyn std::error::Error>> {
    let repo = repository::discover()?;
    let config = load_config(&repo)?;
    require_config(&config)?;
    install_pin::gate(&repo, PinPolicy::Writer)?;
    if !config.generate_allowed() {
        return Err(Box::new(CliError::new(
            "`oakum generate` needs both `change-files` and `conventional-commits` enabled in `.changeset/_config.toml` (ADR-0029)",
        )));
    }

    let workspace = discover_workspace(&repo)?;
    config.validate_workspace_selection(&workspace)?;
    let git = Git::at_repository(&repo)?;
    let from = resolve_from_ref(&git, args.from.as_deref())?;
    // A package the config does not version is never released, so a bump file
    // naming it is one every later step refuses.
    let CommitIntents {
        intents,
        left_out,
        unresolved,
    } = commit_intents(
        &git,
        &workspace,
        &CommitRanges::from_base(&from, &workspace),
        |package| config.version_managed(package),
    )?;
    for commit in &unresolved {
        super::say_err(&unresolved_scope_line(commit));
    }
    if intents.is_empty() {
        if !left_out.is_empty() {
            let names: BTreeSet<&str> = left_out.iter().map(|id| id.name.as_str()).collect();
            return Err(Box::new(CliError::new(format!(
                "the commits touched only packages the config does not version: {}; adjust `include`/`exclude` or set `private-packages.version = true`",
                names.into_iter().collect::<Vec<_>>().join(", ")
            ))));
        }
        return Err(Box::new(CliError::new(
            "no package bumps detected from commits (need a conventional scope matching a workspace package, or changed files under a package directory)",
        )));
    }

    // A bump file names packages, so a Cargo crate and an npm package sharing
    // a name take one line at the higher of their levels.
    let mut levels: BTreeMap<String, BumpLevel> = BTreeMap::new();
    let mut notes: Vec<String> = Vec::new();
    let mut coverage_notes: Vec<String> = Vec::new();
    for (_, intent) in &intents {
        let names: BTreeSet<&str> = intent
            .packages()
            .iter()
            .map(|id| id.name.as_str())
            .collect();
        for name in names {
            let level = levels.entry(String::from(name)).or_insert(BumpLevel::None);
            *level = (*level).max(intent.level());
            let line = format!("- {name}: {}", intent.summary());
            if intent.level().is_release() {
                notes.push(line);
            } else {
                coverage_notes.push(line);
            }
        }
    }
    // The note reaches every package the file releases, so a commit that
    // releases nothing stays out of it unless nothing in the file releases.
    let note = if notes.is_empty() {
        coverage_notes
    } else {
        notes
    }
    .join("\n");

    // No dropped-note check on this arm, unlike `write_bump_file_in` below:
    // the note is composed from commits as `- <package>: <summary>` lines, so
    // it always carries body text and never renders as a bare heading.
    if args.dry_run {
        let knope = knope_presence(&repo)?;
        let entries: Vec<(String, BumpLevel)> = levels.into_iter().collect();
        let body =
            bump_file_body(&entries, &note, knope).map_err(|err| CliError::new(err.to_string()))?;
        deliver_block(&body)
            .map_err(|err| CliError::undelivered("bump file composed, but it", &err))?;
        return Ok(());
    }

    let specs: Vec<PackageSpec> = levels
        .into_iter()
        .map(|(name, level)| PackageSpec::new(name, level))
        .collect();
    write_bump_file_in(
        &repo,
        &workspace,
        &config,
        &specs,
        &note,
        args.name.as_deref(),
    )?;
    Ok(())
}

pub(super) struct CommitIntents {
    /// Keyed by commit hash.
    pub(super) intents: Vec<(String, CommitIntent)>,
    /// Named by a releasing commit, left out by `keep`.
    pub(super) left_out: BTreeSet<PackageId>,
    pub(super) unresolved: Vec<UnresolvedScope>,
}

/// Where each package's commits start: after its base, or at the root of
/// `HEAD`'s history when the base is `None`. A map, so no package can read
/// from two bases.
pub(super) struct CommitRanges(BTreeMap<PackageId, Option<String>>);

impl CommitRanges {
    pub(super) const fn new(bases: BTreeMap<PackageId, Option<String>>) -> Self {
        Self(bases)
    }

    pub(super) fn from_base(base: &str, workspace: &Workspace) -> Self {
        Self::new(
            workspace
                .packages()
                .map(|package| (package.id().clone(), Some(String::from(base))))
                .collect(),
        )
    }
}

/// Each commit in `ranges` that names a package `keep` admits, attributed only
/// to the packages whose range holds it.
pub(super) fn commit_intents(
    git: &Git,
    workspace: &Workspace,
    ranges: &CommitRanges,
    keep: impl Fn(&Package) -> bool,
) -> Result<CommitIntents, Box<dyn std::error::Error>> {
    let package_dirs: Vec<(PackageId, String)> = workspace
        .packages()
        .map(|package| (package.id().clone(), package.manifest_dir().to_owned()))
        .collect();
    let mut intents = Vec::new();
    let mut left_out_released = BTreeSet::new();
    let mut unresolved = Vec::new();
    for (commit, reading) in commits_in(git, ranges)? {
        let message = commit.message();
        let (packages, level, summary, unresolved_scope) =
            match message_intent(&message, workspace).map_err(CliError::new)? {
                MessageIntent::Scoped {
                    package,
                    level,
                    summary,
                } => (BTreeSet::from([package]), level, summary, None),
                MessageIntent::PathFallback {
                    level,
                    summary,
                    unresolved_scope,
                } => {
                    let files = files_changed_in_commit(git, &commit.hash)?;
                    let packages = packages_for_paths(&files, &package_dirs);
                    (packages, level, summary, unresolved_scope)
                }
            };
        let names_nothing = packages.is_empty();
        let reached = oakum::plan::reach(workspace, &packages, &keep);
        let kept: BTreeSet<PackageId> = reached
            .managed
            .into_keys()
            .filter(|id| reading.packages.contains(id))
            .collect();
        let left_out: BTreeSet<PackageId> = reached
            .stranded
            .into_iter()
            .filter(|id| reading.packages.contains(id))
            .collect();
        // Only a commit that asks for a release drops intent; routine work
        // under an unknown scope or on an unversioned package is not that. A
        // scope naming nothing is reported only while every range holds the
        // commit, so history before a release does not resurface.
        if level.is_release() {
            match unresolved_scope {
                Some(scope) if names_nothing && reading.in_every_range => {
                    unresolved.push(UnresolvedScope {
                        commit: commit.hash.clone(),
                        scope,
                    });
                }
                _ => left_out_released.extend(left_out),
            }
        }
        if let Some(intent) = CommitIntent::new(kept, level, summary) {
            intents.push((commit.hash.clone(), intent));
        }
    }
    Ok(CommitIntents {
        intents,
        left_out: left_out_released,
        unresolved,
    })
}

#[derive(Debug)]
struct GitCommit {
    hash: String,
    subject: String,
    body: String,
}

impl GitCommit {
    fn message(&self) -> String {
        if self.body.is_empty() {
            self.subject.clone()
        } else {
            format!("{}\n\n{}", self.subject, self.body)
        }
    }
}

pub(super) fn resolve_from_ref(
    git: &Git,
    explicit: Option<&str>,
) -> Result<String, Box<dyn std::error::Error>> {
    if let Some(from) = explicit {
        return Ok(String::from(from));
    }
    for candidate in ["origin/main", "main", "master"] {
        if git.predicate(Op::RefExists {
            reference: candidate,
        })? {
            // No common ancestor is a real answer (exit 1, empty streams);
            // fall back to the tip. A failure to look is an error.
            return Ok(match merge_base(git, candidate)? {
                Some(base) => base,
                None => String::from(candidate),
            });
        }
    }
    Err(Box::new(CliError::new(
        "could not find a default base ref; pass `--from <ref>`",
    )))
}

fn merge_base(git: &Git, tip: &str) -> Result<Option<String>, CliError> {
    git.optional_text(Op::MergeBase { tip })
}

/// Which packages read a commit, and whether every base's range holds it.
#[derive(Debug, Default)]
struct Reading {
    packages: BTreeSet<PackageId>,
    in_every_range: bool,
}

/// Every commit some range holds, oldest first. One base lists its own range.
/// Several list `HEAD`'s whole history for the order, since ranges from tags on
/// diverging lines do not nest and no single range orders their union.
fn commits_in(
    git: &Git,
    ranges: &CommitRanges,
) -> Result<Vec<(GitCommit, Reading)>, Box<dyn std::error::Error>> {
    let mut by_base: BTreeMap<Option<&str>, BTreeSet<PackageId>> = BTreeMap::new();
    for (id, base) in &ranges.0 {
        by_base
            .entry(base.as_deref())
            .or_default()
            .insert(id.clone());
    }
    if let [(base, packages)] = by_base.iter().collect::<Vec<_>>().as_slice() {
        return Ok(list_commits(git, **base)?
            .into_iter()
            .map(|commit| {
                let reading = Reading {
                    packages: (*packages).clone(),
                    in_every_range: true,
                };
                (commit, reading)
            })
            .collect());
    }
    let mut held: BTreeMap<String, (BTreeSet<PackageId>, usize)> = BTreeMap::new();
    for (base, packages) in &by_base {
        for commit in list_commits(git, *base)? {
            let entry = held.entry(commit.hash).or_default();
            entry.0.extend(packages.iter().cloned());
            entry.1 += 1;
        }
    }
    if held.is_empty() {
        return Ok(Vec::new());
    }
    Ok(list_commits(git, None)?
        .into_iter()
        .filter_map(|commit| {
            let (packages, count) = held.remove(&commit.hash)?;
            let reading = Reading {
                packages,
                in_every_range: count == by_base.len(),
            };
            Some((commit, reading))
        })
        .collect())
}

fn list_commits(
    git: &Git,
    from: Option<&str>,
) -> Result<Vec<GitCommit>, Box<dyn std::error::Error>> {
    let raw = git.text(Op::Commits { from })?;
    let mut commits = Vec::new();
    for chunk in raw.split('\0').collect::<Vec<_>>().chunks(3) {
        if chunk.len() < 3 {
            continue;
        }
        let hash = chunk[0].trim();
        if hash.is_empty() {
            continue;
        }
        commits.push(GitCommit {
            hash: String::from(hash),
            subject: String::from(chunk[1].trim()),
            body: String::from(chunk[2].trim()),
        });
    }
    Ok(commits)
}

fn files_changed_in_commit(
    git: &Git,
    hash: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    // Merges: do not path-attribute. `diff-tree -m` unions both parents and can
    // credit base-branch-only files when main is merged into a feature branch.
    if commit_parent_count(git, hash)? > 1 {
        return Ok(Vec::new());
    }

    Ok(git.paths(Op::CommitPaths { hash })?)
}

fn commit_parent_count(git: &Git, hash: &str) -> Result<usize, Box<dyn std::error::Error>> {
    let line = git.text(Op::CommitParents { hash })?;
    let count = line.split_whitespace().count().saturating_sub(1);
    Ok(count)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use oakum::plan::{Ecosystem, Package, PackageId, ResolvesDependenciesAt, Workspace};
    use semver::Version;

    use super::{commit_intents, merge_base, resolve_from_ref, CommitRanges, Git};
    use crate::cli::git::Reply;

    fn workspace(names: &[&str]) -> Workspace {
        Workspace::new(names.iter().map(|name| {
            Package::new(
                PackageId::new(Ecosystem::Cargo, *name),
                Version::new(0, 1, 0),
                ResolvesDependenciesAt::Install,
                true,
                Vec::new(),
            )
            .with_manifest_dir(*name)
        }))
        .expect("workspace")
    }

    fn cargo(name: &str) -> PackageId {
        PackageId::new(Ecosystem::Cargo, name)
    }

    /// `git log` output for `(hash, subject)` pairs, oldest first.
    fn log(commits: &[(&str, &str)]) -> Reply {
        Reply::said(
            commits
                .iter()
                .flat_map(|(hash, subject)| [hash, "\0", subject, "\0\0\n"])
                .collect::<String>(),
        )
    }

    fn ranges(bases: &[(&str, Option<&str>)]) -> CommitRanges {
        CommitRanges::new(
            bases
                .iter()
                .map(|(name, base)| (cargo(name), base.map(String::from)))
                .collect::<BTreeMap<_, _>>(),
        )
    }

    /// Tags on diverging lines: neither range holds the other, so each
    /// package's commits keep `HEAD`'s order only if the union is ordered by
    /// one listing.
    #[test]
    fn diverging_ranges_keep_history_order() {
        let git = Git::answering([
            (
                "log",
                log(&[
                    ("b1", "fix(alpha): b1"),
                    ("B", "fix(alpha): B"),
                    ("h1", "fix(beta): h1"),
                ]),
            ),
            (
                "log",
                log(&[
                    ("a1", "fix(beta): a1"),
                    ("A", "fix(beta): A"),
                    ("h1", "fix(beta): h1"),
                ]),
            ),
            (
                "log",
                log(&[
                    ("r", "chore: root"),
                    ("a1", "fix(beta): a1"),
                    ("b1", "fix(alpha): b1"),
                    ("A", "fix(beta): A"),
                    ("B", "fix(alpha): B"),
                    ("h1", "fix(beta): h1"),
                ]),
            ),
        ]);
        let ws = workspace(&["alpha", "beta"]);
        let found = commit_intents(
            &git,
            &ws,
            &ranges(&[("alpha", Some("tagA")), ("beta", Some("tagB"))]),
            |_| true,
        )
        .expect("intents");
        let order: Vec<&str> = found
            .intents
            .iter()
            .map(|(hash, _)| hash.as_str())
            .collect();
        assert_eq!(order, ["a1", "b1", "A", "B", "h1"]);
    }

    /// A package reads only its own range: `b1` is before beta's base.
    #[test]
    fn a_commit_outside_a_package_range_is_not_its_intent() {
        let git = Git::answering([
            (
                "log",
                log(&[("b1", "fix(beta): b1"), ("h1", "fix(alpha): h1")]),
            ),
            ("log", log(&[("h1", "fix(alpha): h1")])),
            (
                "log",
                log(&[("b1", "fix(beta): b1"), ("h1", "fix(alpha): h1")]),
            ),
        ]);
        let ws = workspace(&["alpha", "beta"]);
        // Ranges list in base order, so `a-old` answers first.
        let found = commit_intents(
            &git,
            &ws,
            &ranges(&[("alpha", Some("a-old")), ("beta", Some("b-new"))]),
            |_| true,
        )
        .expect("intents");
        let named: Vec<(&str, Vec<&PackageId>)> = found
            .intents
            .iter()
            .map(|(hash, intent)| (hash.as_str(), intent.packages().iter().collect()))
            .collect();
        assert_eq!(named, [("h1", vec![&cargo("alpha")])]);
    }

    /// A scope naming nothing is reported only while every range holds the
    /// commit; one already behind a package's base has been released past.
    #[test]
    fn an_unresolved_scope_behind_a_base_is_not_reported() {
        let git = Git::answering([
            (
                "log",
                log(&[("x1", "fix(zzz): old"), ("x2", "fix(zzz): new")]),
            ),
            ("log", log(&[("x2", "fix(zzz): new")])),
            (
                "log",
                log(&[("x1", "fix(zzz): old"), ("x2", "fix(zzz): new")]),
            ),
            ("rev-list --parents", Reply::said("x1 p\n")),
            ("diff-tree", Reply::said("README.md\0")),
            ("rev-list --parents", Reply::said("x2 x1\n")),
            ("diff-tree", Reply::said("README.md\0")),
        ]);
        let ws = workspace(&["alpha", "beta"]);
        let found = commit_intents(
            &git,
            &ws,
            &ranges(&[("alpha", Some("a-old")), ("beta", Some("b-new"))]),
            |_| true,
        )
        .expect("intents");
        let reported: Vec<&str> = found.unresolved.iter().map(|u| u.commit.as_str()).collect();
        assert_eq!(reported, ["x2"]);
    }

    /// The paths named a package, just not one this read covers, so the scope
    /// did reach a package.
    #[test]
    fn a_scope_whose_paths_name_an_unread_package_is_not_unresolved() {
        let git = Git::answering([
            ("log", log(&[("x1", "fix(typo): p")])),
            ("rev-list --parents", Reply::said("x1 p\n")),
            ("diff-tree", Reply::said("pending/src/lib.rs\0")),
        ]);
        let ws = workspace(&["alpha", "pending"]);
        let found = commit_intents(&git, &ws, &ranges(&[("alpha", Some("v1"))]), |_| true)
            .expect("intents");
        assert!(found.unresolved.is_empty(), "{:?}", found.unresolved);
        assert_eq!(found.intents.len(), 0);
    }

    /// Nothing to read means no git at all.
    #[test]
    fn no_ranges_list_nothing() {
        let git = Git::answering([]);
        let found =
            commit_intents(&git, &workspace(&["alpha"]), &ranges(&[]), |_| true).expect("intents");
        assert_eq!(found.intents.len(), 0);
        assert!(git.asked().is_empty(), "{:?}", git.asked());
    }

    #[test]
    fn resolve_from_ref_keeps_an_explicit_ref() {
        let git = Git::answering([]);
        let from = resolve_from_ref(&git, Some("v1.0.0")).expect("explicit");
        assert_eq!(from, "v1.0.0");
        assert!(git.asked().is_empty(), "{:?}", git.asked());
    }

    #[test]
    fn resolve_from_ref_uses_the_merge_base_when_one_exists() {
        // `--quiet` still prints the object id on success.
        let git = Git::answering([
            ("rev-parse --verify", Reply::said("tipsha")),
            ("merge-base", Reply::said("abc123")),
        ]);
        let from = resolve_from_ref(&git, None).expect("resolved");
        assert_eq!(from, "abc123");
        assert_eq!(
            git.asked(),
            vec![
                String::from("rev-parse --verify"),
                String::from("merge-base"),
            ]
        );
    }

    #[test]
    fn resolve_from_ref_falls_back_to_the_tip_when_histories_are_unrelated() {
        // Measured: unrelated histories → exit 1, empty streams (`said_no`).
        let git = Git::answering([
            ("rev-parse --verify", Reply::said("tipsha")),
            ("merge-base", Reply::absent()),
        ]);
        let from = resolve_from_ref(&git, None).expect("resolved");
        assert_eq!(from, "origin/main");
    }

    #[test]
    fn resolve_from_ref_propagates_a_diagnosed_merge_base_failure() {
        let git = Git::answering([
            ("rev-parse --verify", Reply::said("tipsha")),
            (
                "merge-base",
                Reply::failed(128, "fatal: Not a valid object name origin/main"),
            ),
        ]);
        let err = resolve_from_ref(&git, None).expect_err("diagnosed failure");
        assert!(
            err.to_string().contains("merge-base") || err.to_string().contains("valid object"),
            "{err}"
        );
    }

    #[test]
    fn merge_base_distinguishes_no_ancestor_from_a_failed_look() {
        let unrelated = Git::answering([("merge-base", Reply::absent())]);
        assert_eq!(
            merge_base(&unrelated, "main").expect("looked"),
            None,
            "no common ancestor is Ok(None)"
        );

        let diagnosed = Git::answering([(
            "merge-base",
            Reply::failed(128, "fatal: Not a valid object name main"),
        )]);
        let err = merge_base(&diagnosed, "main").expect_err("failed look");
        assert!(
            err.to_string().contains("merge-base") || err.to_string().contains("valid object"),
            "{err}"
        );
    }
}
