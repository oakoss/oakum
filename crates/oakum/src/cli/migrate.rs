//! `oakum migrate`: transform data, report tooling (ADR-0003 / ADR-0023).
//!
//! Version gate first.

use std::io::{self, IsTerminal, Read, Write};
use std::path::Path;

use cap_std::fs::{Dir, OpenOptions};
use clap::{Args, ValueEnum};
use oakum::changeset::{
    instruction_occupants, is_bump_file_name, load_migration_bump_files, parse_migration,
    resolve_migration_change, write, ChangeFile, KnopePresence, LoadError, MalformedBumpFile,
    MigrationBumpFile, MigrationLoadAbort, UnknownReason,
};
use oakum::detect::ReleaseTool;
use oakum::plan::{
    aggregate, compare_plans, compose, format_versions, plan_fingerprint, BumpFile, BumpLevel,
    CascadeAs, Plan, PlanFingerprint, Versioning, Workspace,
};

use super::add::try_discover_workspace;

use super::ci::VERSION_BRANCH;
use super::config::{enforce_tool_version, read_config_source, LoadedConfig};
use super::detect_tools;
use super::fs::write_file_via_rename;
use super::init::{
    binary_version, changeset_file_names, ensure_changeset_dir, list_paths, missing_owned_files,
    print_workflow_and_footer, regular_file_exists, restore_owned_file, write_owned_files,
    WorkflowPins, README_REL, SCHEMA_REL,
};
use super::migrate_source_plan::{fetch_source_before_plan, primary_plan_tool, SourceBeforePlan};
use super::repository;
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

#[derive(Debug, Args)]
pub(super) struct MigrateArgs {
    /// Override versioning inferred from the source tool.
    #[arg(long, value_enum)]
    versioning: Option<VersioningArg>,

    /// Skip the confirmation prompt.
    #[arg(long)]
    yes: bool,
}

