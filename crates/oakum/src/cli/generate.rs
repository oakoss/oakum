//! `oakum generate`: derive one bump file from branch commits (ADR-0029 / `okm-j1r`).

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Args;
use serde::Deserialize;

use oakum::changeset::{write, PackageSpec};
use oakum::commits::{
    aggregate, contributions_from_paths, message_intent, AggregatedIntent, CommitContribution,
    MessageIntent,
};
use oakum::plan::Workspace;

use super::add::{
    discover_workspace, find_manifest_dir, knope_presence, repo_root, write_bump_file_in,
};
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

    let package_dirs = package_dir_map(repo)?;
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

/// Same nestable start directories as [`discover_workspace`].
fn package_dir_map(repo: &Path) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    let cwd = std::env::current_dir()?;
    let mut dirs = Vec::new();
    let mut probed = false;

    let cargo_dir = find_manifest_dir(&cwd, repo, "Cargo.toml");
    if cargo_dir.is_some() || repo.join("Cargo.toml").is_file() {
        probed = true;
        let start = cargo_dir.as_deref().unwrap_or(repo);
        dirs.extend(cargo_package_dirs(start, repo)?);
    }

    let pnpm_marker = repo.join("pnpm-workspace.yaml").is_file()
        || repo.join("package.json").is_file()
        || find_manifest_dir(&cwd, repo, "package.json").is_some();
    if pnpm_marker {
        probed = true;
        let start = find_manifest_dir(&cwd, repo, "package.json")
            .or_else(|| find_manifest_dir(&cwd, repo, "pnpm-workspace.yaml"))
            .unwrap_or_else(|| repo.to_path_buf());
        dirs.extend(pnpm_package_dirs(&start, repo)?);
    }

    if probed && dirs.is_empty() {
        return Err(Box::new(CliError::new(
            "could not resolve package directories for path fallback",
        )));
    }
    Ok(dirs)
}

fn cargo_package_dirs(
    start: &Path,
    repo: &Path,
) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(start)
        .output()
        .map_err(|err| CliError::new(format!("failed to run cargo metadata: {err}")))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(Box::new(CliError::new(format!(
            "cargo metadata failed: {err}"
        ))));
    }
    let meta: CargoMetadata = serde_json::from_slice(&output.stdout)
        .map_err(|err| CliError::new(format!("cargo metadata JSON: {err}")))?;
    let repo_canon = std::fs::canonicalize(repo)?;
    let mut out = Vec::new();
    for pkg in meta.packages {
        if !meta.workspace_members.contains(&pkg.id) {
            continue;
        }
        let manifest = PathBuf::from(&pkg.manifest_path);
        let dir = manifest
            .parent()
            .ok_or_else(|| CliError::new("package manifest has no parent"))?;
        let rel = repo_relative_dir(dir, &repo_canon)?;
        out.push((pkg.name, rel));
    }
    Ok(out)
}

fn pnpm_package_dirs(
    start: &Path,
    repo: &Path,
) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    #[derive(Deserialize)]
    struct PnpmEntry {
        name: Option<String>,
        path: Option<String>,
        version: Option<String>,
    }

    let output = Command::new("pnpm")
        .args(["list", "-r", "--depth", "-1", "--json"])
        .current_dir(start)
        .output()
        .map_err(|err| CliError::new(format!("failed to run pnpm list: {err}")))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(Box::new(CliError::new(format!("pnpm list failed: {err}"))));
    }
    let entries: Vec<PnpmEntry> = serde_json::from_slice(&output.stdout)
        .map_err(|err| CliError::new(format!("pnpm list JSON: {err}")))?;
    let repo_canon = std::fs::canonicalize(repo)?;
    let mut out = Vec::new();
    for entry in entries {
        // Match discover/pnpm: versionless entries (private workspace roots) are not packages.
        if entry.version.is_none() {
            continue;
        }
        let (Some(name), Some(path)) = (entry.name, entry.path) else {
            continue;
        };
        let dir = PathBuf::from(path);
        let rel = repo_relative_dir(&dir, &repo_canon)?;
        out.push((name, rel));
    }
    Ok(out)
}

fn repo_relative_dir(dir: &Path, repo_canon: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let dir_canon = std::fs::canonicalize(dir).map_err(|err| {
        CliError::new(format!(
            "failed to canonicalize package dir {}: {err}",
            dir.display()
        ))
    })?;
    let rel = dir_canon.strip_prefix(repo_canon).map_err(|_| {
        CliError::new(format!(
            "package directory {} is outside the repository root {}",
            dir_canon.display(),
            repo_canon.display()
        ))
    })?;
    Ok(rel.to_string_lossy().replace('\\', "/"))
}

#[derive(Deserialize)]
struct CargoMetadata {
    packages: Vec<CargoPackage>,
    workspace_members: Vec<String>,
}

#[derive(Deserialize)]
struct CargoPackage {
    name: String,
    id: String,
    manifest_path: String,
}
