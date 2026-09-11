//! Shared readiness path (ADR-0020). Reports drift and names the fix; never
//! applies it (ADR-0003).

use std::collections::BTreeSet;

use clap::Args;

use oakum::plan::{PackageId, Workspace};
use oakum::tags::Drift;
use semver::Version;

use super::changelog;
use super::config::{
    intent_names_unmanaged, load_config, require_config, tag_managed_ids, LoadedConfig,
    PlanIntentSource, ALL_PRIVATE_GUIDANCE,
};
use super::coverage;
use super::fs::{repo_path_display, stray_staging_files, stray_staging_message};
use super::git::Git;
use super::install_pin;
use super::intent::load_plan_bump_files;
use super::repository::{self, Repository};
use super::tags::{self, CommitTags};
use super::version::extra_file_repo_path;
use super::{add, CliError};

/// `pending` is drift ∪ untagged-ahead; `current` matches a reachable tag.
#[derive(Debug)]
pub(super) struct TagEvaluation {
    drift: Vec<Drift>,
    untagged_ahead: Vec<(PackageId, Version)>,
    current: Vec<(PackageId, Version)>,
}

#[derive(Clone, Debug)]
pub(super) struct PendingRelease {
    id: PackageId,
    version: Version,
}

impl PendingRelease {
    #[must_use]
    pub(super) fn new(id: PackageId, version: Version) -> Self {
        Self { id, version }
    }

    #[must_use]
    pub(super) fn id(&self) -> &PackageId {
        &self.id
    }

    #[must_use]
    pub(super) fn version(&self) -> &Version {
        &self.version
    }
}

impl TagEvaluation {
    #[must_use]
    pub(super) fn is_clean(&self) -> bool {
        self.drift.is_empty() && self.untagged_ahead.is_empty()
    }

    #[must_use]
    pub(super) fn pending(&self) -> Vec<PendingRelease> {
        let mut pending: Vec<PendingRelease> = self
            .drift
            .iter()
            .map(|item| PendingRelease::new(item.id().clone(), item.manifest().clone()))
            .chain(
                self.untagged_ahead
                    .iter()
                    .map(|(id, version)| PendingRelease::new(id.clone(), version.clone())),
            )
            .collect();
        pending.sort_by(|left, right| left.id.cmp(&right.id));
        pending
    }

    #[must_use]
    pub(super) fn current(&self) -> Vec<PendingRelease> {
        let mut current: Vec<PendingRelease> = self
            .current
            .iter()
            .map(|(id, version)| PendingRelease::new(id.clone(), version.clone()))
            .collect();
        current.sort_by(|left, right| left.id.cmp(&right.id));
        current
    }
}

#[derive(Debug, Args)]
pub(super) struct CheckArgs {
    /// Fail when a changed package is not named by the enabled intent mechanism.
    #[arg(long)]
    strict: bool,
    /// Git ref to diff from (exclusive). Same default as `generate` / `status`.
    #[arg(long, value_name = "REF")]
    from: Option<String>,
    /// Fail when newest local tags are missing from the remote (ADR-0016).
    #[arg(long)]
    remote: bool,
    /// How many of the newest local tags `--remote` requires on the remote.
    #[arg(
        long,
        default_value_t = 3,
        value_name = "N",
        value_parser = clap::value_parser!(u32).range(1..=20),
        requires = "remote"
    )]
    remote_lookback: u32,
}

