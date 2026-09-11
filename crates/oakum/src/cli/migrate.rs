//! `oakum migrate`: transform data, report tooling (ADR-0003 / ADR-0023).
//!
//! Version gate first.

use std::io::{self, IsTerminal, Write};
use std::path::Path;

use cap_std::fs::Dir;
use clap::{Args, ValueEnum};
use oakum::changeset::{
    instruction_occupants, is_bump_file_name, load_migration_bump_files, parse_migration,
    resolve_migration_change, write, ChangeFile, KnopePresence, LoadError, MigrationBumpFile,
    MigrationLoadAbort, UnknownReason,
};
use oakum::detect::{DetectReport, ReleaseTool};
use oakum::plan::{
    aggregate, compare_plans, compose, plan_fingerprint, BumpFile, BumpLevel, CascadeAs, Plan,
    PlanFingerprint, Versioning, Workspace,
};

use super::add::try_discover_workspace;

use super::changelog::foreign_changelogs;
use super::config::{enforce_tool_version, read_config_source, LoadedConfig};
use super::detect_tools;
use super::fs::{read_text, report_stray_staging, write_file_via_rename};
use super::git::{Git, Op};
use super::init::{
    binary_version, changeset_file_names, ensure_changeset_dir, list_paths,
    print_workflow_and_footer, WorkflowPins,
};
use super::install_pin;
use super::intent::refuse_malformed;
use super::migrate_config::{
    carried_private_packages, commit_message_steps, lossy_mapping_steps, migrated_settings,
    read_source_configs, shadowed_commit_messages, SourceConfig,
};
use super::migrate_output::{
    one_line, pending_owned_line, print_left_alone, print_pending, print_plan_comparison,
    print_remaining_steps, print_tag_shape, Remaining,
};
use super::migrate_source_plan::{fetch_source_before_plan, primary_plan_tool, SourceBeforePlan};
use super::owned_files::{
    missing_owned_files, restore_owned_file, write_owned_files, ConfigSettings, OwnedPlan,
    OwnedWrites, PrivatePackages,
};
use super::release::default_tag_template;
use super::repository;
use super::tag_shape::{self, ReadableTemplate, TagShape};
use super::tags::{all_tag_objects, incomplete_tag_history};
use super::CliError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum VersioningArg {
    #[value(name = "zero-major")]
    ZeroMajor,
    Semver,
}

impl VersioningArg {
    fn to_versioning(self) -> Versioning {
        match self {
            Self::ZeroMajor => Versioning::ZeroMajor,
            Self::Semver => Versioning::Semver,
        }
    }
}

/// The `versioning` mode and what settled it.
///
/// [`Versioning`] alone says which mode was written, not whether anyone chose
/// it, and the two readings call for different action from a reader checking
/// the config.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VersioningChoice {
    /// `--versioning`, which overrides whatever the source tool implies.
    Requested(Versioning),
    /// Taken from the tool being migrated away from, which decides the mode on
    /// its own — carrying one beside it would let the pair disagree, and a
    /// report would then state the disagreement as fact.
    Inferred(ReleaseTool),
}

impl VersioningChoice {
    pub(super) fn versioning(self) -> Versioning {
        match self {
            Self::Requested(versioning) => versioning,
            Self::Inferred(tool) => Self::implied_by(tool),
        }
    }

    /// knope and release-plz hold a breaking change below 1.0.0; the rest take
    /// `0.1.3` to `1.0.0`, and renumbering that line is not a migration's job.
    ///
    /// release-plz's own configuration documentation states the zero-major
    /// rule: "the transition from `0.x` to `0.(x+1)` is used for breaking
    /// changes". `release-please` sits on the other side by default, its
    /// `bump-minor-pre-major` being `false`.
    ///
    /// Exhaustive on purpose. A catch-all would give a new [`ReleaseTool`] a
    /// mode *and* the sentence justifying it, printing a claim about a tool
    /// nobody evaluated; the compiler asking is the point.
    pub(super) fn implied_by(tool: ReleaseTool) -> Versioning {
        match tool {
            ReleaseTool::Knope | ReleaseTool::ReleasePlz => Versioning::ZeroMajor,
            ReleaseTool::Changesets
            | ReleaseTool::Bumpy
            | ReleaseTool::ReleasePlease
            | ReleaseTool::SemanticRelease
            | ReleaseTool::NxRelease => Versioning::Semver,
        }
    }
}

#[derive(Debug, Args)]
pub(super) struct MigrateArgs {
    /// Override versioning inferred from the source tool.
    #[arg(long, value_enum)]
    versioning: Option<VersioningArg>,

    /// Apply without the confirmation prompt; required when stdin is not a terminal.
    #[arg(long)]
    yes: bool,
}

