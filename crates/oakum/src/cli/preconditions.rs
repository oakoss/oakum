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
    let context = LookContext {
        git: &git,
        repo: &repo,
        loaded: &loaded,
        // The base the report named, resolved once. The failure travels too,
        // rather than being re-derived: a second resolution that succeeded
        // where the first did not would run a coverage look the report has
        // already said is not happening.
        base: scope.base.as_deref().map_err(String::as_str),
        strict: args.strict,
        remote_lookback: args.remote_lookback,
    };
    let looks = LOOKS.iter().chain(args.remote.then_some(&REMOTE));
    decide(looks, &context).map(|_| ())
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
    strict: bool,
    remote_lookback: u32,
}

/// What one look established: what it reports without refusing, each refusal
/// with the detail that supports it, and — for the tag look alone — the
/// evaluation `release` reads.
#[derive(Default)]
struct LookReport {
    lines: Vec<String>,
    refusals: Vec<Refusal>,
    tags: Option<TagEvaluation>,
}

/// A refusal and the evidence beneath it: one block of the verdict.
struct Refusal {
    error: CliError,
    lines: Vec<String>,
}

impl Refusal {
    fn bare(error: CliError) -> Self {
        Self {
            error,
            lines: Vec::new(),
        }
    }
}

impl LookReport {
    fn from_result(result: Result<(), CliError>) -> Self {
        Self::refusing(result.err().into_iter().map(Refusal::bare).collect())
    }

    fn refusing(refusals: Vec<Refusal>) -> Self {
        Self {
            refusals,
            ..Self::default()
        }
    }
}

/// The looks `check` performs, in the order it announces and runs them.
/// Management is the most fundamental thing wrong with a repository, but it
/// is one look among six: refusing on it alone would hide a stale install pin
/// or an unfinished write, and a different oakum may not have this look at
/// all. Tags run before the install pin so that a git which cannot run at all
/// is the first line a reader meets, not a pin string that differs.
const LOOKS: [Look; 6] = [MANAGEMENT, TAGS, INSTALL_PIN, CHANGELOGS, STAGING, COVERAGE];

const MANAGEMENT: Look = Look {
    name: "management",
    run: |context| evaluate_management(context.loaded),
};

const TAGS: Look = Look {
    name: "tags",
    run: look_tags_and_pending,
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
    run: |context| {
        evaluate_coverage(
            context.git,
            context.repo,
            context.loaded,
            context.base,
            context.strict,
        )
    },
};

/// Asked for by `--remote`, and announced by the scope report's own clause.
const REMOTE: Look = Look {
    name: "remote",
    run: |context| LookReport::from_result(evaluate_remote(context.git, context.remote_lookback)),
};

/// `tag-drift` is `check`'s tag look and the pin, in `check`'s order.
const TAG_DRIFT_LOOKS: [Look; 2] = [TAGS, INSTALL_PIN];

/// The tag state `release` reads: the same looks, without `check`'s pending
/// refusal — pending tags are what `release` is for.
const RELEASE_LOOKS: [Look; 3] = [
    Look {
        name: "tags",
        run: |context| match evaluate_tags(context.git, context.repo, context.loaded) {
            Ok(tags) => LookReport {
                tags: Some(tags),
                ..LookReport::default()
            },
            Err(refusal) => LookReport::from_result(Err(refusal)),
        },
    },
    INSTALL_PIN,
    COVERAGE,
];

/// Pending tags are a finding like the others and compete with them on that
/// footing; the detail travels with the refusal, one block.
fn look_tags_and_pending(context: &LookContext<'_>) -> LookReport {
    let tags = match evaluate_tags(context.git, context.repo, context.loaded) {
        Ok(tags) => tags,
        Err(refusal) => return LookReport::from_result(Err(refusal)),
    };
    let refusals = refuse_if_pending(&tags)
        .err()
        .map(|error| Refusal {
            error,
            lines: pending_lines(&tags),
        })
        .into_iter()
        .collect();
    LookReport {
        lines: Vec::new(),
        refusals,
        tags: Some(tags),
    }
}

/// Every look runs and every report is kept, so one unverified state does not
/// hide another — measured before the fold: a stale install pin returned early
/// and erased the coverage refusal, and a refused sibling dropped a tag
/// evaluation that had answered. Reports are said, the verdict is returned.
fn decide<'l>(
    looks: impl IntoIterator<Item = &'l Look>,
    context: &LookContext<'_>,
) -> Result<Option<TagEvaluation>, CliError> {
    let mut evaluation = None;
    let mut reports = Vec::new();
    for look in looks {
        let mut report = (look.run)(context);
        if let Some(tags) = report.tags.take() {
            assert!(
                evaluation.replace(tags).is_none(),
                "{} answered for the tag look after it had answered",
                look.name
            );
        }
        reports.push(report);
    }
    let (said, verdict) = carry(reports);
    for line in &said {
        super::say_err(line);
    }
    match verdict {
        Some(refusal) => Err(refusal),
        None => Ok(evaluation),
    }
}

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
            // One line of why, inside the parentheses; the coverage look
            // raises the whole failure on stderr.
            Err(why) => write!(f, "; no base ref to diff from ({})", first_line(why))?,
        }
        let names: Vec<&str> = LOOKS.iter().map(|look| look.name).collect();
        write!(f, "\ncheck: looking at {}", named(&names))?;
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
        strict: false,
        remote_lookback: 1,
    };
    decide(&TAG_DRIFT_LOOKS, &context).map(|_| ())
}