pub(super) fn run(args: &CheckArgs) -> Result<(), CliError> {
    let repo = repository::discover().map_err(CliError::from_boxed)?;
    let config = load_config(&repo).map_err(CliError::from_boxed)?;
    require_config(&config)?;
    let loaded = Loaded::discover(&repo, config)?;
    let git = Git::at_repository(&repo).map_err(CliError::from_boxed)?;
    // Printed here and refused at the end, like the looks below: a config that
    // manages nothing is the most fundamental thing wrong with a repository,
    // but refusing on it first would hide a stale install pin or an unfinished
    // write, and a different oakum may not have this look at all.
    let management = evaluate_management(&loaded);
    let evaluated = evaluate_with(
        &git,
        &repo,
        &loaded,
        args.from.as_deref(),
        args.strict,
        args.remote,
        args.remote_lookback,
    );
    // Every look prints before the first refusal returns, so one unverified
    // state does not hide another. This result is held for the same reason: a
    // stale install pin lives inside it, and returning on one would hide the
    // unfinished write the next look names.
    let changelogs = evaluate_changelogs(&repo, &loaded);
    let staging = evaluate_staging(&repo, &loaded);
    let tags = evaluated?;
    if !tags.is_clean() {
        report_pending(&tags);
    }
    changelogs?;
    staging?;
    management?;
    refuse_if_pending(&tags)
}

/// A staging file means a write did not finish and the file it was replacing
/// may be stale; that is unverified, not clean. The directories are the ones
/// `version` stages into: `.changeset/`, the repository root (lockfiles),
/// every package directory (manifest, changelog), and every declared
/// extra-file's directory.
fn evaluate_staging(repo: &Repository, loaded: &Loaded) -> Result<(), CliError> {
    let mut dirs: BTreeSet<String> = [String::from(".changeset"), String::from(".")].into();
    for package in loaded.workspace.packages() {
        let dir = package.manifest_dir();
        if !dir.is_empty() {
            dirs.insert(dir.to_owned());
        }
        for extra in loaded.config.extra_files_for(&package.id().name) {
            let path = extra_file_repo_path(package, extra.path()).map_err(CliError::from_boxed)?;
            let parent = path.parent().map(repo_path_display).unwrap_or_default();
            dirs.insert(if parent.is_empty() {
                String::from(".")
            } else {
                parent
            });
        }
    }
    let mut strays = Vec::new();
    for sub in &dirs {
        strays.extend(stray_staging_files(repo.dir(), sub).map_err(CliError::from_boxed)?);
    }
    if strays.is_empty() {
        return Ok(());
    }
    for path in &strays {
        eprintln!("{}", stray_staging_message(path));
    }
    Err(CliError::unverified(format!(
        "unverified: {} oakum staging file(s) left behind",
        strays.len()
    )))
}

/// Config and workspace, read once per run: discovery shells out, and every
/// look here needs the same two.
struct Loaded {
    config: LoadedConfig,
    workspace: Workspace,
}

impl Loaded {
    fn load(repo: &Repository) -> Result<Self, CliError> {
        let config = load_config(repo).map_err(CliError::from_boxed)?;
        Self::discover(repo, config)
    }

    fn discover(repo: &Repository, config: LoadedConfig) -> Result<Self, CliError> {
        let workspace = add::discover_workspace(repo).map_err(CliError::from_boxed)?;
        config.validate_workspace_selection(&workspace)?;
        Ok(Self { config, workspace })
    }
}

/// A changelog `version` would refuse to splice is drift `check` can see
/// before the version job fails in CI. Only `check` asks: `release` never
/// reads the title line; it takes the `## <version>` section and falls back
/// to the release title only when that section is missing or empty.
fn evaluate_changelogs(repo: &Repository, loaded: &Loaded) -> Result<(), CliError> {
    let reports = changelog::foreign_changelogs(repo.dir(), &loaded.workspace, |package| {
        loaded.config.version_managed(package)
    })
    .map_err(|err| CliError::unverified(format!("unverified: {err}")))?;
    if reports.is_empty() {
        return Ok(());
    }
    for report in &reports {
        eprintln!("{report}");
    }
    Err(CliError::unverified(format!(
        "unverified: {} changelog(s) `oakum version` would refuse to append to",
        reports.len()
    )))
}