pub(super) fn run(args: &MigrateArgs) -> Result<(), Box<dyn std::error::Error>> {
    let repo = repository::discover()?;
    report_stray_staging(repo.dir())?;
    if let Some(source) = read_config_source(&repo)? {
        return already_migrated(&repo, &source, args.versioning, args.yes);
    }

    let report = detect_tools::scan(repo.dir())?;
    refuse_on_detection_errors(&report)?;
    if report.detections.is_empty() {
        return Err(Box::new(CliError::new(
            "nothing to migrate; run `oakum init`",
        )));
    }

    let found = SourceTools::read(repo.dir(), &report)?;
    let SourceTools {
        knope,
        changesets,
        bumpy,
        ref changeset_names,
        ref bumpy_names,
    } = found;

    // `detections` is non-empty: the refusal above is the only way past an
    // empty scan, so this cannot manufacture a tool nobody detected.
    let chosen = versioning_choice(&report.detections, args.versioning)
        .ok_or_else(|| CliError::new("nothing to migrate; run `oakum init`"))?;
    let versioning = chosen.versioning();

    // Before anything prints: a README or schema that is a directory or
    // symlink refuses here, not after the bump files were rewritten.
    let owned = OwnedPlan::probe(repo.dir())?;
    report_changeset_subdirs(repo.dir())?;

    let (sources, unreadable_sources) = read_and_report_source_configs(repo.dir());
    let workspace = optional_workspace(&repo)?;
    let foreign = foreign_changelog_reports(&repo, workspace.as_ref())?;
    let prepared = prepare_migration(
        repo.dir(),
        changeset_names,
        bumpy_names,
        knope,
        workspace.as_ref(),
    )?;
    let before_files = prepared.bump_files();
    let before = resolve_before_proof(
        &repo,
        workspace.as_ref(),
        &before_files,
        infer_versioning(&report.detections),
        knope,
        primary_plan_tool(knope, bumpy, changesets),
        &report.detections,
    )?;

    let (shape, settings) = tag_shape_and_settings(&repo, workspace.as_ref(), versioning, &sources);
    print_pending(&prepared.rewrites, &sources, &settings, owned, chosen);
    confirm_migration(args.yes)?;
    let owned_now = recheck_owned(repo.dir(), owned)?;

    let binary = binary_version()?;
    let pins = WorkflowPins::lookup(repo.ambient_path()?)?;
    let created = write_migration(
        repo.dir(),
        &prepared.rewrites,
        owned_now,
        &binary,
        settings.clone(),
    )?;

    let after_plan = after_plan(
        repo.dir(),
        workspace.as_ref(),
        &prepared.unknown_pairs(),
        versioning,
    );
    print_left_alone(owned_now.readme, &sources, &unreadable_sources);
    print_tag_shape(&shape, settings.tag_format);
    let comparison = conclude_plan_comparison(
        workspace.as_ref(),
        &before_files,
        knope,
        before.as_ref(),
        after_plan,
        prepared.unverified,
    );
    let leftovers: Vec<&str> = prepared
        .rewrites
        .iter()
        .filter_map(BumpRewrite::leftover)
        .collect();
    let gates = find_bump_file_gates(&repo, &report.detections);
    let mut owed_steps = lossy_mapping_steps(&sources);
    owed_steps.extend(commit_message_steps(&sources));
    owed_steps.extend(shadowed_commit_messages(&sources));
    let steps = print_steps_and_workflow(
        &Remaining {
            detections: &report.detections,
            knope,
            foreign_changelogs: &foreign,
            pinned: install_pin::has_any(repo.dir()),
            npm: pins.installs_via_npm(),
            binary: &binary,
            shape: &shape,
            leftovers: &leftovers,
            gates: &gates,
            owed: &owed_steps,
        },
        &pins,
        &created.written,
    );
    // The comparison first: it is a finding, and the gate look's failure is
    // only "we could not look". Reporting the second over the first would tell
    // a caller the transform went unverified when oakum had in fact verified
    // that it changed the release plan — the collapse run backwards, and the
    // CI recipe in docs/guide/github-actions.md would wave it through.
    comparison.and(steps)
}

/// The closing report, and the one verdict it carries of its own.
///
/// A failed gate look prints the word `unverified:` in its step, so the exit
/// code has to agree with it: a run that says `unverified:` on stdout and hands
/// the shell a `0` is the collapse [ADR-0034] closes, in the command that
/// motivated it.
///
/// [ADR-0034]: ../../../../docs/decisions/0034-exit-two-for-unverified.md
fn print_steps_and_workflow(
    remaining: &Remaining<'_>,
    pins: &WorkflowPins,
    written: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    print_remaining_steps(remaining);
    print_workflow_and_footer(remaining.binary, pins, written);
    if let GateLook::Failed(why) = remaining.gates {
        // Flattened: git's own diagnostic starts with `error:`, so a raw
        // interpolation puts a second `error:` line on stderr that reads as a
        // second failure.
        return Err(Box::new(CliError::unverified(format!(
            "unverified: migrated files were kept; could not look for gates on the old bump-file directory: {}",
            one_line(why)
        ))));
    }
    Ok(())
}

/// Which source tools this repository holds, and the bump files each left, read
/// once at the top of the run.
struct SourceTools {
    knope: bool,
    changesets: bool,
    bumpy: bool,
    changeset_names: Vec<String>,
    bumpy_names: Vec<String>,
}

impl SourceTools {
    /// Reports the instruction files occupying `.changeset/` as it goes: a
    /// reader meets them before the plan, which is where they can still act.
    fn read(dir: &Dir, report: &DetectReport) -> Result<Self, Box<dyn std::error::Error>> {
        let has = |tool| report.detections.iter().any(|hit| hit.tool() == tool);
        let bumpy = has(ReleaseTool::Bumpy);
        let changeset_names = changeset_file_names(dir)?;
        for occupant in instruction_occupants(changeset_names.iter().map(String::as_str)) {
            println!("{}", occupant.migrate_message());
        }
        Ok(Self {
            knope: knope_present(dir)?,
            changesets: has(ReleaseTool::Changesets),
            bumpy,
            changeset_names,
            bumpy_names: if bumpy {
                dir_file_names(dir, ".bumpy")?
            } else {
                Vec::new()
            },
        })
    }
}

/// What the write carries: the source tools' settings, plus the tag shape the
/// repository's own history settles.
fn settings_with_tag_shape(
    shape: &TagShape,
    workspace: Option<&Workspace>,
    versioning: Versioning,
    sources: &[SourceConfig],
    private_packages: PrivatePackages,
) -> ConfigSettings {
    migrated_settings(
        versioning,
        sources,
        written_tag_format(shape, workspace, private_packages),
    )
}

/// A scan that half-failed prints what it did see before refusing, so the
/// reader learns which tools were found rather than only that the look broke.
fn refuse_on_detection_errors(report: &DetectReport) -> Result<(), CliError> {
    if report.errors.is_empty() {
        return Ok(());
    }
    for hit in &report.detections {
        println!("{}\t{}", hit.tool().name(), hit.evidence());
    }
    let joined = report
        .errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ");
    Err(CliError::unverified(format!("unverified: {joined}")))
}

/// What the tags settle and the config that follows from it, together because
/// the second reads the first and neither is useful alone.
fn tag_shape_and_settings(
    repo: &repository::Repository,
    workspace: Option<&Workspace>,
    versioning: Versioning,
    sources: &[SourceConfig],
) -> (TagShape, ConfigSettings) {
    // One value reaches both. The derivation refuses a bare shape when more
    // than one package is tag-managed, and the write suppresses one that equals
    // the default; those agree only while they count the same packages.
    let private_packages = carried_private_packages(sources);
    let shape = derive_tag_shape(repo, workspace, private_packages);
    let settings =
        settings_with_tag_shape(&shape, workspace, versioning, sources, private_packages);
    (shape, settings)
}

