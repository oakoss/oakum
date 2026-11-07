//! `oakum version`: write planned manifests, declared extra-files, inherited pins,
//! lockfile rows, and changelogs, then delete consumed bump files.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use cap_std::fs::Dir;
use clap::Args;
use oakum::config::ExtraFileFormat;
use oakum::manifest::{
    cargo_package_version_inherits_workspace, replace_json_at_key, retarget_cargo_lock,
    retarget_pnpm_lock, rewrite_dependencies, set_json_string, set_toml_string, CargoLockBump,
    PnpmCatalogChange, PnpmCatalogRef, PnpmImporterChange,
};
use oakum::plan::{
    aggregate, BumpFile, ChangeSource, DeclaredRange, Ecosystem, Package, PackageId, Plan,
    Workspace,
};
use oakum::state::{CoverageOutcome, ReleaseState, RenderTarget};
use semver::Version;

use super::changelog::{
    entry_refusals, plan_changelog_writes, supplied_note, utc_date, ChangelogPlan, EntryRefusal,
    Links, Provenance, ReleaseNote,
};
use super::config::{load_config, require_config, LoadedConfig, PlanIntentSource};
use super::consume;
use super::fs::repo_path_display;
use super::git::{Git, Op};
use super::inherited::plan_inherited_writes;
use super::install_pin::{self, PinPolicy};
use super::intent::{load_plan_bump_files, DroppedIntent};
use super::preconditions;
use super::printable::Printable;
use super::release_state::{apply_package_overrides, compose_plan, Discovered};
use super::repository;
use super::template::{load_contained_file, load_template_body};
use super::write_set::{
    commit_write_set, read_text, Consume, PlannedWrite, WriteSet, WriteSetFailure,
};
use super::CliError;

const CARGO_LOCK: &str = "Cargo.lock";
const PNPM_LOCK: &str = "pnpm-lock.yaml";
pub(super) const CHANGESET_DIR: &str = ".changeset";

pub(super) struct VersionWritePlan {
    pub repo: repository::Repository,
    pub writes: Vec<PlannedWrite>,
    pub consume: Consume,
    pub plan: Plan,
    pub tool_version: String,
    pub title: Option<oakum::template::TemplateSource>,
    pub commit_message: Option<oakum::template::TemplateSource>,
    /// Lockfile entries this run invalidated but left for `pnpm install`, one
    /// line each, for readers who never see stderr.
    pub lockfile_left: Vec<String>,
    /// What the writes do past a catalog pin through a YAML anchor.
    pub anchor_notes: Vec<String>,
    /// Read back from the changelog writes, so they are the bytes the branch commits.
    pub release_notes: Vec<ReleaseNote>,
    pub dropped: DroppedIntent,
}

impl VersionWritePlan {
    /// Whether a write changes a file: consuming bump files that release
    /// nothing opens no pull request (ADR-0023, amended 2026-10-30), though
    /// `ci version-pr` still refreshes one already open.
    pub(super) fn needs_github(&self) -> bool {
        self.writes
            .iter()
            .any(|write| write.original() != write.next())
    }

    /// Printed once the writes land: a refused run wrote nothing to describe.
    pub(super) fn say_landed_notes(&self) {
        for line in &self.lockfile_left {
            super::say_err(&format!("{line}; run `pnpm install` to update it"));
        }
        for note in &self.anchor_notes {
            super::say_err(note);
        }
    }

    /// Said once the plan is read, unlike the landed notes: it describes the
    /// intent, so a run that writes nothing or fails to write still says it.
    pub(super) fn say_dropped_intent(&self) {
        for line in self.dropped.lines() {
            super::say_err(&line);
        }
    }

    /// The state the version PR's body and templates render, so they name
    /// what `status` named.
    pub(super) fn release_state(&self) -> ReleaseState {
        self.dropped.report(ReleaseState::from_plan(
            &self.plan,
            CoverageOutcome::NotAsked,
            RenderTarget::Status,
        ))
    }
}

