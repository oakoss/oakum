//! `oakum generate`: derive one bump file from branch commits (ADR-0029 / `okm-j1r`).

use clap::Args;

use oakum::changeset::{write, PackageSpec};
use oakum::commits::{
    aggregate, contributions_from_paths, message_intent, AggregatedIntent, CommitContribution,
    MessageIntent,
};
use oakum::plan::Workspace;

use super::add::{discover_workspace, knope_presence, write_bump_file_in};
use super::config::{enforce_tool_version, load_config};
use super::git::{Git, Op};
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
    enforce_tool_version(&config)?;
    if !config.generate_allowed() {
        return Err(Box::new(CliError::new(
            "`oakum generate` needs both `change-files` and `conventional-commits` enabled in `.changeset/_config.toml` (ADR-0029)",
        )));
    }

    let workspace = discover_workspace(repo.path())?;
    let git = Git::at(repo.path());
    let from = resolve_from_ref(&git, args.from.as_deref())?;
    let aggregated = aggregated_intent_from_commits(&git, &workspace, &from)?;
    if aggregated.entries().is_empty() {
        return Err(Box::new(CliError::new(
            "no package bumps detected from commits (need a conventional scope matching a workspace package, or changed files under a package directory)",
        )));
    }

    let specs: Vec<PackageSpec> = aggregated
        .entries()
        .iter()
        .map(|(name, level)| PackageSpec::new(name.clone(), *level))
        .collect();

    if args.dry_run {
        let knope = knope_presence(repo.path());
        let body = write(
            &aggregated
                .entries()
                .iter()
                .map(|(n, l)| (n.clone(), *l))
                .collect::<Vec<_>>(),
            aggregated.note(),
            knope,
        )
        .map_err(|err| CliError::new(err.to_string()))?;
        print!("{body}");
        return Ok(());
    }

    write_bump_file_in(
        repo.path(),
        &workspace,
        &specs,
        aggregated.note(),
        args.name.as_deref(),
    )?;
    Ok(())
}

/// Aggregate package bumps for `from..HEAD` (shared with commits-only plan intent).
pub(super) fn aggregated_intent_from_commits(
    git: &Git,
    workspace: &Workspace,
    from: &str,
) -> Result<AggregatedIntent, Box<dyn std::error::Error>> {
    let commits = list_commits(git, from)?;
    // Empty range → empty intent. Plan treats that as nothing to release;
    // `generate` refuses empty intent in `run`.
    if commits.is_empty() {
        return Ok(aggregate(&[]));
    }

    let package_dirs: Vec<(String, String)> = workspace
        .packages()
        .map(|package| (package.id().name.clone(), package.manifest_dir().to_owned()))
        .collect();
    let mut contributions: Vec<CommitContribution> = Vec::new();
    for commit in &commits {
        let message = commit.message();
        let intent = message_intent(&message, workspace).map_err(CliError::new)?;
        match intent {
            MessageIntent::Contributions(mapped) => contributions.extend(mapped),
            MessageIntent::PathFallback { level, summary } => {
                let files = files_changed_in_commit(git, &commit.hash)?;
                contributions.extend(contributions_from_paths(
                    &files,
                    &package_dirs,
                    level,
                    &summary,
                ));
            }
        }
    }
    Ok(aggregate(&contributions))
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