/// bumpy's bump-file directory, without a trailing slash: a gate is as likely
/// to be written `grep '^\.bumpy'` as `-- '.bumpy/*.md'`, and searching for the
/// slashed form alone misses the first. [`Op::FilesMentioning`] appends the
/// slash for the pathspec that excludes the directory itself.
///
/// changesets and knope both use `.changeset/`, which oakum adopts in place, so
/// a gate pointed at it still finds files there.
const BUMPY_DIR: &str = ".bumpy";

/// What the repository's own files say about the old bump-file directory.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum GateLook {
    /// Tracked files naming it, outside `.changeset/` and outside the directory
    /// itself. Empty means the look ran and found none — which is reported, not
    /// passed over in silence, because the look sees only tracked files.
    Found(Vec<String>),
    /// The index lists no file and git reports no commit, so there is nothing a
    /// tracked gate could be hiding in. An index that is merely missing is
    /// [`Self::Failed`]: measured, a repository whose HEAD carries a gate
    /// answers `ls-files` with silence once `.git/index` is deleted, and
    /// calling that "nothing tracked" would exit 0 over a live gate.
    ///
    /// `rev-parse --verify --quiet HEAD` answers an unborn branch and a HEAD
    /// pointing at a vanished ref alike, so the line reports what git said
    /// rather than asserting the repository is empty.
    NothingTracked,
    /// The look failed. Never folded into an empty [`Self::Found`]: a gate
    /// nobody looked for is not a gate that is not there.
    Failed(String),
    /// The source tool's bump files already live in `.changeset/`, so there is
    /// no old directory for anything to be pointed at. Not "we skipped it".
    NothingToRepoint,
}

/// Files that gate on the old tool's bump-file directory (`okm-404.24`).
///
/// From the claude-plugins field record: a `PreToolUse` hook grepped
/// `git diff --cached -- '.bumpy/*.md'` for the plugin name, so once
/// `.changeset/` went live a commit carrying a valid oakum bump file was
/// rejected with a message telling the developer to use bumpy. Repointing it is
/// the reader's work; noticing the gate exists is cheap and nothing else does
/// it.
fn find_bump_file_gates(
    repo: &repository::Repository,
    detections: &[oakum::detect::Detection],
) -> GateLook {
    if !detections
        .iter()
        .any(|hit| hit.tool() == ReleaseTool::Bumpy)
    {
        return GateLook::NothingToRepoint;
    }
    let git = match Git::at_repository(repo) {
        Ok(git) => git,
        Err(err) => return GateLook::Failed(CliError::from_boxed(err).detail()),
    };
    match git.matched_paths(Op::FilesMentioning { dir: BUMPY_DIR }) {
        Ok(Some(paths)) => GateLook::Found(paths),
        Ok(None) => searched_nothing_or_found_nothing(&git),
        Err(err) => GateLook::Failed(err.detail()),
    }
}

/// `git grep` answers "no match" and "I searched no files" with the same exit 1
/// and the same silence. Asking what there was to search separates them; a
/// `git` wrapper that exits 1 without a diagnostic still reaches the wrong one,
/// which no question can fix from here.
fn searched_nothing_or_found_nothing(git: &Git) -> GateLook {
    match git.paths(Op::TrackedFiles) {
        Ok(tracked) if tracked.is_empty() => empty_index(git),
        Ok(_) => GateLook::Found(Vec::new()),
        Err(err) => GateLook::Failed(err.detail()),
    }
}

/// An empty index over a repository that has commits is a broken index, not an
/// empty repository — the files are in HEAD and a gate among them was never
/// searched. Only a repository with no commit at all has nothing to hide.
fn empty_index(git: &Git) -> GateLook {
    match git.predicate(Op::RefExists { reference: "HEAD" }) {
        Ok(false) => GateLook::NothingTracked,
        Ok(true) => GateLook::Failed(String::from(
            "git's index lists no file while HEAD has commits, so the index is missing or unbuilt and a gate among the committed files was not searched",
        )),
        Err(err) => GateLook::Failed(err.detail()),
    }
}

/// The shape the repository's own tags settle. Reading them is git I/O, so it
/// happens here; the derivation is pure.
///
/// A read that fails is [`TagShape::Unread`], never "no tags": that would
/// collapse "we did not look" into "never released". Neither outcome stops the
/// migration — `tag-format` is one config line, and `release` still refuses on
/// a mismatch it can see for itself.
fn derive_tag_shape(
    repo: &repository::Repository,
    workspace: Option<&Workspace>,
    private_packages: PrivatePackages,
) -> TagShape {
    match read_tag_names(repo) {
        Ok(names) => tag_shape::derive(&names, workspace, private_packages),
        Err(err) => TagShape::Unread(err.detail()),
    }
}

/// The tag names, or why the listing cannot stand for the history. A shallow
/// clone and a remote configured not to fetch tags both let `for-each-ref`
/// succeed over a set git never had, which would read as "no tags" — the
/// collapse `reachable_tags` already guards against.
fn read_tag_names(repo: &repository::Repository) -> Result<Vec<String>, CliError> {
    let git = Git::at_repository(repo).map_err(CliError::from_boxed)?;
    if let Some(why) = incomplete_tag_history(&git)? {
        return Err(CliError::unverified(format!("unverified: {why}")));
    }
    Ok(all_tag_objects(&git)?
        .into_iter()
        .map(|(name, _)| name)
        .collect())
}

/// The `tag-format` to write, if any. A derived shape that matches the default
/// `release` would apply anyway is not written: a config key that only
/// restates a fact is what ADR-0004 exists to keep out.
fn written_tag_format(
    shape: &TagShape,
    workspace: Option<&Workspace>,
    private_packages: PrivatePackages,
) -> Option<ReadableTemplate> {
    let TagShape::Derived { template, .. } = shape else {
        return None;
    };
    let managed = tag_shape::tag_managed_count(workspace, private_packages);
    (template.as_str() != default_tag_template(managed)).then_some(*template)
}

/// The source configs, with each one that could not be used named as it is
/// found. The same lines reach the closing summary, so a skipped setting is
/// recorded where a reader scrolls back to rather than only in passing.
fn read_and_report_source_configs(dir: &Dir) -> (Vec<SourceConfig>, Vec<String>) {
    let (sources, unreadable) = read_source_configs(dir);
    for line in &unreadable {
        eprintln!("{line}");
    }
    (sources, unreadable)
}