/// A bump file the rollback could not move back is the one thing a reader must
/// put right before re-running: the next run cannot see it, consumes what is
/// left, and reports success at a version nobody asked for.
fn with_recovery(failure: &WriteSetFailure) -> String {
    let mut text = failure.to_string();
    let stranded = failure.stranded();
    if stranded.is_empty() {
        return text;
    }
    let pronoun = if stranded.len() == 1 { "it" } else { "them" };
    let _ = write!(
        text,
        "\nmove {pronoun} back before re-running; a later run cannot see {pronoun} and will \
         version without {pronoun}:"
    );
    // Uncapped, since the changed-file list stops at the cap; Debug-quoted, so a
    // newline in a name cannot split a line meant to be pasted.
    for (staging, path) in &stranded {
        let _ = write!(text, "\n  {staging:?} -> {path:?}");
    }
    text
}

#[derive(Debug, Args)]
pub(super) struct VersionArgs {
    #[arg(long, value_name = "REF", help = super::FROM_INTENT)]
    from: Option<String>,
    /// Release notes body. `-` reads stdin. Path cannot escape the checkout.
    #[arg(long, value_name = "PATH")]
    notes_file: Option<PathBuf>,
}

pub(super) fn run(args: &VersionArgs) -> Result<(), Box<dyn std::error::Error>> {
    let prepared = plan_writes(args)?;
    prepared.say_dropped_intent();
    let committed = commit_write_set(prepared.repo.dir(), &prepared.writes, &prepared.consume)
        .map_err(|failure| with_recovery(&failure))?;
    prepared.say_landed_notes();
    // The writes have landed. A refused summary must not panic away the one
    // account of what changed, nor read as ok: the files exist, the answer did
    // not arrive.
    let delivered = super::deliver_report(&wrote_summary(&prepared)).map_err(|err| {
        CliError::undelivered("files written, but the summary of what changed", &err)
    });
    // After the summary, so an unremoved consumed file cannot cost it.
    committed.into_result()?;
    delivered?;
    Ok(())
}

/// What the run changed, printed after the writes land so it reports rather than
/// promises. `version` performs the irreversible part of a release — manifests,
/// lockfile rows, changelogs, `extra-files`, and the bump files it consumes —
/// and said nothing at all about any of it (`okm-404.7`).
///
/// `ci version-pr` calls [`plan_writes`] rather than [`run`], so its output is
/// unaffected.
fn wrote_summary(prepared: &VersionWritePlan) -> String {
    let mut out = String::new();
    let changes = prepared.plan.changes();
    if changes.is_empty() {
        // Not a wasted run: a bump file that names nothing is still consumed,
        // so the `consumed` lines below are the whole report. Every write path
        // iterates the plan's changes, so an empty plan wrote no file.
        out.push_str("versioned nothing\n");
    } else {
        out.push_str("versioned:\n");
        for (id, change) in changes {
            let cascaded = match change.source() {
                ChangeSource::Cascade { trigger } => format!(" (cascaded from {trigger})"),
                ChangeSource::Intent => String::new(),
            };
            let _ = writeln!(out, "  {id} {} -> {}{cascaded}", change.from(), change.to());
        }
    }
    // Only files whose bytes moved: a planned write that matched what was
    // already there is not something the reader has to look at.
    let touched: Vec<&Path> = prepared
        .writes
        .iter()
        .filter(|write| write.original() != write.next())
        .map(PlannedWrite::path)
        .collect();
    for path in &touched {
        let _ = writeln!(out, "  wrote {}", Printable(&repo_path_display(path)));
    }
    for path in prepared.consume.files() {
        let path = repo_path_display(path);
        let _ = writeln!(out, "  consumed {}", Printable(&path));
    }
    if touched.is_empty() && prepared.consume.files().is_empty() {
        out.push_str("  no file changed\n");
    }
    out
}

