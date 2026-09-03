//! Plan intent loader: change files or commits-only (ADR-0029 / `okm-64b.5`).

use std::io;
use std::path::Path;

use clap::Args;
use oakum::changeset::{is_bump_file_name, load_bump_files};
use oakum::commits::to_bump_file;
use oakum::plan::{BumpFile, Workspace};
use serde::Serialize;

use super::add::discover_workspace;
use super::config::{load_config, LoadedConfig, PlanIntentSource};
use super::fs::{repo_path_display, resolve_capability_path};
use super::generate::{aggregated_intent_from_commits, resolve_from_ref};
use super::git::Git;
use super::repository::{self, Repository};
use super::write_set::read_text;
use super::CliError;

/// Synthetic bump-file id for commits-only plan (never written to disk).
pub(super) const COMMITS_BUMP_FILE_ID: &str = "commits";

#[derive(Debug, Args)]
pub(super) struct PlanIntentArgs {
    /// Git ref to scan from (exclusive). Same default as `generate`.
    #[arg(long, value_name = "REF")]
    from: Option<String>,
}

/// Hidden until `status`/`check` land; ADR-0029 plan intent for integration tests.
pub(super) fn run(args: &PlanIntentArgs) -> Result<(), Box<dyn std::error::Error>> {
    let repo = repository::discover()?;
    let config = load_config(&repo)?;
    let workspace = discover_workspace(&repo)?;
    let git = Git::at_repository(&repo)?;
    let files = load_plan_bump_files(&git, &repo, &workspace, &config, args.from.as_deref())?;
    let report: Vec<PlanIntentReportFile> = files.iter().map(PlanIntentReportFile::from).collect();
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

#[derive(Debug, Serialize)]
struct PlanIntentReportFile {
    id: String,
    entries: Vec<PlanIntentReportEntry>,
    note: String,
}

#[derive(Debug, Serialize)]
struct PlanIntentReportEntry {
    ecosystem: String,
    name: String,
    level: String,
}

impl From<&BumpFile> for PlanIntentReportFile {
    fn from(file: &BumpFile) -> Self {
        Self {
            id: file.id.clone(),
            entries: file
                .entries
                .iter()
                .map(|(id, level)| PlanIntentReportEntry {
                    ecosystem: id.ecosystem.to_string(),
                    name: id.name.clone(),
                    level: level.to_string(),
                })
                .collect(),
            note: file.note.clone(),
        }
    }
}

/// Plan bump-file inputs per ADR-0029 (change files or one synthetic commits file).
/// Does not write.
///
/// `from` is the exclusive git base for commits-only mode (same default rules as
/// `oakum generate` when `None`).
pub(super) fn load_plan_bump_files(
    git: &Git,
    repo: &Repository,
    workspace: &Workspace,
    config: &LoadedConfig,
    from: Option<&str>,
) -> Result<Vec<BumpFile>, Box<dyn std::error::Error>> {
    match config.plan_intent_source()? {
        PlanIntentSource::ChangeFiles => load_change_files(repo, workspace),
        PlanIntentSource::CommitsOnly => {
            let from = resolve_from_ref(git, from)?;
            let intent = aggregated_intent_from_commits(git, workspace, &from)?;
            if intent.entries().is_empty() {
                return Ok(Vec::new());
            }
            let file = to_bump_file(&intent, workspace, String::from(COMMITS_BUMP_FILE_ID))
                .map_err(CliError::new)?;
            Ok(Vec::from([file]))
        }
    }
}

fn load_change_files(
    repo: &Repository,
    workspace: &Workspace,
) -> Result<Vec<BumpFile>, Box<dyn std::error::Error>> {
    let dir = repo.dir();
    match dir.symlink_metadata(".changeset") {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(Box::new(CliError::new(format!(
                "failed to inspect `.changeset`: {err}"
            ))));
        }
    }
    let changeset = resolve_capability_path(dir, repo.path(), Path::new(".changeset"))?;
    let entries = dir.read_dir(&changeset).map_err(|err| {
        CliError::new(format!(
            "failed to read `{}`: {err}",
            repo_path_display(&changeset)
        ))
    })?;

    let mut pairs: Vec<(String, String)> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| {
            CliError::new(format!(
                "failed to read `{}`: {err}",
                repo_path_display(&changeset)
            ))
        })?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !is_bump_file_name(name) {
            continue;
        }
        let relative = changeset.join(name);
        match dir.metadata(&relative) {
            Ok(meta) if meta.is_file() => {}
            Ok(_) => continue,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(Box::new(CliError::new(format!(
                    "failed to read `{}`: file disappeared",
                    repo_path_display(&relative)
                ))));
            }
            Err(err) => {
                return Err(Box::new(CliError::new(format!(
                    "failed to inspect `{}`: {err}",
                    repo_path_display(&relative)
                ))));
            }
        }
        let Some(body) = read_text(dir, &relative)? else {
            return Err(Box::new(CliError::new(format!(
                "failed to read `{}`: file disappeared",
                repo_path_display(&relative)
            ))));
        };
        pairs.push((String::from(name), body));
    }
    pairs.sort_by(|a, b| a.0.cmp(&b.0));

    let loaded = load_bump_files(
        pairs
            .iter()
            .map(|(name, body)| (name.as_str(), body.as_str())),
        workspace,
    )
    .map_err(|err| CliError::new(err.to_string()))?;

    for report in &loaded.malformed {
        eprintln!("{report}");
    }

    Ok(loaded.files)
}