pub(super) fn run_tags_only() -> Result<(), CliError> {
    let repo = repository::discover().map_err(CliError::from_boxed)?;
    let git = Git::at_repository(&repo).map_err(CliError::from_boxed)?;
    let tags = evaluate_tags(&git, &repo, &Loaded::load(&repo)?)?;
    if !tags.is_clean() {
        report_pending(&tags);
    }
    refuse_if_pending(&tags)
}

/// Ok even when tags are pending; `check` refuses that case.
pub(super) fn evaluate(
    git: &Git,
    repo: &Repository,
    from: Option<&str>,
    strict: bool,
    remote: bool,
    remote_lookback: u32,
) -> Result<TagEvaluation, CliError> {
    let loaded = Loaded::load(repo)?;
    evaluate_with(git, repo, &loaded, from, strict, remote, remote_lookback)
}

fn evaluate_with(
    git: &Git,
    repo: &Repository,
    loaded: &Loaded,
    from: Option<&str>,
    strict: bool,
    remote: bool,
    remote_lookback: u32,
) -> Result<TagEvaluation, CliError> {
    // Bound rather than chained, for the reason `run` binds its own looks: a
    // stale install pin lives inside `evaluate_tags`, and returning on it would
    // hide the coverage refusal.
    let tags = evaluate_tags(git, repo, loaded);
    let coverage = evaluate_coverage(git, repo, loaded, from, strict);
    let remote = evaluate_remote(git, remote, remote_lookback);
    let tags = tags?;
    coverage?;
    remote?;
    Ok(tags)
}

/// `include`/`exclude` left nothing selected, so no plan can name a package.
///
/// Kept apart from [`manages_nothing`], which returns `false` here: the two are
/// different facts about a config that can never release, and `check` is silent
/// on this one because emptying a selection is a decision someone wrote down.
pub(super) fn selection_is_empty(config: &LoadedConfig, workspace: &Workspace) -> bool {
    !workspace
        .packages()
        .any(|package| config.selected(&package.id().name))
}

/// The state [`evaluate_management`] refuses on, as a question. `status`
/// reports it rather than refusing, and both must decide it the same way.
pub(super) fn manages_nothing(config: &LoadedConfig, workspace: &Workspace) -> bool {
    let mut selected = workspace
        .packages()
        .filter(|package| config.selected(&package.id().name))
        .peekable();
    if selected.peek().is_none() {
        return false;
    }
    !selected.any(|package| config.version_managed(package) || config.tag_managed(package))
}

/// A config under which no selected package can produce work on either axis.
/// Every plan is then empty and every release a no-op that reports success.
/// Measured in a repository whose members are all `private` and whose migrated
/// config left `private-packages` at its default.
///
/// An empty selection stays silent. It produces the same empty plan, but
/// `include` and `exclude` are the only keys that empty one, so it is a
/// decision someone wrote down, and the invariant against collapsing "we
/// didn't look" into "it's fine" does not reach a user who said not to look. A package left in the selection
/// and unmanaged by default had no decision written about it, and that is the
/// state worth reporting. Both axes count, because [ADR-0027] makes them
/// independent and a repository that only tags its private packages is doing
/// real work.
///
/// `check` asks; `release` does not, because a release with nothing to do says
/// so in its own words.
///
/// [ADR-0027]: ../../../docs/decisions/0027-private-packages-version-opt-in.md
fn evaluate_management(loaded: &Loaded) -> Result<(), CliError> {
    let Loaded { config, workspace } = loaded;
    if !manages_nothing(config, workspace) {
        return Ok(());
    }
    // The guidance prints; the refusal stays short, like every sibling look.
    eprintln!("{ALL_PRIVATE_GUIDANCE}");
    Err(CliError::unverified(String::from(
        "unverified: this config manages no package on either axis",
    )))
}

fn refuse_if_pending(tags: &TagEvaluation) -> Result<(), CliError> {
    if tags.is_clean() {
        return Ok(());
    }
    Err(CliError::tag_drift(
        tags.drift.len() + tags.untagged_ahead.len(),
    ))
}