pub(super) fn plan_writes(
    args: &VersionArgs,
) -> Result<VersionWritePlan, Box<dyn std::error::Error>> {
    let repo = repository::discover()?;
    let config = load_config(&repo)?;
    require_config(&config)?;
    install_pin::gate(&repo, PinPolicy::Writer)?;
    consume::refuse_interrupted_consume(repo.dir())?;
    let (workspace, git, files, dropped) =
        Discovered::read(&repo, &config, args.from.as_deref())?.into_parts();
    let consume_ids = consumed_ids(&config, &files)?;
    let intent = aggregate(files);
    let plan = compose_plan(&config, &workspace, &intent)?;
    preconditions::hand_set_versions(&git, &repo, &config, &workspace, plan.changes())?.refuse()?;

    let (
        writes,
        consume,
        tool_version,
        title,
        commit_message,
        lockfile_left,
        anchor_notes,
        release_notes,
    ) = {
        let dir = repo.dir();
        let new_versions = versions_from_plan(&plan);
        let mut write_set = WriteSet::new();
        let (inherited, anchor_notes) = plan_inherited_writes(dir, &workspace, &new_versions)?;
        write_set.extend(inherited);
        plan_member_writes(dir, &workspace, &plan, &mut write_set)?;
        plan_extra_file_writes(dir, &workspace, &plan, &config, &mut write_set)?;
        write_set.extend(plan_lock_writes(dir, &workspace, &plan)?);
        let (pnpm_lock, lockfile_left) = plan_pnpm_lock_write(dir, &workspace, &plan, &write_set)?;
        write_set.extend(pnpm_lock);
        let date = utc_date(SystemTime::now())?;
        let tool_version = env!("CARGO_PKG_VERSION").to_owned();
        let template_body = changelog_template(&repo, &config)?;
        let supplied_notes = load_supplied_notes(&repo, args.notes_file.as_deref())?;
        let links = changelog_links(
            &git,
            template_body.as_deref(),
            supplied_notes.is_some(),
            &consume_ids,
        )?;
        let (changelogs, release_notes) = plan_changelog_writes(
            dir,
            &workspace,
            &config,
            &plan,
            &intent,
            &ChangelogPlan::new(
                &date,
                &tool_version,
                template_body.as_deref(),
                supplied_notes.as_deref(),
                links.as_ref(),
            ),
        )?;
        write_set.extend(changelogs);
        let consume = plan_consume(dir, &consume_ids)?;
        (
            write_set.writes(),
            consume,
            tool_version,
            config.title().cloned(),
            config.commit_message().cloned(),
            lockfile_left,
            anchor_notes,
            release_notes,
        )
    };
    Ok(VersionWritePlan {
        repo,
        writes,
        consume,
        plan,
        tool_version,
        title,
        commit_message,
        lockfile_left,
        anchor_notes,
        release_notes,
        dropped,
    })
}

/// The changelog entries [`plan_writes`] would refuse for the pending plan,
/// every one, without reading or writing a changelog. Inputs match `version`'s
/// except the date, which is today's, and provenance, which is the branch's:
/// for a bump file `base` does not hold, the commit that will add it to the
/// default branch does not exist yet, so a refusal that lifts once it does is
/// [`EntryRefusal::Unsettled`].
pub(super) fn refused_entries(
    repo: &repository::Repository,
    git: &Git,
    config: &LoadedConfig,
    discovered: &Workspace,
    from: Option<&str>,
    base: Result<&str, &str>,
    notes_file: Option<&Path>,
) -> Result<Vec<EntryRefusal>, Box<dyn std::error::Error>> {
    let workspace = apply_package_overrides(discovered, config)?;
    let files = load_plan_bump_files(git, repo, &workspace, config, from)?;
    let ids = consumed_ids(config, &files)?;
    let intent = aggregate(files);
    let plan = compose_plan(config, &workspace, &intent)?;
    if plan.changes().is_empty() {
        return Ok(Vec::new());
    }
    let template = changelog_template(repo, config)?;
    let supplied_notes = load_supplied_notes(repo, notes_file)?;
    let links = changelog_links(git, template.as_deref(), supplied_notes.is_some(), &ids)?;
    let settled = match &links {
        Some(links) => merged_links(links, &ids, &held_by(git, base, &ids)?),
        None => None,
    };
    let date = utc_date(SystemTime::now())?;
    let tool_version = env!("CARGO_PKG_VERSION");
    let input = |links| {
        ChangelogPlan::new(
            &date,
            tool_version,
            template.as_deref(),
            supplied_notes.as_deref(),
            links,
        )
    };
    Ok(entry_refusals(
        &workspace,
        config,
        &plan,
        &intent,
        &input(links.as_ref()),
        settled
            .as_ref()
            .map(|settled| input(Some(settled)))
            .as_ref(),
    ))
}

/// `links` as a squash merge records it for the files `held` lacks; `None`
/// when no file needs a stand-in.
fn merged_links(links: &Links, file_ids: &[String], held: &BTreeSet<String>) -> Option<Links> {
    let merged = Provenance {
        commit: "0".repeat(40),
        author: String::from("author"),
        email: String::from("author@example.invalid"),
        subject: String::from("merged (#1)"),
    };
    let mut stood_in = false;
    let by_file = file_ids
        .iter()
        .map(|id| match links.by_file.get(id) {
            Some(own) if held.contains(id) => (id.clone(), own.clone()),
            _ => {
                stood_in = true;
                (id.clone(), merged.clone())
            }
        })
        .collect();
    stood_in.then(|| Links {
        repo: links.repo.clone(),
        by_file,
    })
}