/// Ordered for a run that fails part-way: the directory, then the rewritten
/// bump files, then the files oakum owns.
fn write_migration(
    dir: &Dir,
    rewrites: &[BumpRewrite],
    owned: OwnedPlan,
    binary: &semver::Version,
    settings: ConfigSettings,
) -> Result<OwnedWrites, Box<dyn std::error::Error>> {
    ensure_changeset_dir(dir)?;
    apply_bump_rewrites(dir, rewrites)?;
    write_owned_files(dir, owned, binary, settings)
}

/// The prompt can wait a while; look at the owned files again before the
/// first write, and say what moved. The same sentence with a different plan
/// means only the README's ownership changed.
fn recheck_owned(dir: &Dir, before: OwnedPlan) -> Result<OwnedPlan, Box<dyn std::error::Error>> {
    let now = OwnedPlan::probe(dir)?;
    if now != before {
        println!("changed while waiting:");
        let line = pending_owned_line(now);
        if line == pending_owned_line(before) {
            println!("  .changeset/README.md changed; it is left as is");
        } else {
            println!("  {line}");
        }
    }
    Ok(now)
}

/// Changelogs `version` would refuse, read before the prompt so a read
/// failure stops the run before any write, and against the defaults `migrate`
/// is about to write so the same packages count as version-managed.
fn foreign_changelog_reports(
    repo: &super::repository::Repository,
    workspace: Option<&Workspace>,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let Some(workspace) = workspace else {
        return Ok(Vec::new());
    };
    let config = LoadedConfig::from_parsed(repo, oakum::config::OakumConfig::defaults())?;
    foreign_changelogs(repo.dir(), workspace, |package| {
        config.version_managed(package)
    })
}

/// `body` with single-quoted frontmatter keys written double-quoted, which
/// is what [`write`] emits. A file that differs from the canonical form only
/// in quote style keeps its quotes ([specs/bump-files.md]: a scoped name
/// keeps its quotes), so a Prettier `singleQuote` repository is not churned.
fn double_quoted_scoped_keys(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut delimiters = 0;
    for line in body.split_inclusive('\n') {
        let content = line.trim_end_matches(['\n', '\r']);
        if content == "---" {
            delimiters += 1;
        }
        let in_frontmatter = delimiters == 1 && content != "---";
        match content
            .strip_prefix('\'')
            .and_then(|rest| rest.split_once("': "))
        {
            Some((key, level)) if in_frontmatter => {
                out.push('"');
                out.push_str(key);
                out.push_str("\": ");
                out.push_str(level);
                out.push_str(&line[content.len()..]);
            }
            _ => out.push_str(line),
        }
    }
    out
}

/// `Ok(true)` skips the prompt. Without a terminal nobody can answer it, and
/// that run is usually CI by accident, so `--yes` is required, not assumed.
fn skip_migration_confirmation(yes: bool, stdin_is_tty: bool) -> Result<bool, CliError> {
    if yes {
        return Ok(true);
    }
    if !stdin_is_tty {
        return Err(CliError::new(
            "stdin is not a terminal; rerun with --yes to apply the changes above",
        ));
    }
    Ok(false)
}

fn confirm_migration(yes: bool) -> Result<(), Box<dyn std::error::Error>> {
    if skip_migration_confirmation(yes, io::stdin().is_terminal())? {
        return Ok(());
    }
    eprint!("Apply these changes? [y/N] ");
    io::stderr().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    accept_migration_answer(line.trim()).map_err(Into::into)
}

fn accept_migration_answer(answer: &str) -> Result<(), CliError> {
    match answer {
        "y" | "yes" | "Y" | "Yes" | "YES" => Ok(()),
        "" | "n" | "no" | "N" | "No" | "NO" => Err(CliError::new("migration cancelled")),
        other => Err(CliError::new(format!(
            "migration cancelled: unknown answer `{other}`; use y or n"
        ))),
    }
}

fn already_migrated(
    repo: &super::repository::Repository,
    source: &super::config::ConfigSource,
    versioning_flag: Option<VersioningArg>,
    yes: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let parsed = oakum::config::parse(source.text()).map_err(|err| {
        CliError::new(format!(
            "`.changeset/_config.toml` is not a valid oakum config: {err}"
        ))
    })?;
    let loaded = LoadedConfig::from_parsed(repo, parsed)?;
    enforce_tool_version(&loaded)?;
    if let Some(flag) = versioning_flag {
        let wanted = flag.to_versioning();
        let have = loaded.versioning();
        if wanted != have {
            return Err(Box::new(CliError::new(format!(
                "`--versioning` is `{wanted}` but `.changeset/_config.toml` has `versioning = \"{have}\"`; change `versioning` in `.changeset/_config.toml` to `{wanted}`"
            ))));
        }
    }
    restore_missing_owned_files(repo, yes)?;
    println!("already migrated");
    Ok(())
}

/// The write half of an already-migrated run: put back the owned files a
/// rerun found missing, after the same confirmation a first run gets.
fn restore_missing_owned_files(
    repo: &super::repository::Repository,
    yes: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let missing = missing_owned_files(repo.dir())?;
    if missing.is_empty() {
        return Ok(());
    }
    let rels: Vec<&str> = missing.iter().map(|file| file.rel()).collect();
    println!("pending:");
    println!("  write {}", list_paths(&rels));
    confirm_migration(yes)?;
    for file in missing {
        restore_owned_file(repo.dir(), file)?;
        println!("created {}", file.rel());
    }
    Ok(())
}

fn optional_workspace(
    repo: &repository::Repository,
) -> Result<Option<Workspace>, Box<dyn std::error::Error>> {
    try_discover_workspace(repo)
}

/// One bump file the migration will write, and where its content came from.
///
/// `source` and `dest` differ whenever the file is copied out of the old tool's
/// directory, and the two cases call for different words and different cleanup:
/// an in-place rewrite leaves nothing behind, a copy leaves the original where
/// the old tool still counts it (`okm-404.4`).
pub(super) struct BumpRewrite {
    source: String,
    dest: String,
    body: String,
}

impl BumpRewrite {
    /// A file rewritten where it lies, leaving nothing behind.
    pub(super) fn in_place(rel: String, body: String) -> Self {
        Self {
            source: rel.clone(),
            dest: rel,
            body,
        }
    }

    /// A file written into `.changeset/` from the old tool's directory. The
    /// original stays: [`BumpRewrite::leftover`] is what names it.
    pub(super) fn copied_out(from: String, to: String, body: String) -> Self {
        Self {
            source: from,
            dest: to,
            body,
        }
    }

    pub(super) fn dest(&self) -> &str {
        &self.dest
    }