fn report_pending(tags: &TagEvaluation) {
    for item in &tags.drift {
        eprintln!(
            "{}: manifest {} is above tagged {} (local tags; run `git fetch --tags` if the remote is ahead)",
            item.id(),
            item.manifest(),
            item.tagged()
        );
    }
    for (id, version) in &tags.untagged_ahead {
        eprintln!("{id}: never released, but the manifest is {version}; tag the version you meant");
    }
}

fn evaluate_tags(git: &Git, repo: &Repository, loaded: &Loaded) -> Result<TagEvaluation, CliError> {
    let _ = repo.ambient_path().map_err(CliError::from_boxed)?;
    let Loaded { config, workspace } = loaded;
    if let Some(expected) = config.tool_version() {
        install_pin::verify(repo.dir(), expected)?;
    }
    let _ = config.plan_intent_source()?;
    let groups = tags::reachable_tags(git)?;
    let owned: Vec<Vec<&str>> = groups
        .iter()
        .map(CommitTags::tags)
        .map(|tags| tags.iter().map(String::as_str).collect())
        .collect();
    let slices: Vec<&[&str]> = owned.iter().map(Vec::as_slice).collect();
    let bare_candidates = tag_managed_ids(workspace, config);
    let tagged =
        oakum::tags::current_versions(&slices, workspace, |id| bare_candidates.contains(id))
            .map_err(|err| CliError::unverified(err.to_string()))?;
    Ok(TagEvaluation {
        drift: oakum::tags::drift(workspace, &tagged, |package| config.tag_managed(package)),
        untagged_ahead: oakum::tags::untagged_ahead(workspace, &tagged, |package| {
            config.tag_managed(package)
        }),
        current: oakum::tags::tagged_current(workspace, &tagged, |package| {
            config.tag_managed(package)
        }),
    })
}

/// Packages a bump file names that the config cannot version-manage, whether
/// the selection dropped them or the opt-in is unset. `status` reaches this
/// through `retain_managed` while composing a plan; `check` composes none, so
/// it asks the question directly.
fn intent_named_unmanaged(
    config: &LoadedConfig,
    workspace: &Workspace,
    files: &[oakum::plan::BumpFile],
) -> Vec<PackageId> {
    let mut named = BTreeSet::new();
    for file in files {
        for (id, _) in &file.entries {
            // Not `standing`: `retain_managed` refuses on any intent-named
            // package it cannot version, an excluded one included, and a gate
            // that asked a narrower question than the pipeline would pass a
            // tree `status` and `version` both reject.
            if workspace
                .get(id)
                .is_some_and(|package| !config.version_managed(package))
            {
                named.insert(id.clone());
            }
        }
    }
    named.into_iter().collect()
}

fn evaluate_coverage(
    git: &Git,
    repo: &Repository,
    loaded: &Loaded,
    from: Option<&str>,
    strict: bool,
) -> Result<(), CliError> {
    let Loaded { config, workspace } = loaded;
    let files =
        load_plan_bump_files(git, repo, workspace, config, from).map_err(CliError::from_boxed)?;
    // The unmanaged half is `status`'s to report: a package the config cannot
    // plan is information, not a decision, and ADR-0027 records private-package
    // silence as something a changesets migratee keeps without a config change.
    // `check` gates; it reads only the coverage half.
    let oakum::state::Coverage { uncovered, .. } =
        coverage::changed_by_standing(git, workspace, &files, from, |package| {
            config.standing(package)
        })?;
    let named_unmanaged = intent_named_unmanaged(config, workspace, &files);
    if !uncovered.is_empty() {
        let hint = match config.plan_intent_source()? {
            PlanIntentSource::ChangeFiles => {
                "add a bump file (or `none` / empty frontmatter under --strict)"
            }
            PlanIntentSource::CommitsOnly => {
                "name the package in a conventional commit (or a path that maps to it)"
            }
        };
        for id in &uncovered {
            eprintln!("{id}: changed with no covering intent; {hint}");
        }
    }
    // Printed before it is returned, like every sibling look: the refusal
    // travels in the `Err`, and `evaluate_with` checks the tag look first, so
    // a stale install pin would otherwise erase this entirely. Every offender
    // is named, not just the one the `Err` carries.
    // The refusal below carries the first offender, so listing is only worth it
    // when there is more than one.
    if named_unmanaged.len() > 1 {
        for id in &named_unmanaged {
            eprintln!("{}", intent_names_unmanaged(&id.name));
        }
    }
    if let Some(id) = named_unmanaged.first() {
        return Err(CliError::new(intent_names_unmanaged(&id.name)));
    }
    if strict && !uncovered.is_empty() {
        return Err(CliError::uncovered(uncovered.len()));
    }
    Ok(())
}