/// Bump files already on `base`, whose provenance the merge does not change.
fn held_by(
    git: &Git,
    base: Result<&str, &str>,
    file_ids: &[String],
) -> Result<BTreeSet<String>, CliError> {
    let Ok(base) = base else {
        return Ok(BTreeSet::new());
    };
    // A shallow clone names its graft as the commit that added a file below
    // it, so no provenance read there is the merge's.
    if super::tags::is_shallow(git)? {
        return Ok(BTreeSet::new());
    }
    let Some(commit) = git.merge_base(base)? else {
        return Ok(BTreeSet::new());
    };
    // A tree listing: reading contents would fetch blobs into a partial clone.
    let listed: BTreeSet<String> = git
        .paths(Op::TreePaths {
            commit: &commit,
            dir: ".changeset",
        })?
        .into_iter()
        .collect();
    Ok(file_ids
        .iter()
        .filter(|id| listed.contains(&format!(".changeset/{id}")))
        .cloned()
        .collect())
}

/// Commits-only intent has no file on disk to consume.
fn consumed_ids(config: &LoadedConfig, files: &[BumpFile]) -> Result<Vec<String>, CliError> {
    Ok(match config.plan_intent_source()? {
        PlanIntentSource::ChangeFiles => files.iter().map(|file| file.id.clone()).collect(),
        PlanIntentSource::CommitsOnly => Vec::new(),
    })
}

fn changelog_template(
    repo: &repository::Repository,
    config: &LoadedConfig,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    config
        .template()
        .map(|source| load_template_body(repo.dir(), repo.path(), source))
        .transpose()
}

/// A supplied body leaves `changes` empty, so there is nothing to link.
fn changelog_links(
    git: &Git,
    template: Option<&str>,
    supplied_notes: bool,
    file_ids: &[String],
) -> Result<Option<Links>, Box<dyn std::error::Error>> {
    match (template, supplied_notes) {
        (Some(source), false) => template_links(git, source, file_ids),
        _ => Ok(None),
    }
}

fn load_supplied_notes(
    repo: &repository::Repository,
    notes_file: Option<&Path>,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let Some(path) = notes_file else {
        return Ok(None);
    };
    if path == Path::new("-") {
        let mut body = String::new();
        io::stdin()
            .read_to_string(&mut body)
            .map_err(|err| CliError::new(format!("`--notes-file -`: {err}")))?;
        let body = strip_bom(&body);
        if supplied_note(&body).is_none() {
            return Err(Box::new(CliError::new(
                "`--notes-file -` produced no notes",
            )));
        }
        return Ok(Some(body));
    }
    let relative = path
        .to_str()
        .ok_or_else(|| CliError::new("`--notes-file` path is not valid UTF-8"))?;
    let body = load_contained_file(repo.dir(), repo.path(), relative, "--notes-file")?;
    Ok(Some(strip_bom(&body)))
}

fn strip_bom(body: &str) -> String {
    body.trim_start_matches('\u{FEFF}').to_owned()
}

fn plan_consume(dir: &Dir, ids: &[String]) -> Result<Consume, Box<dyn std::error::Error>> {
    let consume = Consume::new(CHANGESET_DIR, ids);
    for path in consume.files() {
        read_text(dir, path)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} is missing", repo_path_display(path)),
            )
        })?;
    }
    Ok(consume)
}

fn versions_from_plan(plan: &Plan) -> BTreeMap<PackageId, Version> {
    plan.changes()
        .values()
        .map(|change| (change.id().clone(), change.to().clone()))
        .collect()
}