    pub(super) fn body(&self) -> &str {
        &self.body
    }

    /// The original still on disk after the write, if the write left one.
    pub(super) fn leftover(&self) -> Option<&str> {
        (self.source != self.dest).then_some(self.source.as_str())
    }
}

#[derive(Default)]
struct PreparedMigration {
    rewrites: Vec<BumpRewrite>,
    /// Resolved once; compose uses known packages via [`MigrationBumpFile::bump_file`].
    snapshots: Vec<MigrationBumpFile>,
    unverified: bool,
}

impl PreparedMigration {
    fn bump_files(&self) -> Vec<BumpFile> {
        self.snapshots
            .iter()
            .map(MigrationBumpFile::bump_file)
            .collect()
    }

    fn unknown_pairs(&self) -> Vec<(String, String)> {
        self.snapshots
            .iter()
            .flat_map(|file| {
                file.unknown_missing()
                    .iter()
                    .map(|name| (file.id().to_string(), name.clone()))
            })
            .collect()
    }
}

fn prepare_migration(
    dir: &Dir,
    changeset_names: &[String],
    bumpy_names: &[String],
    knope: bool,
    workspace: Option<&Workspace>,
) -> Result<PreparedMigration, Box<dyn std::error::Error>> {
    // Batch load cannot interleave rewrite/knope refuses between parse and resolve.
    let mut prepared = PreparedMigration::default();
    let mut dests = Vec::new();

    if workspace.is_none() {
        prepared.unverified = changeset_names
            .iter()
            .chain(bumpy_names)
            .any(|name| is_bump_file_name(name));
        if prepared.unverified {
            println!("plan comparison skipped: no packages discovered");
        } else {
            println!("plan comparison skipped: nothing to compare");
        }
    }

    for name in changeset_names {
        if !is_bump_file_name(name) {
            continue;
        }
        let rel = format!(".changeset/{name}");
        let Some(body) = read_text(dir, &rel)? else {
            if workspace.is_some() {
                return Err(Box::new(CliError::new(format!(
                    "`{rel}` was listed but is missing"
                ))));
            }
            continue;
        };
        let change = parse_migration(&body).map_err(|err| {
            CliError::new(format!("`.changeset/{name}` is not a bump file: {err}"))
        })?;
        refuse_knope_unsafe(&rel, &change, knope)?;
        let next =
            write(change.entries(), change.note(), KnopePresence::Absent).map_err(|err| {
                CliError::new(format!("failed to rewrite `.changeset/{name}`: {err}"))
            })?;
        dests.push(name.clone());
        if next != body && double_quoted_scoped_keys(&body) != next {
            prepared
                .rewrites
                .push(BumpRewrite::in_place(rel.clone(), next));
        }
        if let Some(workspace) = workspace {
            push_snapshot(&mut prepared, rel, change, workspace)?;
        }
    }

    for name in bumpy_names {
        if !is_bump_file_name(name) {
            continue;
        }
        if dests.iter().any(|existing| existing == name) {
            return Err(Box::new(CliError::new(format!(
                "refusing to migrate `.bumpy/{name}`: `.changeset/{name}` already exists"
            ))));
        }
        let src = format!(".bumpy/{name}");
        let Some(body) = read_text(dir, &src)? else {
            if workspace.is_some() {
                return Err(Box::new(CliError::new(format!(
                    "`{src}` was listed but is missing"
                ))));
            }
            continue;
        };
        let change = parse_migration(&body)
            .map_err(|err| CliError::new(format!("`.bumpy/{name}` is not a bump file: {err}")))?;
        refuse_knope_unsafe(&src, &change, knope)?;
        let next = write(change.entries(), change.note(), KnopePresence::Absent)
            .map_err(|err| CliError::new(format!("failed to rewrite `.bumpy/{name}`: {err}")))?;
        prepared.rewrites.push(BumpRewrite::copied_out(
            src.clone(),
            format!(".changeset/{name}"),
            next,
        ));
        dests.push(name.clone());
        if let Some(workspace) = workspace {
            push_snapshot(&mut prepared, src, change, workspace)?;
        }
    }

    for (path, name) in prepared.unknown_pairs() {
        println!("unknown package `{name}` in `{path}`");
    }
    Ok(prepared)
}

fn push_snapshot(
    prepared: &mut PreparedMigration,
    rel: String,
    change: ChangeFile,
    workspace: &Workspace,
) -> Result<(), Box<dyn std::error::Error>> {
    let resolved = resolve_migration_change(rel, change, workspace).map_err(map_resolve_err)?;
    prepared.snapshots.push(resolved);
    Ok(())
}

fn map_resolve_err(err: LoadError) -> CliError {
    match err {
        LoadError::UnknownPackage(unknown) if unknown.reason == UnknownReason::Ambiguous => {
            CliError::new(format!(
                "package `{}` in `{}` matches more than one workspace package",
                unknown.name, unknown.file
            ))
        }
        other => CliError::new(other.to_string()),
    }
}

fn map_migration_load_abort(err: &MigrationLoadAbort) -> CliError {
    CliError::new(err.to_string())
}

/// Before fingerprint for plan comparison (`okm-45t.1`).
enum BeforeProof {
    Source {
        tool: ReleaseTool,
        fingerprint: PlanFingerprint,
        /// Read under a tool-specific convention from a child that did not exit
        /// 0, so a difference measured against it is unverified rather than a
        /// finding.
        under_convention: bool,
    },
    /// Fallback when the source tool could not supply a plan.
    Simulated {
        plan: Plan,
        tool_label: String,
        reason: String,
    },
}

