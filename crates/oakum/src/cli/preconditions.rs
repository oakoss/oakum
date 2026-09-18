//! Shared readiness path (ADR-0020). Reports drift and names the fix; never
//! applies it (ADR-0003).

use std::collections::BTreeSet;

use clap::Args;

use oakum::plan::{PackageId, Workspace};
use oakum::tags::Drift;
use semver::Version;

use super::changelog;
use super::check_report::{CheckReport, ScopeRow};
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
use super::verdict::{carry, first_line, named, LookReport, Refusal, Verdict};
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
    /// Print the versioned `CheckReport` JSON document instead of the report.
    /// Refusals still reach stderr, and a run that refuses before the first
    /// look writes no document (ADR-0036).
    #[arg(long)]
    json: bool,
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
    let plan = LookPlan::check(args.remote);
    let scope = Scope::of(&git, &loaded, args, &plan);
    if !args.json {
        super::say_out(&scope.to_string());
    }
    let context = LookContext {
        git: &git,
        repo: &repo,
        loaded: &loaded,
        // The base the report named, resolved once. The failure travels too,
        // rather than being re-derived: a second resolution that succeeded
        // where the first did not would run a coverage look the report has
        // already said is not happening.
        base: scope.base.as_deref().map_err(String::as_str),
        strict: Some(args.strict),
        remote_lookback: Some(args.remote_lookback),
    };
    if !args.json {
        return plan.decide(&context).map(|_| ());
    }
    // The document is the report, so it is written whether or not the run
    // refuses; the refusal still reaches stderr and still sets the exit code.
    let (verdict, evaluation) = plan.look(&context);
    for (_, line) in verdict.said() {
        super::say_err(line);
    }
    let report = CheckReport::of(
        &verdict,
        scope.row(),
        &plan.names(),
        &LookPlan::check(true).names(),
    );
    let written = serde_json::to_string_pretty(&report)
        .map_err(|err| CliError::unverified(format!("the check report could not be built: {err}")))
        .and_then(|document| {
            super::deliver_out(&document).map_err(|err| CliError::undelivered("report", &err))
        });
    // The run's own verdict keeps its class. A delivery that failed after a
    // finding was established must not restate the run as unverified — that is
    // ADR-0034's split run backwards, the inversion `carry` was written to stop
    // one layer down.
    match (plan.settle(verdict.error, evaluation), written) {
        (Err(established), Err(undelivered)) => Err(established.recast(format!(
            "{}\nalso {}: {}",
            established.detail(),
            undelivered.outcome(),
            undelivered.detail()
        ))),
        (settled, Ok(())) => settled.map(|_| ()),
        (Ok(_), Err(undelivered)) => Err(undelivered),
    }
}

/// One look: its name, as the scope report announces it, and what it does.
/// The report is derived from this table, so it cannot name a look that does
/// not run; the fold walks it in this order, so the announced order is the
/// execution order and the tie-break order — measured before the table: a
/// `submodules` look announced but never run, and a stale install pin
/// deciding the exit code over a git that could not run at all.
#[derive(Clone, Copy)]
struct Look {
    name: &'static str,
    run: fn(&LookContext<'_>) -> LookReport,
}

/// What every look reads, read once per run.
struct LookContext<'a> {
    git: &'a Git,
    repo: &'a Repository,
    loaded: &'a Loaded,
    /// `Err` carries why the base could not be named.
    base: Result<&'a str, &'a str>,
    /// Read by the coverage look alone. `None` when the caller runs no
    /// coverage look, so a caller cannot invent a gate nobody asked for.
    strict: Option<bool>,
    /// Read by the remote look alone, and asked for only by `--remote`.
    remote_lookback: Option<u32>,
}