/// The tag state, for `release`: Ok even when tags are pending, which `check`
/// refuses. The gate's other looks are `check`'s alone.
pub(super) fn evaluate(
    git: &Git,
    repo: &Repository,
    from: Option<&str>,
) -> Result<TagEvaluation, CliError> {
    const NO_STRICT: bool = false;
    const UNREAD_LOOKBACK: u32 = 1;
    let loaded = Loaded::load(repo)?;
    let base = resolved_base(git, from);
    let context = LookContext {
        git,
        repo,
        loaded: &loaded,
        base: base.as_deref().map_err(String::as_str),
        strict: NO_STRICT,
        remote_lookback: UNREAD_LOOKBACK,
    };
    Ok(decide(&RELEASE_LOOKS, &context)?.expect("no refusal means the tag look answered"))
}

/// Every refusal reported, and the one that decides the exit code chosen by
/// what it means rather than by where it sits in the source: a finding
/// outranks a look that did not happen, and among equals the announced order
/// decides. Each look is one block — its summary, then its detail beneath —
/// with the deciding block first and the rest marked `also`, so a reader
/// meets the verdict before what is subordinate to it, and evidence sits
/// under the line it supports. A look with detail and no refusal is a report,
/// returned for the caller to say before the verdict.
///
/// Measured before this shape: `?` carried out whichever refusal was written
/// first, so a stray staging file turned a tag drift from `error` into
/// `unverified` (ADR-0034's split run backwards); the `also` lines printed
/// before the line they were also-to; and a look's detail sat five lines from
/// its summary with three unrelated lines between.
fn carry(reports: Vec<LookReport>) -> (Vec<String>, Option<CliError>) {
    let mut said = Vec::new();
    let mut blocks: Vec<Refusal> = Vec::new();
    for report in reports {
        said.extend(report.lines);
        for refusal in report.refusals {
            // Two looks can fail identically — the tag look and the coverage
            // look both run `rev-parse --is-shallow-repository` — and `also`
            // reads as a second, different problem. Say it once, and keep the
            // evidence both brought.
            match blocks.iter_mut().find(|block| {
                block.error.class() == refusal.error.class()
                    && block.error.to_string() == refusal.error.to_string()
            }) {
                Some(block) => block.lines.extend(refusal.lines),
                None => blocks.push(refusal),
            }
        }
    }
    // `min_by_key` returns the first minimum, so equal-severity refusals keep
    // the announced order and the empty case is the `?`.
    let Some(deciding) = blocks
        .iter()
        .enumerate()
        .min_by_key(|(_, block)| block.error.class())
        .map(|(index, _)| index)
    else {
        return (said, None);
    };
    let chosen = blocks.remove(deciding);
    let mut detail = first_line(&chosen.error.detail());
    indent_into(&mut detail, &continuation(&chosen.error.detail()));
    indent_into(&mut detail, &chosen.lines);
    for also in &blocks {
        detail.push_str("\nalso ");
        detail.push_str(also.error.outcome());
        detail.push_str(": ");
        detail.push_str(&first_line(&also.error.detail()));
        indent_into(&mut detail, &continuation(&also.error.detail()));
        indent_into(&mut detail, &also.lines);
    }
    (said, Some(chosen.error.recast(detail)))
}

/// A summary is one line; whatever git said beneath it is detail like any
/// other, so a two-line refusal does not read as two blocks.
fn first_line(detail: &str) -> String {
    detail.lines().next().unwrap_or_default().to_owned()
}

fn continuation(detail: &str) -> Vec<String> {
    detail.lines().skip(1).map(str::to_owned).collect()
}