fn plan_member_writes(
    dir: &Dir,
    workspace: &Workspace,
    plan: &Plan,
    write_set: &mut WriteSet,
) -> Result<(), Box<dyn std::error::Error>> {
    let new_versions = versions_from_plan(plan);
    if new_versions.is_empty() {
        return Ok(());
    }
    for package in workspace.packages() {
        let bump = plan.get(package.id());
        let retargets = package
            .dependencies()
            .iter()
            .any(|dep| new_versions.contains_key(&dep.on));
        if bump.is_none() && !retargets {
            continue;
        }
        let path = PathBuf::from(package.manifest_path());
        let (original, mut next) = write_set.source_text(dir, &path)?;
        if let Some(change) = bump {
            if package.id().ecosystem == Ecosystem::Cargo
                && cargo_package_version_inherits_workspace(&next)
                    .map_err(|err| format!("{}: {err}", repo_path_display(&path)))?
            {
                ensure_inheritors_are_planned(dir, workspace, write_set, plan, change.to())?;
                next = plan_workspace_package_version(
                    dir,
                    workspace,
                    write_set,
                    &path,
                    next,
                    change.from(),
                    change.to(),
                )?;
            } else {
                next = bump_package_version(package.id().ecosystem, &next, change.to()).map_err(
                    |err| -> Box<dyn std::error::Error> {
                        format!("{}: {err}", repo_path_display(&path)).into()
                    },
                )?;
            }
        }
        if retargets {
            next = rewrite_dependencies(
                package.id().ecosystem,
                &next,
                package.dependencies(),
                &new_versions,
            )
            .map_err(|err| -> Box<dyn std::error::Error> {
                format!("{}: {err}", repo_path_display(&path)).into()
            })?;
        }
        write_set.put_write(path, original, next);
    }
    Ok(())
}

/// `None` when the template reads neither `repo` nor `changes`, so it costs
/// no git children. Off GitHub `repo` is `None` and the commit fields render
/// without URLs; a `GITHUB_REPOSITORY` that is set but malformed is an error,
/// as in `ci`.
fn template_links(
    git: &Git,
    source: &str,
    file_ids: &[String],
) -> Result<Option<Links>, Box<dyn std::error::Error>> {
    if !oakum::template::reads_any(source, &["repo", "changes"])? {
        return Ok(None);
    }
    let slug = super::ci::repository_slug(git);
    let repo = if std::env::var("GITHUB_REPOSITORY").is_ok_and(|value| !value.trim().is_empty()) {
        Some(slug?)
    } else {
        slug.ok()
    };
    let mut links = Links {
        repo,
        by_file: BTreeMap::new(),
    };
    for id in file_ids {
        let path = format!(".changeset/{id}");
        if let Some(found) = Provenance::of(git, &path)? {
            links.by_file.insert(id.clone(), found);
        }
    }
    Ok(Some(links))
}

fn plan_extra_file_writes(
    dir: &Dir,
    workspace: &Workspace,
    plan: &Plan,
    config: &LoadedConfig,
    write_set: &mut WriteSet,
) -> Result<(), Box<dyn std::error::Error>> {
    for change in plan.changes().values() {
        let Some(package) = workspace.get(change.id()) else {
            return Err(format!(
                "plan contains `{}` but the workspace has no such package",
                change.id().name
            )
            .into());
        };
        let extras = config.extra_files_for(&change.id().name);
        if extras.is_empty() {
            continue;
        }
        let next_version = change.to().to_string();
        for extra in extras {
            let path = extra_file_repo_path(package, extra.path()).map_err(|err| {
                format!(
                    "extra-files `{}` for `{}`: {err}",
                    extra.path(),
                    change.id().name
                )
            })?;
            let (original, current) = write_set.source_text(dir, &path).map_err(|err| {
                format!(
                    "extra-files `{}` for `{}`: {err}",
                    extra.path(),
                    change.id().name
                )
            })?;
            let next = match extra.format() {
                ExtraFileFormat::Json => replace_json_at_key(&current, extra.key(), &next_version)
                    .map_err(|err| {
                        format!(
                            "{} (extra-files key `{}` for `{}`): {err}",
                            repo_path_display(&path),
                            extra.key(),
                            change.id().name
                        )
                    })?,
            };
            write_set.put_write(path, original, next);
        }
    }
    Ok(())
}

/// Leading `/` is repository-root relative; otherwise relative to the package
/// manifest directory (ADR-0033). Lexically collapse `.` / `..` so two spellings
/// of the same shared file share one `WriteSet` key. Escaping above the
/// repository root is an error (unmatched `..` is not clamped away).
pub(super) fn extra_file_repo_path(
    package: &Package,
    declared: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    // `Path::join`, not a string join: on Windows a rooted or prefixed
    // `declared` replaces the package directory, which normalization then drops.
    let joined = match declared.strip_prefix('/') {
        Some(rest) => PathBuf::from(rest),
        None => Path::new(package.manifest_dir()).join(declared),
    };
    lexical_normalize_repo_relative(&joined)
}