/// The tag look, passed apart from the others because exactly one runs and
/// the caller that needs its value must not have to ask whether it was
/// filled — `check` and `tag-drift` discard it; `release` reads it. As one
/// of [`Look`] it was a channel
/// any look could fill and none had to: the fill was asserted rather than
/// typed, and a look table without a tag look compiled and panicked on every
/// clean run (measured: 117 of 132 release tests failed on that mutant).
#[derive(Clone, Copy)]
struct TagLook {
    name: &'static str,
    run: fn(&LookContext<'_>) -> Result<(TagEvaluation, LookReport), CliError>,
}

/// The looks either side of the tag look. [`LookPlan::check`] composes the
/// order and carries the reasoning for it.
const BEFORE_TAGS: [Look; 1] = [MANAGEMENT];
const AFTER_TAGS: [Look; 4] = [INSTALL_PIN, CHANGELOGS, STAGING, COVERAGE];

/// The looks one run performs, in order. The scope report names these and
/// [`Self::decide`] runs these, so the announcement cannot name a look that
/// does not run — the property the single table used to carry, and the one a
/// separately-composed name list silently lost (measured: dropping the tag
/// look from that list passed all 588 unit tests).
struct LookPlan {
    before: &'static [Look],
    tag: TagLook,
    after: Vec<Look>,
}

impl LookPlan {
    /// `check`'s own order. Management is the most fundamental thing wrong
    /// with a repository, but it is one look among several: refusing on it
    /// alone would hide a stale install pin or an unfinished write. Tags run
    /// before the install pin so that a git which cannot run at all is the
    /// first line a reader meets, not a pin string that differs.
    fn check(remote: bool) -> Self {
        Self {
            before: &BEFORE_TAGS,
            tag: TAGS,
            after: AFTER_TAGS
                .into_iter()
                .chain(remote.then_some(REMOTE))
                .collect(),
        }
    }

    /// `tag-drift` is `check`'s tag look and the pin, in `check`'s order.
    fn tag_drift() -> Self {
        Self {
            before: &[],
            tag: TAGS,
            after: Vec::from([INSTALL_PIN]),
        }
    }

    /// The tag state `release` gates on: the evaluation, the pin and coverage.
    fn release() -> Self {
        Self {
            before: &[],
            tag: RELEASE_TAGS,
            after: Vec::from([INSTALL_PIN, COVERAGE]),
        }
    }

    fn names(&self) -> Vec<&'static str> {
        self.before
            .iter()
            .map(|look| look.name)
            .chain(std::iter::once(self.tag.name))
            .chain(self.after.iter().map(|look| look.name))
            .collect()
    }

    /// Whether this plan runs the look `--remote` asks for. Read off the
    /// plan rather than the flag, so the sentence describes what runs.
    fn runs_remote(&self) -> bool {
        self.names().contains(&REMOTE.name)
    }

    /// The looks the report lists. The remote look is left out because the
    /// sentence gives it its own clause either way — naming it here too said
    /// it twice. Still derived from the plan, so it cannot name a look that
    /// does not run, or omit one that does beyond the one it hands on.
    fn listed_names(&self) -> Vec<&'static str> {
        self.names()
            .into_iter()
            .filter(|name| *name != REMOTE.name)
            .collect()
    }
}

const MANAGEMENT: Look = Look {
    name: "management",
    run: |context| evaluate_management(context.loaded),
};

const TAGS: TagLook = TagLook {
    name: "tags",
    run: look_tags_and_pending,
};

/// The tag state `release` reads: the same evaluation, without `check`'s
/// pending refusal — pending tags are what `release` is for.
const RELEASE_TAGS: TagLook = TagLook {
    name: "tags",
    run: |context| {
        Ok((
            evaluate_tags(context.git, context.repo, context.loaded)?,
            LookReport::default(),
        ))
    },
};

const INSTALL_PIN: Look = Look {
    name: "install pin",
    run: |context| LookReport::from_result(evaluate_install_pin(context.repo, context.loaded)),
};

const CHANGELOGS: Look = Look {
    name: "changelogs",
    run: |context| evaluate_changelogs(context.repo, context.loaded),
};

const STAGING: Look = Look {
    name: "staging",
    run: |context| evaluate_staging(context.repo, context.loaded),
};

const COVERAGE: Look = Look {
    name: "coverage",
    run: |context| match context.strict {
        Some(strict) => evaluate_coverage(
            context.git,
            context.repo,
            context.loaded,
            context.base,
            strict,
        ),
        // Unreachable while every table carrying this look is built beside a
        // caller that decides: a refusal rather than a panic, because the
        // look not running is a look that did not happen.
        None => LookReport::from_result(Err(CliError::unverified(
            "unverified: the coverage look ran without a strictness decision, so it did not look",
        ))),
    },
};