pub(super) fn run(args: &MigrateArgs) -> Result<(), Box<dyn std::error::Error>> {
    let repo = repository::discover()?;
    if let Some(source) = read_config_source(&repo)? {
        return already_migrated(&repo, &source, args.versioning, args.yes);
    }

    let report = detect_tools::scan(repo.dir())?;
    if !report.errors.is_empty() {
        for hit in &report.detections {
            println!("{}\t{}", hit.tool().name(), hit.evidence());
        }
        let joined = report
            .errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        return Err(Box::new(CliError::unverified(format!(
            "unverified: {joined}"
        ))));
    }
    if report.detections.is_empty() {
        return Err(Box::new(CliError::new(
            "nothing to migrate; run `oakum init`",
        )));
    }

    let knope = knope_present(repo.dir())?;
    let changeset_names = changeset_file_names(repo.dir())?;
    let bumpy = report
        .detections
        .iter()
        .any(|hit| hit.tool() == ReleaseTool::Bumpy);
    let changesets = report
        .detections
        .iter()
        .any(|hit| hit.tool() == ReleaseTool::Changesets);
    let bumpy_names = if bumpy {
        dir_file_names(repo.dir(), ".bumpy")?
    } else {
        Vec::new()
    };
    for occupant in instruction_occupants(changeset_names.iter().map(String::as_str)) {
        println!("{}", occupant.migrate_message());
    }

    let versioning = args.versioning.map_or_else(
        || infer_versioning(&report.detections),
        VersioningArg::to_versioning,
    );

    // The write's own predicates, run before anything prints or is written:
    // a README or schema that is a directory or symlink refuses here, not
    // after the bump files were rewritten.
    let readme_present = regular_file_exists(repo.dir(), README_REL)?;
    let schema_present = regular_file_exists(repo.dir(), SCHEMA_REL)?;
    report_changeset_subdirs(repo.dir())?;

    let dropped = parse_dropped_config_keys(repo.dir())?;
    let workspace = optional_workspace(&repo)?;
    let prepared = prepare_migration(
        repo.dir(),
        &changeset_names,
        &bumpy_names,
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

    print_pending(&prepared.rewrites, &dropped, readme_present, schema_present);
    confirm_migration(args.yes)?;
    // The prompt can wait a while; look again before the first write.
    regular_file_exists(repo.dir(), README_REL)?;
    regular_file_exists(repo.dir(), SCHEMA_REL)?;

    let binary = binary_version()?;
    let pins = WorkflowPins::lookup(repo.ambient_path()?)?;
    ensure_changeset_dir(repo.dir())?;
    apply_bump_rewrites(repo.dir(), &prepared.rewrites)?;
    let created = write_owned_files(repo.dir(), &binary, true, true, versioning)?;

    let after_plan = after_plan(
        repo.dir(),
        workspace.as_ref(),
        &prepared.unknown_pairs(),
        versioning,
    );
    print_left_alone(&created.written, &dropped);
    let comparison = conclude_plan_comparison(
        workspace.as_ref(),
        &before_files,
        knope,
        before.as_ref(),
        after_plan,
        prepared.unverified,
    );
    print_remaining_steps(&report.detections, knope);
    print_workflow_and_footer(&binary, &pins, &created.written);
    comparison
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

/// The pending line for the owned files, from the same facts the writes use.
fn pending_owned_line(readme_present: bool, schema_present: bool) -> String {
    let schema = if schema_present {
        "replace the existing .changeset/_schema.json"
    } else {
        "write .changeset/_schema.json"
    };
    if readme_present {
        format!("write .changeset/_config.toml and {schema} (keeping the existing .changeset/README.md)")
    } else {
        format!("write .changeset/_config.toml and .changeset/README.md, and {schema}")
    }
}

fn print_pending(
    planned: &[(String, String)],
    dropped: &[String],
    readme_present: bool,
    schema_present: bool,
) {
    println!("pending:");
    for (path, _) in planned {
        println!("  rewrite {path}");
    }
    println!("  {}", pending_owned_line(readme_present, schema_present));
    for key in dropped {
        println!("  leave `{key}` behind in `.changeset/config.json` (not an oakum config key)");
    }
}

fn print_left_alone(written: &[&str], dropped: &[String]) {
    if !written.contains(&README_REL) {
        println!("kept {README_REL} (oakum did not write it; left as is)");
    }
    for key in dropped {
        println!(
            "not carried over: `{key}` (not an oakum config key; `.changeset/config.json` is untouched)"
        );
    }
}

fn skip_migration_confirmation(yes: bool, stdin_is_tty: bool) -> bool {
    yes || !stdin_is_tty
}

fn confirm_migration(yes: bool) -> Result<(), Box<dyn std::error::Error>> {
    if skip_migration_confirmation(yes, io::stdin().is_terminal()) {
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
    let missing = missing_owned_files(repo.dir())?;
    if !missing.is_empty() {
        let rels: Vec<&str> = missing.iter().map(|file| file.rel()).collect();
        println!("pending:");
        println!("  write {}", list_paths(&rels));
        confirm_migration(yes)?;
        for file in missing {
            restore_owned_file(repo.dir(), file)?;
            println!("created {}", file.rel());
        }
    }
    println!("already migrated");
    Ok(())
}

fn optional_workspace(
    repo: &repository::Repository,
) -> Result<Option<Workspace>, Box<dyn std::error::Error>> {
    try_discover_workspace(repo)
}

#[derive(Default)]
struct PreparedMigration {
    rewrites: Vec<(String, String)>,
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
            prepared.rewrites.push((rel.clone(), next));
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
        prepared.rewrites.push((format!(".changeset/{name}"), next));
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

/// Library collects malformed; migrate refuses every report.
fn refuse_migration_malformed(malformed: &[MalformedBumpFile]) -> Result<(), CliError> {
    use std::fmt::Write as _;

    if malformed.is_empty() {
        return Ok(());
    }
    let mut message = String::new();
    for (i, report) in malformed.iter().enumerate() {
        if i > 0 {
            message.push_str("; also ");
        }
        let _ = write!(
            message,
            "`{}` is not a bump file: {}",
            report.file, report.error
        );
    }
    Err(CliError::new(message))
}

/// Before fingerprint for plan comparison (`okm-45t.1`).
enum BeforeProof {
    Source {
        tool: ReleaseTool,
        fingerprint: PlanFingerprint,
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
        SourceBeforePlan::Available { tool, fingerprint } => {
            println!("plan comparison: before-plan from {}", tool.name());
            Ok(Some(BeforeProof::Source { tool, fingerprint }))
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
    refuse_migration_malformed(loaded.malformed())?;
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

fn infer_versioning(detections: &[oakum::detect::Detection]) -> Versioning {
    if detections
        .iter()
        .any(|hit| hit.tool() == ReleaseTool::Knope)
    {
        Versioning::ZeroMajor
    } else {
        Versioning::Semver
    }
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

fn report_plan_comparison(
    workspace: &Workspace,
    files: &[BumpFile],
    knope: bool,
    before: &BeforeProof,
    after: &Plan,
) -> bool {
    let simulated_fp;
    let (before_fp, before_plan, before_label, planned_by) = match before {
        BeforeProof::Source { tool, fingerprint } => (
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
    if let Some(diffs) = comparison.expected_knope_feature() {
        println!(
            "plan comparison: knope maps a pending feature on a pre-1.0 package to patch; oakum maps it to minor"
        );
        for diff in diffs {
            println!(
                "  {}: {} ({before_label}) vs {} (oakum)",
                diff.id(),
                format_versions(diff.before()),
                format_versions(diff.after()),
            );
        }
        return false;
    }
    if let Some(parts) = comparison.unexpected() {
        println!("plan comparison: unexpected difference");
        for diff in parts.expected.iter().chain(parts.unexpected.iter()) {
            println!(
                "  {}: {} vs {}",
                diff.id(),
                format_versions(diff.before()),
                format_versions(diff.after()),
            );
        }
        return true;
    }
    println!(
        "plan comparison: {} package(s) {planned_by} and by oakum; match{match_suffix}",
        after.changes().len()
    );
    false
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

/// Prints each `rewrote` line as the write lands, so a failure part-way
/// through leaves an accurate record.
fn apply_bump_rewrites(
    dir: &Dir,
    planned: &[(String, String)],
) -> Result<(), Box<dyn std::error::Error>> {
    for (rel, body) in planned {
        write_file_via_rename(dir, Path::new(rel), body)?;
        println!("rewrote {rel}");
    }
    Ok(())
}

fn parse_dropped_config_keys(dir: &Dir) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let Some(body) = read_text(dir, ".changeset/config.json")? else {
        return Ok(Vec::new());
    };
    let value: serde_json::Value = serde_json::from_str(&body).map_err(|err| {
        CliError::new(format!("`.changeset/config.json` is not valid JSON: {err}"))
    })?;
    let Some(object) = value.as_object() else {
        return Err(Box::new(CliError::new(
            "`.changeset/config.json` is not a JSON object",
        )));
    };
    let mut keys: Vec<String> = object.keys().cloned().collect();
    keys.sort();
    Ok(keys)
}

fn read_text(dir: &Dir, path: &str) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let mut file = match dir.open_with(path, &options) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(Box::new(CliError::new(format!(
                "failed to open `{path}`: {err}"
            ))));
        }
    };
    let meta = file
        .metadata()
        .map_err(|err| CliError::new(format!("failed to inspect `{path}`: {err}")))?;
    if !meta.is_file() {
        return Err(Box::new(CliError::new(format!(
            "`{path}` is not a regular file"
        ))));
    }
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|err| CliError::new(format!("failed to read `{path}`: {err}")))?;
    Ok(Some(text))
}

fn print_remaining_steps(detections: &[oakum::detect::Detection], knope: bool) {
    println!("remaining (oakum does not perform these):");
    println!("- add oakum to a workflow (YAML printed below)");
    println!(
        "- publish: `oakum release` only tags and creates the GitHub release; the old workflow's publish step (`npm publish`, `cargo publish`) needs a job of its own on the tag push (`on: push: tags`)"
    );
    println!(
        "- the version PR opens on branch `{VERSION_BRANCH}`; add it to any branch-name filters that need it"
    );
    for hit in detections {
        if let Some(path) = remaining_removal(hit.evidence()) {
            println!("- remove {path} ({})", hit.tool().name());
        }
    }
    println!("- remove the old tool's dependency and its workflow");
    if knope {
        println!(
            "- `.changeset/README.md` aborts knope until `knope.toml` and its workflow are removed"
        );
    }
}

/// `.changeset/` is oakum's directory after migrate. Only the old changesets
/// config file is still foreign.
fn remaining_removal(evidence: &str) -> Option<&str> {
    if evidence == ".changeset/" {
        return None;
    }
    if evidence.starts_with(".changeset/") && evidence != ".changeset/config.json" {
        return None;
    }
    Some(evidence)
}

#[cfg(test)]
mod confirmation {
    use super::{accept_migration_answer, skip_migration_confirmation};

    #[test]
    fn skip_when_yes_or_non_tty() {
        assert!(skip_migration_confirmation(true, true));
        assert!(skip_migration_confirmation(true, false));
        assert!(skip_migration_confirmation(false, false));
        assert!(!skip_migration_confirmation(false, true));
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
    use super::refuse_migration_malformed;
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
        refuse_migration_malformed(&[]).expect("empty");
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
        let err = refuse_migration_malformed(loaded.malformed()).expect_err("refuse");
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

#[cfg(test)]
mod pending_wording {
    use super::pending_owned_line;

    #[test]
    fn every_owned_file_state_has_its_sentence() {
        assert_eq!(
            pending_owned_line(false, false),
            "write .changeset/_config.toml and .changeset/README.md, and write .changeset/_schema.json"
        );
        assert_eq!(
            pending_owned_line(false, true),
            "write .changeset/_config.toml and .changeset/README.md, and replace the existing .changeset/_schema.json"
        );
        assert_eq!(
            pending_owned_line(true, false),
            "write .changeset/_config.toml and write .changeset/_schema.json (keeping the existing .changeset/README.md)"
        );
        assert_eq!(
            pending_owned_line(true, true),
            "write .changeset/_config.toml and replace the existing .changeset/_schema.json (keeping the existing .changeset/README.md)"
        );
    }
}