fn lexical_normalize_repo_relative(path: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if !out.pop() {
                    return Err("path escapes the repository".into());
                }
            }
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
            Component::Normal(s) => out.push(s),
        }
    }
    if out.as_os_str().is_empty() {
        return Err("path resolves to empty".into());
    }
    Ok(out)
}

fn plan_lock_writes(
    dir: &Dir,
    workspace: &Workspace,
    plan: &Plan,
) -> Result<Vec<PlannedWrite>, Box<dyn std::error::Error>> {
    let bumps: Vec<CargoLockBump<'_>> = plan
        .changes()
        .values()
        .filter(|change| change.id().ecosystem == Ecosystem::Cargo)
        .map(|change| CargoLockBump {
            name: change.id().name.as_str(),
            from: change.from(),
            to: change.to(),
        })
        .collect();
    if bumps.is_empty() {
        return Ok(Vec::new());
    }
    let path = cargo_lock_path(workspace).ok_or_else(|| {
        CliError::new(
            "cargo packages were bumped but no cargo workspace root is set, so Cargo.lock cannot be retargeted",
        )
    })?;
    let Some(original) = read_text(dir, &path)? else {
        return Ok(Vec::new());
    };
    let next =
        retarget_cargo_lock(&original, &bumps).map_err(|err| -> Box<dyn std::error::Error> {
            format!("{}: {err}", repo_path_display(&path)).into()
        })?;
    Ok(vec![PlannedWrite::new(path, original, next)])
}

/// Reads the manifests and catalog after every other write is planned, so the
/// lockfile follows what those writes changed rather than restating the rules
/// that changed them. An entry it cannot rewrite is named, not refused: the
/// files it did write are still right, and `pnpm install` finishes the job.
/// The second value is one line per entry left.
fn plan_pnpm_lock_write(
    dir: &Dir,
    workspace: &Workspace,
    plan: &Plan,
    write_set: &WriteSet,
) -> Result<(Option<PlannedWrite>, Vec<String>), Box<dyn std::error::Error>> {
    let Some(lock_dir) = workspace.pnpm_lock_dir() else {
        return Ok((None, Vec::new()));
    };
    let new_versions = versions_from_plan(plan);
    if !new_versions.keys().any(|id| id.ecosystem == Ecosystem::Npm) {
        return Ok((None, Vec::new()));
    }
    let path = Path::new(lock_dir).join(PNPM_LOCK);
    let Some(original) = read_text(dir, &path)? else {
        return Ok((None, Vec::new()));
    };

    let mut manifests = Vec::new();
    let mut catalog_refs = Vec::new();
    for package in workspace.packages() {
        if package.id().ecosystem != Ecosystem::Npm {
            continue;
        }
        let importer = pnpm_importer(lock_dir, package.manifest_dir());
        let mut edges = Vec::new();
        for dep in package.dependencies() {
            if !new_versions.contains_key(&dep.on) {
                continue;
            }
            if let DeclaredRange::Catalog { name, .. } = &dep.range {
                catalog_refs.push((name.clone(), dep.declared_as.clone(), importer.clone()));
            }
            if let Some(section) = dep.kind.section(Ecosystem::Npm) {
                edges.push((section, dep.declared_as.clone()));
            }
        }
        if edges.is_empty() {
            continue;
        }
        let (before, after) =
            write_set.source_text(dir, &PathBuf::from(package.manifest_path()))?;
        manifests.push((importer, before, after, edges));
    }
    let edge_refs: Vec<Vec<(&str, &str)>> = manifests
        .iter()
        .map(|(_, _, _, edges)| {
            edges
                .iter()
                .map(|(section, key)| (*section, key.as_str()))
                .collect()
        })
        .collect();
    let importers: Vec<PnpmImporterChange<'_>> = manifests
        .iter()
        .zip(&edge_refs)
        .map(|((importer, before, after, _), edges)| PnpmImporterChange {
            importer,
            original: before,
            next: after,
            edges,
        })
        .collect();

    let catalog_texts = match workspace.catalog_file() {
        Some(file) if !catalog_refs.is_empty() => {
            Some(write_set.source_text(dir, Path::new(file))?)
        }
        _ => None,
    };
    let refs: Vec<PnpmCatalogRef<'_>> = catalog_refs
        .iter()
        .map(|(name, key, importer)| PnpmCatalogRef {
            catalog: name.as_deref(),
            key,
            importer,
        })
        .collect();
    let catalog = (!refs.is_empty()).then(|| PnpmCatalogChange {
        files: catalog_texts
            .as_ref()
            .map(|(before, after)| (before.as_str(), after.as_str())),
        refs: &refs,
    });

    let retarget = retarget_pnpm_lock(&original, &importers, catalog);
    let left = retarget
        .left
        .iter()
        .map(|left| format!("{}: left {left}", repo_path_display(&path)))
        .collect();
    Ok((Some(PlannedWrite::new(path, original, retarget.text)), left))
}

