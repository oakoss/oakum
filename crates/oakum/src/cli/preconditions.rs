//! Shared readiness path (ADR-0020). Reports drift and names the fix; never
//! applies it (ADR-0003).

use std::collections::BTreeSet;

use clap::Args;

use oakum::plan::PackageId;
use oakum::tags::Drift;
use semver::Version;

use super::config::{load_config, PlanIntentSource};
use super::coverage;
use super::install_pin;
use super::intent::load_plan_bump_files;
use super::repository::{self, Repository};
use super::tags::{self, CommitTags};
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
    fn new(id: PackageId, version: Version) -> Self {
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
    refuse_if_pending(&evaluate(
        &repo,
        args.from.as_deref(),
        args.strict,
        args.remote,
        args.remote_lookback,
    )?)
}

pub(super) fn run_tags_only() -> Result<(), CliError> {
    let repo = repository::discover().map_err(CliError::from_boxed)?;
    refuse_if_pending(&evaluate_tags(&repo)?)
}

/// Ok even when tags are pending; `check` refuses that case.
pub(super) fn evaluate(
    repo: &Repository,
    from: Option<&str>,
    strict: bool,
    remote: bool,
    remote_lookback: u32,
) -> Result<TagEvaluation, CliError> {
    let tags = evaluate_tags(repo)?;
    evaluate_coverage(repo, from, strict)?;
    evaluate_remote(repo, remote, remote_lookback)?;
    Ok(tags)
}

fn refuse_if_pending(tags: &TagEvaluation) -> Result<(), CliError> {
    if tags.is_clean() {
        return Ok(());
    }
    report_pending(tags);
    Err(CliError::tag_drift(
        tags.drift.len() + tags.untagged_ahead.len(),
    ))
}

fn report_pending(tags: &TagEvaluation) {
    for item in &tags.drift {
        eprintln!(
            "{}: manifest {} is above tagged {}",
            item.id(),
            item.manifest(),
            item.tagged()
        );
    }
    for (id, version) in &tags.untagged_ahead {
        eprintln!("{id}: never released, but the manifest is {version}; tag the version you meant");
    }
}

fn evaluate_tags(repo: &Repository) -> Result<TagEvaluation, CliError> {
    let config = load_config(repo).map_err(CliError::from_boxed)?;
    if let Some(expected) = config.tool_version() {
        install_pin::verify(repo.dir(), expected)?;
    }
    let _ = config.plan_intent_source()?;
    let groups = tags::reachable_tags(repo.path())?;
    let workspace = add::discover_workspace(repo.path()).map_err(CliError::from_boxed)?;
    let owned: Vec<Vec<&str>> = groups
        .iter()
        .map(CommitTags::tags)
        .map(|tags| tags.iter().map(String::as_str).collect())
        .collect();
    let slices: Vec<&[&str]> = owned.iter().map(Vec::as_slice).collect();
    let tagged = oakum::tags::current_versions(&slices, &workspace)
        .map_err(|err| CliError::unverified(err.to_string()))?;
    Ok(TagEvaluation {
        drift: oakum::tags::drift(&workspace, &tagged),
        untagged_ahead: oakum::tags::untagged_ahead(&workspace, &tagged),
        current: oakum::tags::tagged_current(&workspace, &tagged),
    })
}

fn evaluate_coverage(repo: &Repository, from: Option<&str>, strict: bool) -> Result<(), CliError> {
    let config = load_config(repo).map_err(CliError::from_boxed)?;
    let workspace = add::discover_workspace(repo.path()).map_err(CliError::from_boxed)?;
    let files = load_plan_bump_files(repo.path(), &workspace, &config, from)
        .map_err(CliError::from_boxed)?;
    let uncovered = coverage::uncovered_packages(repo.path(), &workspace, &files, from)?;
    if uncovered.is_empty() {
        return Ok(());
    }
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
    if strict {
        return Err(CliError::uncovered(uncovered.len()));
    }
    Ok(())
}

fn evaluate_remote(repo: &Repository, remote: bool, remote_lookback: u32) -> Result<(), CliError> {
    if !remote {
        return Ok(());
    }
    let Some(remote) = tags::first_remote(repo.path())? else {
        return Err(CliError::unverified(
            "unverified: --remote set but this repository has no remotes",
        ));
    };
    let advertised = tags::remote_tag_names(repo.path(), &remote)?;
    let local = tags::reachable_tags(repo.path())?;
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