fn resolve_before_proof(
    repo: &super::repository::Repository,
    workspace: Option<&Workspace>,
    files: &[BumpFile],
    versioning: Versioning,
    remap_knope_features: bool,
    plan_tool: Option<ReleaseTool>,
    detections: &[oakum::detect::Detection],
) -> Result<Option<BeforeProof>, Box<dyn std::error::Error>> {
    let Some(workspace) = workspace else {
        return Ok(None);
    };
    let Some(tool) = plan_tool else {
        let tool_label = detections.first().map_or_else(
            || String::from("unknown"),
            |hit| hit.tool().name().to_string(),
        );
        let reason = String::from("no supported source-tool before-plan command");
        println!(
            "plan comparison: source tool {tool_label} not runnable ({reason}); using oakum simulation — will exit unverified"
        );
        let plan = compose_plan(workspace, files, versioning, false)?;
        return Ok(Some(BeforeProof::Simulated {
            plan,
            tool_label,
            reason,
        }));
    };
    let cwd = repo.ambient_path()?;
    match fetch_source_before_plan(tool, cwd, workspace) {
        SourceBeforePlan::Available {
            tool,
            fingerprint,
            under_convention,
        } => {
            println!("plan comparison: before-plan from {}", tool.name());
            Ok(Some(BeforeProof::Source {
                tool,
                fingerprint,
                under_convention,
            }))
        }
        SourceBeforePlan::Unavailable { tool, reason } => {
            println!(
                "plan comparison: source tool {} not runnable ({reason}); using oakum simulation — will exit unverified",
                tool.name()
            );
            let plan = compose_plan(workspace, files, versioning, remap_knope_features)?;
            Ok(Some(BeforeProof::Simulated {
                plan,
                tool_label: tool.name().to_string(),
                reason,
            }))
        }
    }
}

enum AfterPlan {
    Skipped,
    Compared(Plan),
    Failed(Box<dyn std::error::Error>),
}

fn after_plan(
    dir: &Dir,
    workspace: Option<&Workspace>,
    already_unknown: &[(String, String)],
    versioning: Versioning,
) -> AfterPlan {
    let Some(workspace) = workspace else {
        return AfterPlan::Skipped;
    };
    let after_names = match changeset_file_names(dir) {
        Ok(names) => names,
        Err(err) => return AfterPlan::Failed(err),
    };
    let after_snapshots = match load_after_snapshots(dir, &after_names, workspace) {
        Ok(loaded) => loaded,
        Err(err) => return AfterPlan::Failed(err),
    };
    for file in &after_snapshots {
        for name in file.unknown_missing() {
            if !already_unknown
                .iter()
                .any(|(seen_path, seen)| seen == name && same_bump_file(seen_path, file.id()))
            {
                println!("unknown package `{name}` in `{}`", file.id());
            }
        }
    }
    let after_files: Vec<BumpFile> = after_snapshots
        .iter()
        .map(MigrationBumpFile::bump_file)
        .collect();
    match compose_plan(workspace, &after_files, versioning, false) {
        Ok(plan) => AfterPlan::Compared(plan),
        Err(err) => AfterPlan::Failed(err),
    }
}

fn load_after_snapshots(
    dir: &Dir,
    changeset_names: &[String],
    workspace: &Workspace,
) -> Result<Vec<MigrationBumpFile>, Box<dyn std::error::Error>> {
    let mut bodies = Vec::new();
    for name in changeset_names {
        if !is_bump_file_name(name) {
            continue;
        }
        let rel = format!(".changeset/{name}");
        let Some(body) = read_text(dir, &rel)? else {
            return Err(Box::new(CliError::new(format!(
                "`{rel}` was listed but is missing"
            ))));
        };
        bodies.push((rel, body));
    }
    let refs: Vec<(&str, &str)> = bodies
        .iter()
        .map(|(rel, body)| (rel.as_str(), body.as_str()))
        .collect();
    let loaded =
        load_migration_bump_files(refs, workspace).map_err(|err| map_migration_load_abort(&err))?;
    refuse_malformed(loaded.malformed())?;
    Ok(loaded.files().to_vec())
}

fn conclude_plan_comparison(
    workspace: Option<&Workspace>,
    files: &[BumpFile],
    knope: bool,
    before: Option<&BeforeProof>,
    after: AfterPlan,
    unverified_no_packages: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let unexpected = match (workspace, before, after) {
        (Some(workspace), Some(before), AfterPlan::Compared(after)) => {
            report_plan_comparison(workspace, files, knope, before, &after)
        }
        (_, _, AfterPlan::Failed(err)) => {
            println!("plan comparison: failed to recompute");
            return Err(Box::new(CliError::new(format!(
                "migrated files were kept; failed to recompute the release plan: {err}"
            ))));
        }
        _ => false,
    };
    if unexpected {
        // A difference is a finding only when the before-plan is one. Read from
        // a child that did not exit 0, under a convention about what its silence
        // means, it is not: the tool may simply have crashed, and calling that
        // a changed release plan reports a corrupted transform that never
        // happened. knope and changesets already refuse such a plan outright;
        // bumpy's convention is the one arm that accepts one.
        if matches!(
            before,
            Some(BeforeProof::Source {
                under_convention: true,
                ..
            })
        ) {
            return Err(Box::new(CliError::unverified(
                "unverified: migrated files were kept; the release plan differs from a before-plan read under bumpy's exit-1 convention, which a crashed run is indistinguishable from",
            )));
        }
        return Err(Box::new(CliError::new(
            "migrated files were kept; the release plan changed",
        )));
    }
    if unverified_no_packages {
        return Err(Box::new(CliError::unverified(
            "unverified: migrated files were kept; plan comparison skipped; no packages discovered",
        )));
    }
    if let Some(BeforeProof::Simulated {
        tool_label, reason, ..
    }) = before
    {
        return Err(Box::new(CliError::unverified(format!(
            "unverified: migrated files were kept; source-tool before-plan unavailable ({tool_label}): {reason}"
        ))));
    }
    Ok(())
}

/// The mode the *source tool* implies, with `--versioning` deliberately
/// dropped: the before-plan simulates what that tool would have planned, and
/// the user's override is about what oakum writes from here on.
fn infer_versioning(detections: &[oakum::detect::Detection]) -> Versioning {
    versioning_choice(detections, None).map_or(Versioning::Semver, VersioningChoice::versioning)
}

/// The mode and where it came from. `--versioning` wins; otherwise knope
/// decides, and every other source tool leaves `semver`.
///
/// The provenance travels with the value because the value alone cannot be
/// checked by a reader: `semver` is the non-default ([ADR-0022]) and, for a
/// repository whose packages are all below 1.0.0, the most consequential line
/// in the config `migrate` writes (`okm-404.9`).
///
/// [ADR-0022]: ../../../../docs/decisions/0022-zero-major-versioning.md
fn versioning_choice(
    detections: &[oakum::detect::Detection],
    flag: Option<VersioningArg>,
) -> Option<VersioningChoice> {
    if let Some(flag) = flag {
        return Some(VersioningChoice::Requested(flag.to_versioning()));
    }
    let tools: Vec<ReleaseTool> = detections
        .iter()
        .map(oakum::detect::Detection::tool)
        .collect();
    Some(VersioningChoice::Inferred(settles_the_mode(&tools)?))
}

