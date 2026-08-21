//! `oakum generate`: derive one bump file from branch commits (ADR-0029 / `okm-j1r`).

use std::path::Path;
use std::process::Command;

use clap::Args;

use oakum::changeset::{write, PackageSpec};
use oakum::commits::{
    aggregate, contributions_from_paths, message_intent, AggregatedIntent, CommitContribution,
    MessageIntent,
};
use oakum::plan::Workspace;

use super::add::{discover_workspace, knope_presence, repo_root, write_bump_file_in};
use super::config::{enforce_tool_version, load_config};
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
    let repo = repo_root()?;
    let config = load_config(&repo)?;
    enforce_tool_version(&config)?;
    if !config.generate_allowed() {
        return Err(Box::new(CliError::new(
            "`oakum generate` needs both `change-files` and `conventional-commits` enabled in `.changeset/_config.toml` (ADR-0029)",
        )));
    }

    let workspace = discover_workspace(&repo)?;
    let from = resolve_from_ref(&repo, args.from.as_deref())?;
    let aggregated = aggregated_intent_from_commits(&repo, &workspace, &from)?;
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
        let knope = knope_presence(&repo);
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
        &repo,
        &workspace,
        &specs,
        aggregated.note(),
        args.name.as_deref(),
    )?;
    Ok(())
}

/// Aggregate package bumps for `from..HEAD` (shared with commits-only plan intent).
pub(super) fn aggregated_intent_from_commits(
    repo: &Path,
    workspace: &Workspace,
    from: &str,
) -> Result<AggregatedIntent, Box<dyn std::error::Error>> {
    let commits = list_commits(repo, from)?;
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
                let files = files_changed_in_commit(repo, &commit.hash)?;
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
    repo: &Path,
    explicit: Option<&str>,
) -> Result<String, Box<dyn std::error::Error>> {
    if let Some(from) = explicit {
        return Ok(String::from(from));
    }
    for candidate in ["origin/main", "main", "master"] {
        if git_ok(repo, &["rev-parse", "--verify", candidate]) {
            if let Some(base) = merge_base(repo, candidate) {
                return Ok(base);
            }
            return Ok(String::from(candidate));
        }
    }
    Err(Box::new(CliError::new(
        "could not find a default base ref; pass `--from <ref>`",
    )))
}

fn merge_base(repo: &Path, tip: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["merge-base", tip, "HEAD"])
        .current_dir(repo)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn git_ok(repo: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .is_ok_and(|o| o.status.success())
}

fn list_commits(repo: &Path, from: &str) -> Result<Vec<GitCommit>, Box<dyn std::error::Error>> {
    let range = format!("{from}..HEAD");
    let output = Command::new("git")
        .args(["log", &range, "--reverse", "--format=%H%x00%s%x00%b%x00"])
        .current_dir(repo)
        .output()
        .map_err(|err| CliError::new(format!("failed to run git log: {err}")))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(Box::new(CliError::new(format!(
            "git log {range} failed: {err}"
        ))));
    }
    let raw = String::from_utf8_lossy(&output.stdout);
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
    repo: &Path,
    hash: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    // Merges: do not path-attribute. `diff-tree -m` unions both parents and can
    // credit base-branch-only files when main is merged into a feature branch.
    if commit_parent_count(repo, hash)? > 1 {
        return Ok(Vec::new());
    }

    let output = Command::new("git")
        .args([
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "-r",
            "--root",
            hash,
        ])
        .current_dir(repo)
        .output()
        .map_err(|err| CliError::new(format!("failed to list files for {hash}: {err}")))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(Box::new(CliError::new(format!(
            "git diff-tree {hash} failed: {err}"
        ))));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect())
}

fn commit_parent_count(repo: &Path, hash: &str) -> Result<usize, Box<dyn std::error::Error>> {
    let output = Command::new("git")
        .args(["rev-list", "--parents", "-n", "1", hash])
        .current_dir(repo)
        .output()
        .map_err(|err| CliError::new(format!("failed to inspect parents for {hash}: {err}")))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(Box::new(CliError::new(format!(
            "git rev-list --parents {hash} failed: {err}"
        ))));
    }
    let line = String::from_utf8_lossy(&output.stdout);
    let count = line.split_whitespace().count().saturating_sub(1);
    Ok(count)
}
