//! Plan intent loader: change files or commits-only (ADR-0029 / `okm-64b.5`).

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use clap::Args;
use oakum::changeset::{is_bump_file_name, load_bump_files, MalformedBumpFile};
use oakum::plan::{BumpFile, PackageId, Workspace};
use oakum::state::{ReleaseState, UnresolvedScope};
use oakum::tags::{intent_base, latest_tags, IntentBase};
use serde::Serialize;

use super::add::discover_workspace;
use super::changelog::has_version_section;
use super::config::{
    load_config, named_by_a_commit, tag_managed_ids, unresolved_scope_line, LoadedConfig,
    PlanIntentSource,
};
use super::coverage::Standing;
use super::deliver_out;
use super::fs::{repo_path_display, resolve_capability_path};
use super::generate::{commit_intents, resolve_from_ref, CommitIntents, CommitRanges};
use super::git::Git;
use super::repository::{self, Repository};
use super::tags::reachable_tags;
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

/// The plan's inputs, and the release intent commits asked for that no plan
/// receives.
pub(super) struct PlanIntent {
    pub(super) files: Vec<BumpFile>,
    pub(super) dropped: DroppedIntent,
}

/// Intent the plan does not receive, which only a report shows. Change files
/// carry none: the plan refuses a file naming an unversioned package, a file
/// names packages, not scopes, and pending files are consumed, not reread.
#[derive(Debug, Default)]
pub(super) struct DroppedIntent {
    /// Packages the config keeps but cannot version. Excluded packages are left
    /// out, since an exclusion is a decision.
    pub(super) named_unmanaged: Vec<PackageId>,
    pub(super) unresolved: Vec<UnresolvedScope>,
    /// Released by `version` and not yet tagged, so commits before the branch
    /// are not read again.
    pub(super) awaiting_tag: Vec<PackageId>,
}

impl DroppedIntent {
    /// Said by `check` and on stderr by `version`, so the two cannot differ.
    pub(super) fn lines(&self) -> Vec<String> {
        self.named_unmanaged
            .iter()
            .map(named_by_a_commit)
            .chain(self.unresolved.iter().map(unresolved_scope_line))
            .collect()
    }

    pub(super) fn report(&self, state: ReleaseState) -> ReleaseState {
        state
            .with_named_unmanaged(self.named_unmanaged.clone())
            .with_unresolved_scopes(self.unresolved.clone())
            .with_awaiting_tag(self.awaiting_tag.clone())
    }
}

/// [`load_plan_bump_files`], keeping what commits-only intent left out.
pub(super) fn load_plan_intent(
    git: &Git,
    repo: &Repository,
    workspace: &Workspace,
    config: &LoadedConfig,
    from: Option<&str>,
) -> Result<PlanIntent, Box<dyn std::error::Error>> {
    load_intent(git, repo, workspace, config, from, None)
}

/// [`load_plan_intent`] for the coverage look, where a package awaiting its
/// tag reads the commits after `base`. Coverage plans nothing, so intent it
/// reads twice cannot release twice, and a branch's own fix still covers it.
pub(super) fn load_coverage_intent(
    git: &Git,
    repo: &Repository,
    workspace: &Workspace,
    config: &LoadedConfig,
    from: Option<&str>,
    base: &str,
) -> Result<PlanIntent, Box<dyn std::error::Error>> {
    load_intent(git, repo, workspace, config, from, Some(base))
}

fn load_intent(
    git: &Git,
    repo: &Repository,
    workspace: &Workspace,
    config: &LoadedConfig,
    from: Option<&str>,
    awaiting_from: Option<&str>,
) -> Result<PlanIntent, Box<dyn std::error::Error>> {
    match config.plan_intent_source()? {
        PlanIntentSource::ChangeFiles => Ok(PlanIntent {
            files: load_change_files(repo, workspace)?,
            dropped: DroppedIntent::default(),
        }),
        PlanIntentSource::CommitsOnly => {
            let (ranges, awaiting_tag) = match from {
                Some(from) => (
                    CommitRanges::from_base(&resolve_from_ref(git, Some(from))?, workspace),
                    Vec::new(),
                ),
                None => ranges_since_tags(git, repo, workspace, config, awaiting_from)?,
            };
            // A commit is not a decision to release: one touching a package the
            // config does not version names nothing, rather than a package
            // every later step refuses.
            let CommitIntents {
                intents,
                left_out,
                unresolved,
            } = commit_intents(git, workspace, &ranges, |package| {
                config.version_managed(package)
            })?;
            let named_unmanaged = workspace
                .packages()
                .filter(|package| {
                    left_out.contains(package.id())
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
                dropped: DroppedIntent {
                    named_unmanaged,
                    unresolved,
                    awaiting_tag,
                },
            })
        }
    }
}

/// Each package's commits since its highest reachable tag, and the packages
/// awaiting a tag, which read nothing unless `awaiting_from` names a base: only
/// the tag records what a release included. A package oakum does not tag keeps
/// the merge-base range.
fn ranges_since_tags(
    git: &Git,
    repo: &Repository,
    workspace: &Workspace,
    config: &LoadedConfig,
    awaiting_from: Option<&str>,
) -> Result<(CommitRanges, Vec<PackageId>), Box<dyn std::error::Error>> {
    let tag_managed = tag_managed_ids(workspace, config);
    let latest = if tag_managed.is_empty() {
        BTreeMap::new()
    } else {
        let groups = reachable_tags(git)?;
        let tags: Vec<Vec<&str>> = groups
            .iter()
            .map(|group| group.tags().iter().map(String::as_str).collect())
            .collect();
        let commits: Vec<(&str, &[&str])> = groups
            .iter()
            .zip(&tags)
            .map(|(group, tags)| (group.commit(), tags.as_slice()))
            .collect();
        latest_tags(&commits, workspace, |id| tag_managed.contains(id))
            .map_err(|err| CliError::unverified(err.to_string()))?
    };
    let mut merge_base: Option<String> = None;
    let mut since_merge_base = || -> Result<Option<String>, Box<dyn std::error::Error>> {
        if merge_base.is_none() {
            merge_base = Some(resolve_from_ref(git, None)?);
        }
        Ok(merge_base.clone())
    };
    let mut bases = BTreeMap::new();
    let mut awaiting_tag = Vec::new();
    for package in workspace.packages() {
        let base = if tag_managed.contains(package.id()) {
            match intent_base(package, latest.get(package.id()), || {
                has_version_section(repo.dir(), package)
            })? {
                IntentBase::Since(commit) => Some(commit),
                IntentBase::Root => None,
                IntentBase::AwaitingTag => {
                    if config.version_managed(package) {
                        awaiting_tag.push(package.id().clone());
                    }
                    let Some(base) = awaiting_from else {
                        continue;
                    };
                    Some(String::from(base))
                }
            }
        } else {
            since_merge_base()?
        };
        bases.insert(package.id().clone(), base);
    }
    Ok((CommitRanges::new(bases), awaiting_tag))
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