/// Asked for by `--remote`, and announced by the scope report's own clause.
const REMOTE: Look = Look {
    name: "remote",
    run: |context| {
        LookReport::from_result(match context.remote_lookback {
            Some(lookback) => evaluate_remote(context.git, lookback),
            None => Err(CliError::unverified(
                "unverified: the remote look ran without a lookback, so it did not look",
            )),
        })
    },
};

/// Pending tags are a finding like the others and compete with them on that
/// footing; the detail travels with the refusal, one block.
fn look_tags_and_pending(
    context: &LookContext<'_>,
) -> Result<(TagEvaluation, LookReport), CliError> {
    let tags = evaluate_tags(context.git, context.repo, context.loaded)?;
    let refusals = refuse_if_pending(&tags)
        .err()
        .map(|error| Refusal {
            error,
            lines: pending_lines(&tags),
        })
        .into_iter()
        .collect();
    Ok((tags, LookReport::refusing(refusals)))
}

/// Every look runs and every report is kept, so one unverified state does not
/// hide another — measured before the fold: a stale install pin returned early
/// and erased the coverage refusal, and a refused sibling dropped a tag
/// evaluation that had answered. Reports are said, the verdict is returned.
impl LookPlan {
    /// Run every look and fold the reports. Separate from [`Self::decide`] so
    /// `check --json` can render the same `Verdict` the prose is rendered from,
    /// rather than a second traversal that could disagree with it (ADR-0036).
    fn look(&self, context: &LookContext<'_>) -> (Verdict, Option<TagEvaluation>) {
        let mut reports = Vec::new();
        for look in self.before {
            reports.push((look.name, (look.run)(context)));
        }
        let evaluation = match (self.tag.run)(context) {
            Ok((tags, report)) => {
                reports.push((self.tag.name, report));
                Some(tags)
            }
            Err(refusal) => {
                reports.push((self.tag.name, LookReport::from_result(Err(refusal))));
                None
            }
        };
        for look in &self.after {
            reports.push((look.name, (look.run)(context)));
        }
        (carry(reports), evaluation)
    }

    fn decide(&self, context: &LookContext<'_>) -> Result<TagEvaluation, CliError> {
        let (verdict, evaluation) = self.look(context);
        for (_, line) in verdict.said() {
            super::say_err(line);
        }
        self.settle(verdict.error, evaluation)
    }

    /// The one place a verdict and a tag evaluation become a result, so the
    /// prose and the document cannot reach different conclusions from the same
    /// run — including the pair where nothing refused and no evaluation
    /// arrived, which is a look that did not happen wearing a clean exit.
    fn settle(
        &self,
        error: Option<CliError>,
        evaluation: Option<TagEvaluation>,
    ) -> Result<TagEvaluation, CliError> {
        match (error, evaluation) {
            (Some(refusal), _) => Err(refusal),
            (None, Some(tags)) => Ok(tags),
            // The tag look answers or refuses, and a refusal is a verdict, so
            // this pair cannot arise. Stated as a refusal rather than a panic:
            // the name is the one the table announced.
            (None, None) => Err(CliError::unverified(format!(
                "unverified: the `{}` look neither answered nor refused",
                self.tag.name
            ))),
        }
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
    /// Taken from the plan that runs, never composed a second time.
    look_names: Vec<&'static str>,
}

impl Scope {
    /// The document's half of this report. Beside the fields themselves, so a
    /// transposition is a rename rather than a silent reorder.
    fn row(&self) -> ScopeRow {
        ScopeRow::of(
            self.selected,
            self.packages,
            self.base.as_deref().map_err(String::as_str),
            self.gating_coverage,
        )
    }