/// The importer key pnpm uses for a member: its directory relative to the
/// lockfile's, `.` for the same directory, and `../` steps for a member
/// outside it. Both are repository-relative with `/` separators.
fn pnpm_importer(lock_dir: &str, manifest_dir: &str) -> String {
    let lock: Vec<&str> = lock_dir.split('/').filter(|s| !s.is_empty()).collect();
    let member: Vec<&str> = manifest_dir.split('/').filter(|s| !s.is_empty()).collect();
    let shared = lock.iter().zip(&member).take_while(|(a, b)| a == b).count();
    let mut steps = vec![".."; lock.len() - shared];
    steps.extend(&member[shared..]);
    if steps.is_empty() {
        String::from(".")
    } else {
        steps.join("/")
    }
}

fn plan_workspace_package_version(
    dir: &Dir,
    workspace: &Workspace,
    write_set: &mut WriteSet,
    member_path: &Path,
    member_next: String,
    from: &Version,
    to: &Version,
) -> Result<String, Box<dyn std::error::Error>> {
    let path = cargo_workspace_toml_path(workspace)?;
    if member_path == path {
        return set_workspace_package_version(&member_next, from, to)
            .map_err(|err| format!("{}: {err}", repo_path_display(&path)).into());
    }
    let (original, next) = write_set.source_text(dir, &path)?;
    let next = set_workspace_package_version(&next, from, to).map_err(
        |err| -> Box<dyn std::error::Error> {
            format!("{}: {err}", repo_path_display(&path)).into()
        },
    )?;
    write_set.put_write(path, original, next);
    Ok(member_next)
}

fn ensure_inheritors_are_planned(
    dir: &Dir,
    workspace: &Workspace,
    write_set: &WriteSet,
    plan: &Plan,
    to: &Version,
) -> Result<(), Box<dyn std::error::Error>> {
    for package in workspace.packages() {
        if package.id().ecosystem != Ecosystem::Cargo {
            continue;
        }
        let path = PathBuf::from(package.manifest_path());
        let (_, text) = write_set.source_text(dir, &path)?;
        if !cargo_package_version_inherits_workspace(&text)
            .map_err(|err| format!("{}: {err}", repo_path_display(&path)))?
        {
            continue;
        }
        match plan.get(package.id()) {
            Some(change) if change.to() == to => {}
            Some(change) => {
                return Err(format!(
                    "{} inherits [workspace.package].version but the plan needs {}; another inheritor needs {to}",
                    package.id().name,
                    change.to()
                )
                .into());
            }
            None => {
                return Err(format!(
                    "{} inherits [workspace.package].version and is not in the plan; writing {to} would change it without a changeset",
                    package.id().name
                )
                .into());
            }
        }
    }
    Ok(())
}

fn set_workspace_package_version(
    text: &str,
    from: &Version,
    to: &Version,
) -> Result<String, Box<dyn std::error::Error>> {
    let next = to.to_string();
    if let Some(current) = workspace_package_version(text)? {
        if current == next {
            return Ok(text.to_owned());
        }
        if current != from.to_string() {
            return Err(format!(
                "[workspace.package].version is already {current}; cannot also set {next}"
            )
            .into());
        }
    }
    Ok(set_toml_string(
        text,
        &["workspace", "package", "version"],
        &next,
    )?)
}

fn workspace_package_version(text: &str) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let doc: toml_edit::DocumentMut = text.parse().map_err(|err| format!("{err}"))?;
    Ok(doc
        .get("workspace")
        .and_then(|workspace| workspace.get("package"))
        .and_then(|package| package.get("version"))
        .and_then(|version| version.as_str())
        .map(str::to_owned))
}

