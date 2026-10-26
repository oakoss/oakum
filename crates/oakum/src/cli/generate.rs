//! `oakum generate`: derive one bump file from branch commits (ADR-0029 / `okm-j1r`).

use std::collections::{BTreeMap, BTreeSet};

use clap::Args;

use oakum::changeset::PackageSpec;
use oakum::commits::{message_intent, packages_for_paths, CommitIntent, MessageIntent};
use oakum::plan::{BumpLevel, Package, PackageId, Workspace};

use super::add::{bump_file_body, discover_workspace, knope_presence, write_bump_file_in};
use super::config::{load_config, require_config};
use super::deliver_block;
use super::git::{Git, Op};
use super::install_pin::{self, PinPolicy};
use super::repository;
use super::CliError;

#[derive(Debug, Args)]
pub(super) struct GenerateArgs {
    /// Git ref to scan from (exclusive). Default: merge-base with `origin/main`, then `main`, then `master`.
    #[arg(long, value_name = "REF")]
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
    let CommitIntents { intents, dropped } = commit_intents(&git, &workspace, &from, |package| {
        config.version_managed(package)
    })?;
    if intents.is_empty() {
        if !dropped.is_empty() {
            let names: BTreeSet<&str> = dropped.iter().map(|id| id.name.as_str()).collect();
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
    for (_, intent) in &intents {
        let names: BTreeSet<&str> = intent
            .packages()
            .iter()
            .map(|id| id.name.as_str())
            .collect();
        for name in names {
            let level = levels.entry(String::from(name)).or_insert(BumpLevel::None);
            *level = (*level).max(intent.level());
            notes.push(format!("- {name}: {}", intent.summary()));
        }
    }
    let note = notes.join("\n");

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
    /// Named by a commit, left out by `keep`.
    pub(super) dropped: BTreeSet<PackageId>,
}

/// Each commit in `from..HEAD` that names a package `keep` admits.
pub(super) fn commit_intents(
    git: &Git,
    workspace: &Workspace,
    from: &str,
    keep: impl Fn(&Package) -> bool,
) -> Result<CommitIntents, Box<dyn std::error::Error>> {
    let commits = list_commits(git, from)?;
    let package_dirs: Vec<(PackageId, String)> = workspace
        .packages()
        .map(|package| (package.id().clone(), package.manifest_dir().to_owned()))
        .collect();
    let mut intents = Vec::new();
    let mut dropped = BTreeSet::new();
    for commit in &commits {
        let message = commit.message();
        let (packages, level, summary) =
            match message_intent(&message, workspace).map_err(CliError::new)? {
                MessageIntent::Scoped {
                    package,
                    level,
                    summary,
                } => (BTreeSet::from([package]), level, summary),
                MessageIntent::PathFallback { level, summary } => {
                    let files = files_changed_in_commit(git, &commit.hash)?;
                    (packages_for_paths(&files, &package_dirs), level, summary)
                }
            };
        let (kept, left_out): (BTreeSet<PackageId>, _) = packages
            .into_iter()
            .partition(|id| workspace.get(id).is_some_and(&keep));
        dropped.extend(left_out);
        if let Some(intent) = CommitIntent::new(kept, level, summary) {
            intents.push((commit.hash.clone(), intent));
        }
    }
    Ok(CommitIntents { intents, dropped })
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

fn list_commits(git: &Git, from: &str) -> Result<Vec<GitCommit>, Box<dyn std::error::Error>> {
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
    use super::{merge_base, resolve_from_ref, Git};
    use crate::cli::git::Reply;

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
