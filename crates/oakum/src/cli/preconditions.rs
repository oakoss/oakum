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
use super::git::{Git, Op};
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
    // Before any look, so the runs that refuse describe themselves too.
    let scope = Scope::of(&git, &loaded, args);
    super::say_out(&scope.to_string());
    // Printed here and refused at the end, like the looks below: a config that
    // manages nothing is the most fundamental thing wrong with a repository,
    // but refusing on it first would hide a stale install pin or an unfinished
    // write, and a different oakum may not have this look at all.
    let management = evaluate_management(&loaded);
    let (looked, mut refusals) = evaluate_all(
        &git,
        &repo,
        &loaded,
        // The base the report named, resolved once. The failure travels too,
        // rather than being re-derived: a second resolution that succeeded
        // where the first did not would run a coverage look the report has
        // already said is not happening.
        scope.base.as_deref().map_err(String::as_str),
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
    // Pending tags are a finding like the others and compete with them on that
    // footing. Reached last, they lost to every unverified look ahead of them.
    let pending = looked.as_ref().and_then(|tags| {
        if !tags.is_clean() {
            report_pending(tags);
        }
        refuse_if_pending(tags).err()
    });
    refusals.extend(
        [changelogs.err(), staging.err(), management.err(), pending]
            .into_iter()
            .flatten(),
    );
    match carry(refusals) {
        Some(refusal) => Err(refusal),
        None => Ok(()),
    }
}

/// The looks `run` performs, in the order it announces them. A hand-written
/// sentence drifted silently in both directions — measured: naming a look that
/// does not exist, and dropping one that does, each left the whole suite green.
/// Deriving the sentence from this list closes the first; the second is
/// `okm-404.55`, which makes the list the thing `run` folds over.
const LOOKS: [&str; 6] = [
    "management",
    "tags",
    "install pin",
    "changelogs",
    "staging",
    "coverage",
];