fn indent_into(detail: &mut String, lines: &[String]) {
    for line in lines {
        detail.push('\n');
        if !line.is_empty() {
            detail.push_str("  ");
            detail.push_str(line);
        }
    }
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
    // The unmanaged half is `status`'s to report: a package the config cannot
    // plan is information, not a decision, and ADR-0027 records private-package
    // silence as something a changesets migratee keeps without a config change.
    // `check` gates; it reads only the coverage half.
    let oakum::state::Coverage { uncovered, .. } =
        coverage::changed_by_standing(git, workspace, &files, from, |package| {
            config.standing(package)
        })?;
    let named_unmanaged = intent_named_unmanaged(config, workspace, &files);
    let mut uncovered_lines = Vec::new();
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
            uncovered_lines.push(format!("{id}: changed with no covering intent; {hint}"));
        }
    }
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
    let (lines, uncovered_refusal) = if gated {
        let refusal = Refusal {
            error: CliError::uncovered(uncovered.len()),
            lines: uncovered_lines,
        };
        (Vec::new(), Some(refusal))
    } else {
        (uncovered_lines, None)
    };
    Ok(LookReport {
        lines,
        refusals: [unmanaged, uncovered_refusal]
            .into_iter()
            .flatten()
            .collect(),
        tags: None,
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
        let names: Vec<&str> = super::RELEASE_LOOKS.iter().map(|look| look.name).collect();
        assert_eq!(names, ["tags", "install pin", "coverage"]);
    }

    /// A two-line refusal is one block: its continuation sits under its
    /// summary, indented like detail, so it cannot read as a second block.
    #[test]
    fn a_multi_line_refusal_stays_one_block() {
        let report = super::LookReport::refusing(vec![super::Refusal {
            error: CliError::unverified("unverified: first\nsecond"),
            lines: vec![String::from("detail")],
        }]);
        let verdict = super::carry(vec![report]).1.expect("a refusal");
        assert_eq!(verdict.detail(), "first\n  second\n  detail");
    }

    /// An identical refusal from a second look is said once, and the evidence
    /// both looks brought survives under it — measured before the merge: the
    /// second look's lines were dropped with its duplicate summary.
    #[test]
    fn an_identical_refusal_merges_and_keeps_its_evidence() {
        let same = || CliError::unverified("unverified: same text");
        let first = super::LookReport::refusing(vec![super::Refusal {
            error: same(),
            lines: vec![String::from("from the first look")],
        }]);
        let second = super::LookReport::refusing(vec![super::Refusal {
            error: same(),
            lines: vec![String::from("from the second look")],
        }]);
        let verdict = super::carry(vec![first, second]).1.expect("a refusal");
        assert_eq!(
            verdict.detail(),
            "same text\n  from the first look\n  from the second look"
        );
    }

    /// A shadowed refusal's continuation sits under its `also` line too.
    #[test]
    fn a_shadowed_multi_line_refusal_stays_one_block() {
        let deciding =
            super::LookReport::refusing(vec![super::Refusal::bare(CliError::new("first"))]);
        let shadowed = super::LookReport::refusing(vec![super::Refusal::bare(
            CliError::unverified("unverified: git failed\nfatal: why"),
        )]);
        let verdict = super::carry(vec![deciding, shadowed]).1.expect("a refusal");
        assert_eq!(
            verdict.detail(),
            "first\nalso unverified: git failed\n  fatal: why"
        );
    }

    /// Same words, different classes: a finding must not merge into an
    /// unverified look that happened to say the same thing, or exit 2 would
    /// hide exit 1.
    #[test]
    fn a_finding_does_not_merge_into_an_identical_unverified_look() {
        let look = super::LookReport::refusing(vec![super::Refusal::bare(CliError::unverified(
            "unverified: same words",
        ))]);
        let finding = super::LookReport::refusing(vec![super::Refusal::bare(CliError::new(
            "unverified: same words",
        ))]);
        let verdict = super::carry(vec![look, finding]).1.expect("a refusal");
        assert_eq!(verdict.class(), super::super::Outcome::Error);
        assert_eq!(verdict.detail(), "same words\nalso unverified: same words");
    }

    /// A look with detail and no refusal is a report, handed back to be said
    /// before the verdict; it is not a block.
    #[test]
    fn a_report_is_said_and_is_not_a_block() {
        let advisory = super::LookReport {
            lines: vec![String::from("changed with no covering intent")],
            ..super::LookReport::default()
        };
        let refusing =
            super::LookReport::refusing(vec![super::Refusal::bare(CliError::new("drift"))]);
        let (said, verdict) = super::carry(vec![advisory, refusing]);
        assert_eq!(said, vec![String::from("changed with no covering intent")]);
        assert_eq!(verdict.expect("a refusal").detail(), "drift");
    }

    /// A blank line inside a detail stays blank, not two spaces.
    #[test]
    fn a_blank_detail_line_carries_no_indent() {
        let mut detail = String::from("summary");
        super::indent_into(&mut detail, &super::continuation("summary\none\n\nthree"));
        assert_eq!(detail, "summary\n  one\n\n  three");
    }

    /// The sentence names every look and invents none. A literal drifted in
    /// both directions with the suite green: `submodules` announced as a look
    /// that does not exist, and `staging` dropped while it still ran.
    #[test]
    fn the_announcement_names_every_look() {
        let names: Vec<&str> = super::LOOKS.iter().map(|look| look.name).collect();
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
            super::LOOKS.len(),
            "one item per look: {sentence}"
        );
    }
}
