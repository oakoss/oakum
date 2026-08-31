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
            if let Some(base) = merge_base(git, candidate) {
                return Ok(base);
            }
            return Ok(String::from(candidate));
        }
    }
    Err(Box::new(CliError::new(
        "could not find a default base ref; pass `--from <ref>`",
    )))
}

/// A tip with no common ancestor is not an error; the caller tries the next one.
fn merge_base(git: &Git, tip: &str) -> Option<String> {
    git.text(Op::MergeBase { tip })
        .ok()
        .filter(|base| !base.is_empty())
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