    fn of(git: &Git, loaded: &Loaded, args: &CheckArgs, plan: &LookPlan) -> Self {
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
            // Both read off the plan, not the flag: the sentence describes
            // what runs, and the two are the same answer only by convention.
            remote: plan.runs_remote(),
            look_names: plan.listed_names(),
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
            // Both endpoints, because only one of them is a commit the
            // reader chose: `diffing from <sha>` reads as a comparison against
            // the tree in front of them, and the coverage look reads neither
            // the index nor the worktree.
            Ok(base) => write!(f, ", diffing `{base}...HEAD`")?,
            // One line of why, inside the parentheses; the coverage look
            // raises the whole failure on stderr.
            Err(why) => write!(f, "; no base ref to diff from ({})", first_line(why))?,
        }
        write!(f, "\ncheck: looking at {}", named(&self.look_names))?;
        // A coverage look is a diff from a base. Dropping it from the list with
        // no base left the reader no line explaining the refusal that look then
        // raises, so it says why instead of going unmentioned.
        if self.base.is_err() {
            f.write_str("; coverage cannot run without a base ref")?;
        } else if !self.gating_coverage {
            f.write_str("; coverage reports without gating (`--strict` gates)")?;
        }
        if self.remote {
            return write!(f, "; looking at the {}", REMOTE.name);
        }
        write!(
            f,
            "; not looking at the {} (`--remote` asks for it)",
            REMOTE.name
        )
    }
}

/// A staging file means a write did not finish and the file it was replacing
/// may be stale; that is unverified, not clean. The directories are the ones
/// `version` stages into: `.changeset/`, the repository root (lockfiles),
/// every package directory (manifest, changelog), and every declared
/// extra-file's directory.
fn evaluate_staging(repo: &Repository, loaded: &Loaded) -> LookReport {
    match staging_refusal(repo, loaded) {
        Ok(report) => report,
        Err(refusal) => LookReport::from_result(Err(refusal)),
    }
}

fn staging_refusal(repo: &Repository, loaded: &Loaded) -> Result<LookReport, CliError> {
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
        return Ok(LookReport::default());
    }
    Ok(LookReport::refusing(vec![Refusal {
        error: CliError::unverified(format!(
            "unverified: {} oakum staging file(s) left behind",
            strays.len()
        )),
        lines: strays
            .iter()
            .map(|path| stray_staging_message(path))
            .collect(),
    }]))
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
fn evaluate_changelogs(repo: &Repository, loaded: &Loaded) -> LookReport {
    let reports = match changelog::foreign_changelogs(repo.dir(), &loaded.workspace, |package| {
        loaded.config.version_managed(package)
    }) {
        Ok(reports) => reports,
        Err(err) => {
            return LookReport::from_result(Err(CliError::unverified(format!("unverified: {err}"))))
        }
    };
    if reports.is_empty() {
        return LookReport::default();
    }
    LookReport::refusing(vec![Refusal {
        error: CliError::unverified(format!(
            "unverified: {} changelog(s) `oakum version` would refuse to append to",
            reports.len()
        )),
        lines: reports,
    }])
}

pub(super) fn run_tags_only() -> Result<(), CliError> {
    let repo = repository::discover().map_err(CliError::from_boxed)?;
    let git = Git::at_repository(&repo).map_err(CliError::from_boxed)?;
    let loaded = Loaded::load(&repo)?;
    let context = LookContext {
        git: &git,
        repo: &repo,
        loaded: &loaded,
        base: Err("tag-drift does not diff"),
        // `tag-drift` runs neither look that reads these.
        strict: None,
        remote_lookback: None,
    };
    LookPlan::tag_drift().decide(&context).map(|_| ())
}

/// `release` runs the coverage look and reports rather than gates on it: a
/// decision, not a placeholder. At module scope so a test can assert it —
/// flipped inside the function, the whole suite passed.
const NOT_STRICTLY: Option<bool> = Some(false);