fn evaluate_remote(git: &Git, remote: bool, remote_lookback: u32) -> Result<(), CliError> {
    if !remote {
        return Ok(());
    }
    let Some(remote) = tags::first_remote(git)? else {
        return Err(CliError::unverified(
            "unverified: --remote set but this repository has no remotes",
        ));
    };
    let advertised = tags::remote_tag_names(git, &remote)?;
    let local = tags::reachable_tags(git)?;
    let local_names: BTreeSet<String> = local.iter().flat_map(CommitTags::tags).cloned().collect();
    if local_names.is_empty() {
        if advertised.is_empty() {
            return Ok(());
        }
        return Err(CliError::unverified(format!(
            "unverified: remote {remote:?} advertises tags but none are reachable locally; \
             run `git fetch --tags -- {remote}` (a prior fetch --no-tags leaves no local tagOpt to detect)"
        )));
    }
    let lookback = remote_lookback as usize;
    let missing: Vec<String> = newest_local_tags(&local_names, lookback)
        .into_iter()
        .filter(|name| !advertised.contains(name))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(CliError::unverified(format!(
        "unverified: newest local tags missing from remote {remote:?}: {}; \
         push the tags (`git push --tags -- {remote}`) or confirm the remote did not drop them",
        missing.join(", ")
    )))
}

fn newest_local_tags(local: &BTreeSet<String>, n: usize) -> Vec<String> {
    let mut tags: Vec<String> = local.iter().cloned().collect();
    tags.sort_by(|left, right| compare_tag_names(left, right));
    let skip = tags.len().saturating_sub(n);
    tags.into_iter().skip(skip).collect()
}

fn compare_tag_names(left: &str, right: &str) -> std::cmp::Ordering {
    match (tag_version(left), tag_version(right)) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => std::cmp::Ordering::Greater,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (None, None) => left.cmp(right),
    }
}

fn tag_version(name: &str) -> Option<semver::Version> {
    oakum::tags::version_from_tag(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_local_tags_orders_changeset_and_hyphen_shapes() {
        let names = BTreeSet::from([
            "demo@0.9.0".into(),
            "demo@0.10.0".into(),
            "demo-v0.11.0".into(),
            "other/v0.8.0".into(),
        ]);
        assert_eq!(
            newest_local_tags(&names, 3),
            vec![
                "demo@0.9.0".to_string(),
                "demo@0.10.0".to_string(),
                "demo-v0.11.0".to_string(),
            ]
        );
    }

    #[test]
    fn newest_local_tags_keeps_hyphen_prerelease_starting_with_v() {
        let names = BTreeSet::from([
            "demo@0.9.0".into(),
            "demo@0.10.0".into(),
            "demo-v1.0.0-v1".into(),
            "other/v0.8.0".into(),
        ]);
        assert_eq!(
            newest_local_tags(&names, 3),
            vec![
                "demo@0.9.0".to_string(),
                "demo@0.10.0".to_string(),
                "demo-v1.0.0-v1".to_string(),
            ]
        );
    }
}