/// The tool whose convention settles `versioning`: the first that holds a
/// breaking change below 1.0.0, else the first detected at all.
///
/// One rule, asked once. Naming knope here instead would apply
/// [`VersioningChoice::implied_by`]'s table to the first detection only, so a
/// repository running both bumpy and release-plz would take bumpy's `semver`
/// and print bumpy's justification for it — measured. `None` for an empty scan,
/// because a manufactured default is a claim about a tool nobody detected.
fn settles_the_mode(tools: &[ReleaseTool]) -> Option<ReleaseTool> {
    tools
        .iter()
        .copied()
        .find(|tool| VersioningChoice::implied_by(*tool) == Versioning::ZeroMajor)
        .or_else(|| tools.first().copied())
}

fn report_plan_comparison(
    workspace: &Workspace,
    files: &[BumpFile],
    knope: bool,
    before: &BeforeProof,
    after: &Plan,
) -> bool {
    let simulated_fp;
    let (before_fp, before_plan, before_label, planned_by) = match before {
        BeforeProof::Source {
            tool, fingerprint, ..
        } => (
            fingerprint,
            None,
            tool.name(),
            format!("planned by {}", tool.name()),
        ),
        BeforeProof::Simulated {
            plan, tool_label, ..
        } => {
            simulated_fp = plan_fingerprint(plan);
            (
                &simulated_fp,
                Some(plan),
                tool_label.as_str(),
                String::from("planned by the oakum simulation"),
            )
        }
    };
    let match_suffix = match before {
        BeforeProof::Source { .. } => String::new(),
        BeforeProof::Simulated { .. } => format!(" (unverified: {before_label} did not run)"),
    };
    let comparison = compare_plans(workspace, files, knope, before_fp, before_plan, after);
    print_plan_comparison(
        &comparison,
        before_label,
        &planned_by,
        &match_suffix,
        after.changes().len(),
    );
    comparison.expected_knope_feature().is_none() && comparison.unexpected().is_some()
}

fn report_changeset_subdirs(dir: &Dir) -> Result<(), Box<dyn std::error::Error>> {
    let entries = match dir.read_dir(".changeset") {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(Box::new(CliError::new(format!(
                "failed to read `.changeset/`: {err}"
            ))));
        }
    };
    for entry in entries {
        let entry =
            entry.map_err(|err| CliError::new(format!("failed to read `.changeset/`: {err}")))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(Box::new(CliError::new(
                "a path under `.changeset/` is not valid UTF-8",
            )));
        };
        if name == "." || name == ".." {
            continue;
        }
        let meta = entry.metadata().map_err(|err| {
            CliError::new(format!("failed to inspect `.changeset/{name}`: {err}"))
        })?;
        if meta.is_dir() {
            println!("subdirectory `.changeset/{name}` (ignored)");
        }
    }
    Ok(())
}

fn compose_plan(
    workspace: &Workspace,
    files: &[BumpFile],
    versioning: Versioning,
    remap_knope_features: bool,
) -> Result<Plan, Box<dyn std::error::Error>> {
    let mut files = files.to_vec();
    if remap_knope_features {
        for file in &mut files {
            for (id, level) in &mut file.entries {
                if *level == BumpLevel::Minor
                    && workspace
                        .get(id)
                        .is_some_and(|pkg| pkg.version().major == 0)
                {
                    *level = BumpLevel::Patch;
                }
            }
        }
    }
    let intent = aggregate(files);
    compose(
        workspace,
        &intent,
        |_| versioning,
        CascadeAs::Patch,
        |_, dep| Some(dep.range.clone()),
        |id| {
            workspace
                .get(id)
                .expect("compose only asks for workspace packages")
                .version()
                .clone()
        },
    )
    .map_err(|err| -> Box<dyn std::error::Error> { Box::new(CliError::new(err.to_string())) })
}

fn same_bump_file(left: &str, right: &str) -> bool {
    Path::new(left).file_name() == Path::new(right).file_name()
}

fn knope_present(dir: &Dir) -> Result<bool, Box<dyn std::error::Error>> {
    match dir.metadata("knope.toml") {
        Ok(meta) => Ok(meta.is_file()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(Box::new(CliError::new(format!(
            "failed to inspect `knope.toml`: {err}"
        )))),
    }
}

fn dir_file_names(dir: &Dir, rel: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let entries = match dir.read_dir(rel) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(Box::new(CliError::new(format!(
                "failed to read `{rel}/`: {err}"
            ))));
        }
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|err| CliError::new(format!("failed to read `{rel}/`: {err}")))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(Box::new(CliError::new(format!(
                "a path under `{rel}/` is not valid UTF-8"
            ))));
        };
        if name == "." || name == ".." {
            continue;
        }
        let meta = entry
            .metadata()
            .map_err(|err| CliError::new(format!("failed to inspect `{rel}/{name}`: {err}")))?;
        if meta.is_file() {
            names.push(name.to_string());
        }
    }
    // `read_dir` yields filesystem order, so an unsorted listing prints a
    // different plan on a different machine for the same repository.
    names.sort();
    Ok(names)
}

fn refuse_knope_unsafe(
    path: &str,
    parsed: &ChangeFile,
    knope: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if !knope {
        return Ok(());
    }
    if parsed.entries().is_empty() {
        return Err(Box::new(CliError::new(format!(
            "refusing to migrate `{path}`: empty frontmatter is unsafe while knope.toml is present"
        ))));
    }
    if let Some((pkg, _)) = parsed
        .entries()
        .iter()
        .find(|(pkg, _)| pkg.starts_with('@'))
    {
        return Err(Box::new(CliError::new(format!(
            "refusing to migrate scoped package `{pkg}` while knope.toml is present (quoted keys are invisible to knope; unquoting them breaks @changesets/cli)"
        ))));
    }
    if parsed
        .entries()
        .iter()
        .any(|(_, level)| *level == BumpLevel::None)
    {
        return Err(Box::new(CliError::new(format!(
            "refusing to migrate `{path}`: a `none` entry is unsafe while knope.toml is present"
        ))));
    }
    Ok(())
}