/// The tag state, for `release`: Ok even when tags are pending, which `check`
/// refuses. The gate's other looks are `check`'s alone.
pub(super) fn evaluate(
    git: &Git,
    repo: &Repository,
    from: Option<&str>,
) -> Result<TagEvaluation, CliError> {
    let loaded = Loaded::load(repo)?;
    let base = resolved_base(git, from);
    let context = LookContext {
        git,
        repo,
        loaded: &loaded,
        base: base.as_deref().map_err(String::as_str),
        strict: NOT_STRICTLY,
        remote_lookback: None,
    };
    LookPlan::release().decide(&context)
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
fn evaluate_management(loaded: &Loaded) -> LookReport {
    let Loaded { config, workspace } = loaded;
    if !manages_nothing(config, workspace) {
        return LookReport::default();
    }
    // The guidance is the detail; the refusal stays short, like every sibling.
    LookReport::refusing(vec![Refusal {
        error: CliError::unverified(String::from(
            "unverified: this config manages no package on either axis",
        )),
        lines: vec![String::from(ALL_PRIVATE_GUIDANCE)],
    }])
}

fn refuse_if_pending(tags: &TagEvaluation) -> Result<(), CliError> {
    if tags.is_clean() {
        return Ok(());
    }
    Err(CliError::tag_drift(
        tags.drift.len() + tags.untagged_ahead.len(),
    ))
}

fn pending_lines(tags: &TagEvaluation) -> Vec<String> {
    let drift = tags.drift.iter().map(|item| {
        format!(
            "{}: manifest {} is above tagged {} (local tags; run `git fetch --tags` if the remote is ahead)",
            item.id(),
            item.manifest(),
            item.tagged()
        )
    });
    let untagged = tags.untagged_ahead.iter().map(|(id, version)| {
        format!("{id}: never released, but the manifest is {version}; tag the version you meant")
    });
    drift.chain(untagged).collect()
}

/// A pin that names another oakum.
fn evaluate_install_pin(repo: &Repository, loaded: &Loaded) -> Result<(), CliError> {
    match loaded.config.tool_version() {
        Some(expected) => install_pin::verify(repo.dir(), expected),
        None => Ok(()),
    }
}

fn evaluate_tags(git: &Git, repo: &Repository, loaded: &Loaded) -> Result<TagEvaluation, CliError> {
    let _ = repo.ambient_path().map_err(CliError::from_boxed)?;
    let Loaded { config, workspace } = loaded;
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
) -> LookReport {
    // The look's own early exits carry one refusal each; only its tail can
    // establish two at once.
    match coverage_refusals(git, repo, loaded, base, strict) {
        Ok(report) => report,
        Err(refusal) => LookReport::from_result(Err(refusal)),
    }
}

fn coverage_refusals(
    git: &Git,
    repo: &Repository,
    loaded: &Loaded,
    base: Result<&str, &str>,
    strict: bool,
) -> Result<LookReport, CliError> {
    // `unverified`, not an error: a base git does not have is a look that could
    // not happen, and ADR-0034 exists so a CI step can tell that from a
    // repository that failed a check. Routing it through `CliError::new` moved
    // a shallow-checkout `--from` off exit 2 and onto exit 1.
    let from = Some(base.map_err(|why| CliError::unverified(format!("unverified: {why}")))?);
    let Loaded { config, workspace } = loaded;
    let files =
        load_plan_bump_files(git, repo, workspace, config, from).map_err(CliError::from_boxed)?;
    // Read before anything is built from it: a `?` between constructing the
    // report and returning it would discard the whole look.
    let source = config.plan_intent_source()?;
    // The unmanaged half is `status`'s to report: a package the config cannot
    // plan is information, not a decision, and ADR-0027 records private-package
    // silence as something a changesets migratee keeps without a config change.
    // `check` gates; it reads only the coverage half.
    // `unseen` is what the look could not read, not what it found: this half
    // reads commits while intent is read off disk, so a change that is only
    // staged, only edited or not yet tracked is in neither and would otherwise
    // leave exit 0 standing for a question nobody asked.
    let coverage::Uncovered {
        // Both fields, not `..`: `Coverage` is the wire shape templates read
        // and must not grow one. One of several places that would say so —
        // every construction site fails first with a missing field.
        committed:
            oakum::state::Coverage {
                uncovered,
                unmanaged: _,
            },
        outside_head,
    } = coverage::changed_by_standing_and_unseen(
        git,
        workspace,
        &files,
        from,
        matches!(source, PlanIntentSource::ChangeFiles),
        |package| config.standing(package),
    )?;
    let hint = match source {
        PlanIntentSource::ChangeFiles => {
            "add a bump file (or `none` / empty frontmatter under --strict)"
        }
        PlanIntentSource::CommitsOnly => {
            "name the package in a conventional commit (or a path that maps to it)"
        }
    };
    // The worktree half is advisory in both directions. It never gates when it
    // answers — `--strict` decides what a finding costs, and a change outside
    // `HEAD` is not a finding — so failing to obtain it does not gate either.
    // Said, never silent: what the run could not read is the one thing this
    // look exists to stop it passing over.
    let (unseen, unreadable_worktree) = match outside_head {
        Ok(unseen) => (unseen.lines(hint), None),
        Err(err) => (
            Vec::new(),
            Some(format!(
                "the working tree could not be read, so this run looked only at commits: {}",
                err.detail()
            )),
        ),
    };
    let named_unmanaged = intent_named_unmanaged(config, workspace, &files);
    let mut uncovered_lines = Vec::new();
    for id in &uncovered {
        uncovered_lines.push(format!("{id}: changed with no covering intent; {hint}"));
    }
    let mut unseen_lines = unseen;
    unseen_lines.extend(unreadable_worktree);
    // The refusal names the first offender; the detail names the rest.
    let unmanaged = named_unmanaged.first().map(|id| Refusal {
        error: CliError::new(intent_names_unmanaged(&id.name)),
        lines: named_unmanaged
            .iter()
            .skip(1)
            .map(|id| intent_names_unmanaged(&id.name))
            .collect(),
    });
    // Both refusals travel: ranking, `also` marking and duplicate suppression
    // belong to `carry`.
    let gated = strict && !uncovered.is_empty();
    let (mut lines, uncovered_refusal) = if gated {
        let refusal = Refusal {
            error: CliError::uncovered(uncovered.len()),
            lines: uncovered_lines,
        };
        (Vec::new(), Some(refusal))
    } else {
        (uncovered_lines, None)
    };
    // Reported whether or not the look gates: `--strict` decides what a
    // finding costs, never whether a run admits what it did not read.
    lines.extend(unseen_lines);
    Ok(LookReport {
        lines,
        refusals: [unmanaged, uncovered_refusal]
            .into_iter()
            .flatten()
            .collect(),
    })
}

fn evaluate_remote(git: &Git, remote_lookback: u32) -> Result<(), CliError> {
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
    /// `settle`'s third arm is documented unreachable, which is why no
    /// integration test can drive it — and why the private method is the only
    /// place it can be pinned. Without it, the claim that both renders reach
    /// one conclusion is unverified.
    #[test]
    fn nothing_refused_and_no_evaluation_is_a_look_that_did_not_happen() {
        let refusal = super::LookPlan::check(false)
            .settle(None, None)
            .expect_err("a look that neither answered nor refused");
        assert_eq!(refusal.class(), crate::cli::Outcome::Unverified);
        assert!(refusal.detail().contains("tags"), "{}", refusal.detail());
    }

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

    /// `release` gates on the tag state, the pin and coverage — named, so a
    /// reordered `LOOKS` cannot shift what it gates on.
    #[test]
    fn release_looks_are_named() {
        assert_eq!(
            super::LookPlan::release().names(),
            ["tags", "install pin", "coverage"]
        );
        // `release` runs the coverage look and reports rather than gates.
        // Without this, flipping that decision passes the whole suite.
        assert_eq!(super::NOT_STRICTLY, Some(false));
    }

    /// The sentence names every look and invents none. A literal drifted in
    /// both directions with the suite green: `submodules` announced as a look
    /// that does not exist, and `staging` dropped while it still ran.
    #[test]
    fn the_announcement_names_every_look() {
        // The remote look runs, is reported as running, and is named by its
        // own clause rather than twice.
        let asked = super::LookPlan::check(true);
        assert_eq!(asked.names().last(), Some(&super::REMOTE.name));
        assert!(asked.runs_remote());
        assert!(!asked.listed_names().contains(&super::REMOTE.name));
        assert!(!super::LookPlan::check(false).runs_remote());
        let names = super::LookPlan::check(false).names();
        // The announcement is the plan's own list, so this pins the sentence
        // against the looks rather than against itself.
        assert_eq!(
            names,
            [
                "management",
                "tags",
                "install pin",
                "changelogs",
                "staging",
                "coverage"
            ]
        );
        let sentence = super::named(&names);
        for look in &names {
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
            names.len(),
            "one item per look: {sentence}"
        );
    }
}
