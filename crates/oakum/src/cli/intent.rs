//! Plan intent loader: change files or commits-only (ADR-0029 / `okm-64b.5`).

use std::io;
use std::path::Path;

use clap::Args;
use oakum::changeset::{is_bump_file_name, load_bump_files, MalformedBumpFile};
use oakum::plan::{BumpFile, PackageId, Workspace};
use serde::Serialize;

use super::add::discover_workspace;
use super::config::{load_config, LoadedConfig, PlanIntentSource};
use super::coverage::Standing;
use super::deliver_out;
use super::fs::{repo_path_display, resolve_capability_path};
use super::generate::{commit_intents, resolve_from_ref, CommitIntents};
use super::git::Git;
use super::repository::{self, Repository};
use super::write_set::read_text;
use super::CliError;

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
    config.validate_workspace_selection(&workspace)?;
    let git = Git::at_repository(&repo)?;
    let files = load_plan_bump_files(&git, &repo, &workspace, &config, args.from.as_deref())?;
    let report: Vec<PlanIntentReportFile> = files.iter().map(PlanIntentReportFile::from).collect();
    deliver_out(&serde_json::to_string_pretty(&report)?)
        .map_err(|err| CliError::undelivered("report", &err))?;
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

/// Plan bump-file inputs per ADR-0029 (change files, or one per commit). Does
/// not write.
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
    load_plan_intent(git, repo, workspace, config, from).map(|intent| intent.files)
}

/// The plan's inputs, and the packages commits named that the config keeps but
/// cannot version: intent that releases nothing, which only a report shows.
/// Excluded packages are left out, since an exclusion is a decision. Change
/// files name no such package here: the plan refuses one.
pub(super) struct PlanIntent {
    pub(super) files: Vec<BumpFile>,
    pub(super) named_unmanaged: Vec<PackageId>,
}

/// [`load_plan_bump_files`], keeping what commits-only intent left out.
pub(super) fn load_plan_intent(
    git: &Git,
    repo: &Repository,
    workspace: &Workspace,
    config: &LoadedConfig,
    from: Option<&str>,
) -> Result<PlanIntent, Box<dyn std::error::Error>> {
    match config.plan_intent_source()? {
        PlanIntentSource::ChangeFiles => Ok(PlanIntent {
            files: load_change_files(repo, workspace)?,
            named_unmanaged: Vec::new(),
        }),
        PlanIntentSource::CommitsOnly => {
            let from = resolve_from_ref(git, from)?;
            // A commit is not a decision to release: one touching a package the
            // config does not version names nothing, rather than a package
            // every later step refuses.
            let CommitIntents { intents, dropped } =
                commit_intents(git, workspace, &from, |package| {
                    config.version_managed(package)
                })?;
            let named_unmanaged = workspace
                .packages()
                .filter(|package| {
                    dropped.contains(package.id())
                        && config.standing(package) == Standing::Unmanaged
                })
                .map(|package| package.id().clone())
                .collect();
            let files = intents
                .into_iter()
                .map(|(hash, intent)| intent.bump_file(hash))
                .collect();
            Ok(PlanIntent {
                files,
                named_unmanaged,
            })
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

    refuse_malformed(&loaded.malformed)?;
    Ok(loaded.files)
}

/// The library collects malformed bodies; every command refuses them. A
/// skipped file turns a formatter's rewrite into a release that never happens.
pub(super) fn refuse_malformed(malformed: &[MalformedBumpFile]) -> Result<(), CliError> {
    if malformed.is_empty() {
        return Ok(());
    }
    let reports: Vec<String> = malformed.iter().map(ToString::to_string).collect();
    Err(CliError::new(reports.join("; also ")))
}