fn cargo_workspace_toml_path(workspace: &Workspace) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let manifest = workspace.cargo_root_manifest().ok_or_else(|| {
        CliError::new(
            "a cargo package inherits [package].version from the workspace but no cargo workspace root is set",
        )
    })?;
    Ok(PathBuf::from(manifest))
}

fn bump_package_version(
    ecosystem: Ecosystem,
    text: &str,
    to: &Version,
) -> Result<String, Box<dyn std::error::Error>> {
    let next = to.to_string();
    match ecosystem {
        Ecosystem::Cargo => Ok(set_toml_string(text, &["package", "version"], &next)?),
        Ecosystem::Npm => Ok(set_json_string(text, &["version"], &next)?),
    }
}

fn cargo_lock_path(workspace: &Workspace) -> Option<PathBuf> {
    let root = workspace.cargo_workspace_root()?;
    Some(if root.is_empty() {
        PathBuf::from(CARGO_LOCK)
    } else {
        Path::new(root).join(CARGO_LOCK)
    })
}

#[cfg(test)]
mod tests {
    use super::super::write_set::{LeftChanged, WriteSetFailure, HEAD_FILES};
    use super::{pnpm_importer, with_recovery};

    fn stranded(name: &str) -> LeftChanged {
        LeftChanged::Stranded {
            path: format!(".changeset/{name}.md"),
            staging: format!(".changeset/.{name}.md.oakum-write.1.2.0"),
            err: String::from("nope"),
        }
    }

    fn rolled_back(left_changed: Vec<LeftChanged>, unswept: Vec<String>) -> WriteSetFailure {
        WriteSetFailure::RolledBack {
            cause: String::from("failed: x"),
            left_changed,
            unswept,
        }
    }

    #[test]
    fn a_stranded_bump_file_is_named_with_the_rename_that_restores_it() {
        assert_eq!(
            with_recovery(&rolled_back(vec![stranded("gone")], Vec::new())),
            "failed: x\n1 file(s) left changed:\n  .changeset/gone.md (moved to .changeset/.gone.md.oakum-write.1.2.0, and could not be moved back: nope)\nmove it back before re-running; a later run cannot see it and will version without it:\n  \".changeset/.gone.md.oakum-write.1.2.0\" -> \".changeset/gone.md\""
        );
    }

    /// The advice comes last, after the directories the rollback could not
    /// read, so it stays on screen.
    #[test]
    fn the_recovery_advice_follows_the_unswept_directories() {
        let text = with_recovery(&rolled_back(
            vec![stranded("gone")],
            vec![String::from(".changeset")],
        ));
        let unswept = text.find("could not be checked").expect("unswept list");
        let advice = text.find("move it back").expect("advice");
        assert!(unswept < advice, "{text}");
    }

    /// One restore failing is usually every restore failing, and the list of
    /// changed files stops at the cap, so a capped advice list would name the
    /// rest nowhere.
    #[test]
    fn every_stranded_file_is_listed_past_the_cap() {
        let entries = (0..HEAD_FILES + 2)
            .map(|n| stranded(&format!("s{n}")))
            .collect();
        let text = with_recovery(&rolled_back(entries, Vec::new()));
        assert!(text.contains("move them back before re-running"), "{text}");
        let last = HEAD_FILES + 1;
        assert!(
            text.ends_with(&format!(
                "\".changeset/.s{last}.md.oakum-write.1.2.0\" -> \".changeset/s{last}.md\""
            )),
            "{text}"
        );
    }

    #[test]
    fn a_failure_with_nothing_stranded_gets_no_advice() {
        let failure = rolled_back(
            vec![LeftChanged::Unrestored {
                path: String::from("a.toml"),
                err: String::from("x"),
            }],
            Vec::new(),
        );
        assert_eq!(with_recovery(&failure), failure.to_string());
    }

    #[test]
    fn importer_keys_match_what_pnpm_writes() {
        for (lock_dir, manifest_dir, importer) in [
            ("", "packages/b", "packages/b"),
            ("", "", "."),
            ("js", "js", "."),
            ("js", "js/packages/b", "packages/b"),
            ("js", "tools/e", "../tools/e"),
            ("js", "jsx/b", "../jsx/b"),
            ("js/app", "js/lib", "../lib"),
            ("pkg", "", ".."),
        ] {
            assert_eq!(
                pnpm_importer(lock_dir, manifest_dir),
                importer,
                "lock {lock_dir:?}, member {manifest_dir:?}"
            );
        }
    }
}