/// An English list: comma-separated with a final `and`.
fn named(looks: &[&str]) -> String {
    match looks {
        [] => String::new(),
        [only] => (*only).to_owned(),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

/// What this run of `check` covers, said out loud before the looks begin. A
/// silent exit 0 collapses every look at once, which is what `AGENTS.md`
/// forbids — but so would claiming completion here, since this prints first so
/// that a run which refuses describes itself too. So it states the scope and
/// the intent, never the verdict: the looks asked for and the looks not asked
/// for are listed apart, and what became of them is the exit code's to say.
struct Scope {
    selected: usize,
    packages: usize,
    /// `Err` carries why the base could not be named. The coverage look raises
    /// the real failure; this only has to avoid claiming a ref it does not have.
    base: Result<String, String>,
    gating_coverage: bool,
    remote: bool,
}

impl Scope {
    fn of(git: &Git, loaded: &Loaded, args: &CheckArgs) -> Self {
        let Loaded { config, workspace } = loaded;
        let packages = workspace.packages().count();
        let selected = workspace
            .packages()
            .filter(|package| config.selected(&package.id().name))
            .count();
        // The selection is a subset of the workspace, which is structural here
        // and only site discipline for any later constructor: `9 of 1` renders
        // without complaint.
        debug_assert!(selected <= packages, "{selected} of {packages}");
        Self {
            selected,
            packages,
            base: resolved_base(git, args.from.as_deref()),
            gating_coverage: args.strict,
            remote: args.remote,
        }
    }
}

/// The base this run diffs from, confirmed before the report quotes it. An
/// explicit `--from` is taken verbatim by [`resolve_from_ref`], so the report
/// announced a ref git does not have and the diff then failed on it —
/// measured: `--from no-such-ref` announced that ref and the diff then failed
/// on it with `fatal: ambiguous argument`.
fn resolved_base(git: &Git, from: Option<&str>) -> Result<String, String> {
    let base = super::generate::resolve_from_ref(git, from).map_err(|err| err.to_string())?;
    match git.predicate(Op::RefExists { reference: &base }) {
        Ok(true) => Ok(base),
        Ok(false) => Err(format!("git has no `{base}`")),
        Err(err) => Err(err.detail()),
    }
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self {
            selected, packages, ..
        } = self;
        write!(f, "check: {selected} of {packages} package(s) selected")?;
        match &self.base {
            Ok(base) => write!(f, ", diffing from `{base}`")?,
            Err(why) => write!(f, "; no base ref to diff from ({why})")?,
        }
        write!(f, "\ncheck: looking at {}", named(&LOOKS))?;
        // A coverage look is a diff from a base. Dropping it from the list with
        // no base left the reader no line explaining the refusal that look then
        // raises, so it says why instead of going unmentioned.
        if self.base.is_err() {
            f.write_str("; coverage cannot run without a base ref")?;
        } else if !self.gating_coverage {
            f.write_str("; coverage reports without gating (`--strict` gates)")?;
        }
        if self.remote {
            return f.write_str("; looking at the remote");
        }
        f.write_str("; not looking at the remote (`--remote` asks for it)")
    }
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
        super::say_err(&stray_staging_message(path));
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
        super::say_err(report);
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
/// The tag state, for `release`. The gate's other looks are `check`'s alone, so
/// they are named here rather than passed in — `release` supplied `false, false,
/// 3` at its only call site, where the `3` was a lookback `evaluate_remote`
/// never read because the remote look was off.
pub(super) fn evaluate(
    git: &Git,
    repo: &Repository,
    from: Option<&str>,
) -> Result<TagEvaluation, CliError> {
    const NO_STRICT: bool = false;
    const NO_REMOTE: bool = false;
    const UNREAD_LOOKBACK: u32 = 1;
    let (strict, remote, remote_lookback) = (NO_STRICT, NO_REMOTE, UNREAD_LOOKBACK);
    let loaded = Loaded::load(repo)?;
    let base = resolved_base(git, from);
    evaluate_with(
        git,
        repo,
        &loaded,
        base.as_deref().map_err(String::as_str),
        strict,
        remote,
        remote_lookback,
    )
}

fn evaluate_with(
    git: &Git,
    repo: &Repository,
    loaded: &Loaded,
    base: Result<&str, &str>,
    strict: bool,
    remote: bool,
    remote_lookback: u32,
) -> Result<TagEvaluation, CliError> {
    let (looked, refusals) = evaluate_all(git, repo, loaded, base, strict, remote, remote_lookback);
    match carry(refusals) {
        Some(refusal) => Err(refusal),
        None => Ok(looked.expect("no refusal means the tag look answered")),
    }
}

/// What these three looks established, kept apart from which refusal would
/// carry the exit code. Collapsing them into one `Result` dropped a tag
/// evaluation that had answered whenever a sibling refused, so the pending-tag
/// refusal was never built — measured: real drift plus an unresolvable `--from`
/// printed neither the drift detail nor its summary, and exited 2.
fn evaluate_all(
    git: &Git,
    repo: &Repository,
    loaded: &Loaded,
    base: Result<&str, &str>,
    strict: bool,
    remote: bool,
    remote_lookback: u32,
) -> (Option<TagEvaluation>, Vec<CliError>) {
    // Bound rather than chained, for the reason `run` binds its own looks: a
    // stale install pin lives inside `evaluate_tags`, and returning on it would
    // hide the coverage refusal.
    let tags = evaluate_tags(git, repo, loaded);
    let coverage = evaluate_coverage(git, repo, loaded, base, strict);
    let remote = evaluate_remote(git, remote, remote_lookback);
    let (looked, tag_refusal) = match tags {
        Ok(looked) => (Some(looked), None),
        Err(refusal) => (None, Some(refusal)),
    };
    let mut refusals: Vec<CliError> = tag_refusal.into_iter().collect();
    refusals.extend(coverage);
    refusals.extend(remote.err());
    (looked, refusals)
}

/// Every refusal reported, and the one that decides the exit code chosen by
/// what it means rather than by where it sits in the source.
///
/// Binding the looks was only half the promise: a look whose whole refusal
/// travels in the `Err` — `--remote` against a repository with no remotes, a
/// coverage read that never reached the diff — printed nothing on its own, so
/// the first refusal erased it. Measured: `check --remote` with an unresolvable
/// base discarded `unverified: --remote set but this repository has no remotes`.
///
/// Order was the other half. `?` carries out whichever refusal is written
/// first, so an unrelated stray staging file turned a measured tag drift from
/// `error` (exit 1) into `unverified` (exit 2) and erased the `error:` line —
/// ADR-0034's split run backwards, exactly the collapse `migrate` reasons its
/// way out of. A finding outranks a look that did not happen; the rest are
/// marked `also` so a reader can see which line the exit code came from.
fn carry(refusals: Vec<CliError>) -> Option<CliError> {
    let mut refusals = refusals;
    // `min_by_key` returns the first minimum, so equal-severity refusals keep
    // source order and the empty case is the `?`.
    let deciding = refusals
        .iter()
        .enumerate()
        .min_by_key(|(_, refusal)| refusal.class())
        .map(|(index, _)| index)?;
    let chosen = refusals.remove(deciding);
    // Two looks can fail identically — the tag look and the coverage look both
    // run `rev-parse --is-shallow-repository` — and `also` reads as a second,
    // different problem. Say each distinct refusal once.
    let mut said = vec![chosen.to_string()];
    for also in &refusals {
        let line = also.to_string();
        if said.contains(&line) {
            continue;
        }
        super::say_err(&format!("also {}: {}", also.outcome(), also.detail()));
        said.push(line);
    }
    Some(chosen)
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
    super::say_err(ALL_PRIVATE_GUIDANCE);
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
        super::say_err(&format!(
            "{}: manifest {} is above tagged {} (local tags; run `git fetch --tags` if the remote is ahead)",
            item.id(),
            item.manifest(),
            item.tagged()
        ));
    }
    for (id, version) in &tags.untagged_ahead {
        super::say_err(&format!(
            "{id}: never released, but the manifest is {version}; tag the version you meant"
        ));
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
    base: Result<&str, &str>,
    strict: bool,
) -> Vec<CliError> {
    // The look's own early exits carry one refusal each; only its tail can
    // establish two at once.
    match coverage_refusals(git, repo, loaded, base, strict) {
        Ok(refusals) => refusals,
        Err(refusal) => vec![refusal],
    }
}

fn coverage_refusals(
    git: &Git,
    repo: &Repository,
    loaded: &Loaded,
    base: Result<&str, &str>,
    strict: bool,
) -> Result<Vec<CliError>, CliError> {
    // `unverified`, not an error: a base git does not have is a look that could
    // not happen, and ADR-0034 exists so a CI step can tell that from a
    // repository that failed a check. Routing it through `CliError::new` moved
    // a shallow-checkout `--from` off exit 2 and onto exit 1.
    let from = Some(base.map_err(|why| CliError::unverified(format!("unverified: {why}")))?);
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
            super::say_err(&format!("{id}: changed with no covering intent; {hint}"));
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
            super::say_err(&intent_names_unmanaged(&id.name));
        }
    }
    // Both refusals travel: ranking, `also` marking and duplicate suppression
    // all belong to `carry`, and a second printer here skipped the last of them.
    Ok([
        named_unmanaged
            .first()
            .map(|id| CliError::new(intent_names_unmanaged(&id.name))),
        (strict && !uncovered.is_empty()).then(|| CliError::uncovered(uncovered.len())),
    ]
    .into_iter()
    .flatten()
    .collect())
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

    /// The sentence names every look and invents none. A literal drifted in
    /// both directions with the suite green: `submodules` announced as a look
    /// that does not exist, and `staging` dropped while it still ran.
    #[test]
    fn the_announcement_names_every_look() {
        let sentence = super::named(&super::LOOKS);
        for look in super::LOOKS {
            assert!(
                sentence.contains(look),
                "{look} is not announced: {sentence}"
            );
        }
        // A list, not a run-on: the last item takes the `and`.
        assert!(sentence.ends_with("and coverage"), "{sentence}");
        assert_eq!(sentence.matches(", and ").count(), 1, "{sentence}");
        assert_eq!(
            sentence.split(", ").count(),
            super::LOOKS.len(),
            "one item per look: {sentence}"
        );
    }
}