/// Prints each line as the write lands, so a failure part-way through leaves an
/// accurate record.
///
/// The old tool's file is never touched: `migrate` does not own `.bumpy/`
/// ([ADR-0003](../../../../docs/decisions/0003-write-only-what-a-command-owns.md)),
/// so a copy is all it may do and the original is the reader's to remove.
fn apply_bump_rewrites(
    dir: &Dir,
    planned: &[BumpRewrite],
) -> Result<(), Box<dyn std::error::Error>> {
    for rewrite in planned {
        write_file_via_rename(dir, Path::new(rewrite.dest()), rewrite.body())?;
        match rewrite.leftover() {
            Some(source) => println!("wrote {} from {source}", rewrite.dest()),
            None => println!("rewrote {}", rewrite.dest()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod versioning_modes {
    use oakum::detect::ReleaseTool;
    use oakum::plan::Versioning;

    use super::VersioningChoice;

    /// release-plz's own configuration documentation states the zero-major
    /// rule: "the transition from `0.x` to `0.(x+1)` is used for breaking
    /// changes". Grouping it with the semver tools would write `semver` into
    /// the config *and* print a sentence asserting release-plz takes 0.1.3 to
    /// 1.0.0, which is false about a tool oakum does not own.
    /// The rule is asked of every detection, not of the first. A repository
    /// running both bumpy and release-plz used to take bumpy's `semver` and
    /// print bumpy's justification for it, silently dropping the table's
    /// answer for the other tool.
    #[test]
    fn a_tool_holding_breaking_changes_below_one_settles_the_mode_whatever_its_order() {
        let pairs = [
            (Vec::from([ReleaseTool::ReleasePlz]), Versioning::ZeroMajor),
            (
                Vec::from([ReleaseTool::Bumpy, ReleaseTool::ReleasePlz]),
                Versioning::ZeroMajor,
            ),
            (
                Vec::from([ReleaseTool::ReleasePlz, ReleaseTool::Bumpy]),
                Versioning::ZeroMajor,
            ),
            (
                Vec::from([ReleaseTool::Bumpy, ReleaseTool::Changesets]),
                Versioning::Semver,
            ),
            (
                Vec::from([ReleaseTool::Changesets, ReleaseTool::Knope]),
                Versioning::ZeroMajor,
            ),
        ];
        for (tools, expected) in pairs {
            let settled = super::settles_the_mode(&tools).expect("a detection");
            assert_eq!(
                VersioningChoice::Inferred(settled).versioning(),
                expected,
                "{tools:?}"
            );
        }
    }

    /// No detection, no claim. The old fallback manufactured `Changesets` and
    /// `versioning_line` then printed a justification naming a tool nobody
    /// detected — the hazard `implied_by`'s exhaustive match exists to refuse.
    #[test]
    fn an_empty_scan_settles_nothing() {
        assert_eq!(super::settles_the_mode(&[]), None);
        assert_eq!(super::versioning_choice(&[], None), None);
    }

    #[test]
    fn release_plz_holds_a_breaking_change_below_one() {
        assert_eq!(
            VersioningChoice::Inferred(ReleaseTool::ReleasePlz).versioning(),
            Versioning::ZeroMajor
        );
        assert_eq!(
            VersioningChoice::Inferred(ReleaseTool::Knope).versioning(),
            Versioning::ZeroMajor
        );
        for semver in [
            ReleaseTool::Changesets,
            ReleaseTool::Bumpy,
            ReleaseTool::ReleasePlease,
            ReleaseTool::SemanticRelease,
            ReleaseTool::NxRelease,
        ] {
            assert_eq!(
                VersioningChoice::Inferred(semver).versioning(),
                Versioning::Semver,
                "{}",
                semver.name()
            );
        }
    }
}

#[cfg(test)]
mod confirmation {
    use super::{accept_migration_answer, skip_migration_confirmation};

    #[test]
    fn yes_skips_the_prompt_and_a_tty_gets_it() {
        assert!(skip_migration_confirmation(true, true).expect("yes"));
        assert!(skip_migration_confirmation(true, false).expect("yes"));
        assert!(!skip_migration_confirmation(false, true).expect("tty"));
    }

    #[test]
    fn non_tty_without_yes_refuses_and_names_the_flag() {
        let err = skip_migration_confirmation(false, false).expect_err("non-tty");
        assert!(err.to_string().contains("--yes"), "{err}");
    }

    #[test]
    fn accepts_yes_variants() {
        for answer in ["y", "yes", "Y", "Yes", "YES"] {
            accept_migration_answer(answer).expect("yes");
        }
    }

    #[test]
    fn declines_empty_or_no() {
        for answer in ["", "n", "no", "N", "No", "NO"] {
            let err = accept_migration_answer(answer).expect_err("no");
            assert_eq!(err.to_string(), "migration cancelled");
        }
    }

    #[test]
    fn declines_unknown_answers() {
        let err = accept_migration_answer("maybe").expect_err("unknown");
        assert!(err.to_string().contains("unknown answer `maybe`"));
    }
}

#[cfg(test)]
mod after_load_policy {
    use super::super::intent::refuse_malformed;
    use oakum::changeset::load_migration_bump_files;
    use oakum::plan::{Ecosystem, Package, PackageId, ResolvesDependenciesAt, Workspace};
    use semver::Version;

    fn workspace(packages: Vec<Package>) -> Workspace {
        Workspace::new(packages).expect("workspace")
    }

    fn cargo_pkg(name: &str) -> Package {
        Package::new(
            PackageId::new(Ecosystem::Cargo, name),
            Version::new(0, 1, 0),
            ResolvesDependenciesAt::Install,
            true,
            vec![],
        )
    }

    #[test]
    fn refuse_malformed_ok_when_empty() {
        refuse_malformed(&[]).expect("empty");
    }

    #[test]
    fn refuse_malformed_names_every_report() {
        let ws = workspace(vec![cargo_pkg("core")]);
        let loaded = load_migration_bump_files(
            [
                (".changeset/a.md", "not a bump"),
                (".changeset/b.md", "also broken"),
                (".changeset/ok.md", "---\ncore: patch\n---\n"),
            ],
            &ws,
        )
        .expect("missing packages only");
        assert_eq!(loaded.malformed().len(), 2);
        assert_eq!(loaded.files().len(), 1);
        let err = refuse_malformed(loaded.malformed()).expect_err("refuse");
        let message = err.to_string();
        assert!(
            message.contains("`.changeset/a.md` is not a bump file"),
            "{message}"
        );
        assert!(message.contains("; also "), "{message}");
        assert!(
            message.contains("`.changeset/b.md` is not a bump file"),
            "{message}"
        );
    }
}
