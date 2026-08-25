//! `oakum migrate`: transform data, report tooling (ADR-0003 / ADR-0023).
//!
//! Version gate first.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read};
use std::path::Path;

use cap_std::fs::{Dir, OpenOptions};
use clap::{Args, ValueEnum};
use oakum::changeset::{
    instruction_occupants, is_bump_file_name, parse_migration, resolve_package_name, write,
    KnopePresence, UnknownReason,
};
use oakum::detect::ReleaseTool;
use oakum::plan::{
    aggregate, apply_bump, compose, BumpFile, BumpLevel, CascadeAs, ChangeSource, PackageId, Plan,
    PlannedChange, Versioning, Workspace,
};
use semver::Version;

use super::add::{discover_workspace, NOTHING_TO_DISCOVER};

use super::config::{
    enforce_tool_version, read_config_source, write_file_via_rename, LoadedConfig,
};
use super::detect_tools;
use super::init::{
    binary_version, changeset_file_names, ensure_changeset_dir, print_workflow_and_footer,
    write_owned_files,
};
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
}

pub(super) fn run(args: &MigrateArgs) -> Result<(), Box<dyn std::error::Error>> {
    let repo = repository::discover()?;
    if let Some(source) = read_config_source(&repo)? {
        return already_migrated(&repo, &source, args.versioning);
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

    report_changeset_subdirs(repo.dir())?;

    let planned = plan_bump_rewrites(repo.dir(), &changeset_names, &bumpy_names, knope)?;
    let dropped = parse_dropped_config_keys(repo.dir())?;
    let workspace = optional_workspace(repo.path())?;
    let loaded = load_before_files(
        repo.dir(),
        &changeset_names,
        &bumpy_names,
        workspace.as_ref(),
    )?;
    let before_plan = before_plan(
        workspace.as_ref(),
        &loaded.files,
        infer_versioning(&report.detections),
        knope,
    )?;

    println!("pending:");
    for (path, _) in &planned {
        println!("  rewrite {path}");
    }
    println!("  write .changeset/_config.toml, .changeset/_schema.json, and .changeset/README.md");

    let binary = binary_version()?;
    ensure_changeset_dir(repo.dir())?;
    let rewritten = apply_bump_rewrites(repo.dir(), &planned)?;
    let created = write_owned_files(repo.dir(), &binary, versioning)?;

    let after_plan = after_plan(repo.dir(), workspace.as_ref(), &loaded.unknown, versioning);

    for path in &rewritten {
        println!("rewrote {path}");
    }
    for path in &created {
        println!("created {path}");
    }
    for key in &dropped {
        println!("dropped `{key}` from `.changeset/config.json` (not an oakum config key)");
    }
    let comparison = conclude_plan_comparison(
        workspace.as_ref(),
        &loaded.files,
        knope,
        before_plan.as_ref(),
        after_plan,
        loaded.unverified,
    );
    print_remaining_steps(&report.detections, knope);
    print_workflow_and_footer(&binary);
    comparison
}

fn already_migrated(
    repo: &super::repository::Repository,
    source: &super::config::ConfigSource,
    versioning_flag: Option<VersioningArg>,
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
    println!("already migrated");
    Ok(())
}

fn optional_workspace(path: &Path) -> Result<Option<Workspace>, Box<dyn std::error::Error>> {
    match discover_workspace(path) {
        Ok(workspace) => Ok(Some(workspace)),
        Err(err) if err.to_string() == NOTHING_TO_DISCOVER => Ok(None),
        Err(err) => Err(err),
    }
}

fn load_before_files(
    dir: &Dir,
    changeset_names: &[String],
    bumpy_names: &[String],
    workspace: Option<&Workspace>,
) -> Result<LoadedPlan, Box<dyn std::error::Error>> {
    let Some(workspace) = workspace else {
        let unverified = changeset_names
            .iter()
            .chain(bumpy_names)
            .any(|name| is_bump_file_name(name));
        if unverified {
            println!("plan comparison skipped: no packages discovered");
        } else {
            println!("plan comparison skipped: nothing to compare");
        }
        return Ok(LoadedPlan {
            unverified,
            ..LoadedPlan::default()
        });
    };
    let loaded = load_plan_files(dir, changeset_names, bumpy_names, workspace)?;
    for (path, name) in &loaded.unknown {
        println!("unknown package `{name}` in `{path}`");
    }
    Ok(loaded)
}

fn before_plan(
    workspace: Option<&Workspace>,
    files: &[BumpFile],
    versioning: Versioning,
    knope: bool,
) -> Result<Option<Plan>, Box<dyn std::error::Error>> {
    workspace
        .map(|workspace| compose_plan(workspace, files, versioning, knope))
        .transpose()
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
    let after_loaded = match load_plan_files(dir, &after_names, &[], workspace) {
        Ok(loaded) => loaded,
        Err(err) => return AfterPlan::Failed(err),
    };
    for (path, name) in &after_loaded.unknown {
        if !already_unknown
            .iter()
            .any(|(seen_path, seen)| seen == name && same_bump_file(seen_path, path))
        {
            println!("unknown package `{name}` in `{path}`");
        }
    }
    match compose_plan(workspace, &after_loaded.files, versioning, false) {
        Ok(plan) => AfterPlan::Compared(plan),
        Err(err) => AfterPlan::Failed(err),
    }
}

fn conclude_plan_comparison(
    workspace: Option<&Workspace>,
    files: &[BumpFile],
    knope: bool,
    before: Option<&Plan>,
    after: AfterPlan,
    unverified: bool,
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
    if unverified {
        return Err(Box::new(CliError::unverified(
            "unverified: migrated files were kept; plan comparison skipped; no packages discovered",
        )));
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

#[derive(Default)]
struct LoadedPlan {
    files: Vec<BumpFile>,
    unknown: Vec<(String, String)>,
    unverified: bool,
}

fn load_plan_files(
    dir: &Dir,
    changeset_names: &[String],
    bumpy_names: &[String],
    workspace: &Workspace,
) -> Result<LoadedPlan, Box<dyn std::error::Error>> {
    let mut loaded = LoadedPlan::default();
    for name in changeset_names {
        if !is_bump_file_name(name) {
            continue;
        }
        load_one_plan_file(
            dir,
            &format!(".changeset/{name}"),
            workspace,
            &mut loaded.files,
            &mut loaded.unknown,
        )?;
    }
    for name in bumpy_names {
        if !is_bump_file_name(name) {
            continue;
        }
        load_one_plan_file(
            dir,
            &format!(".bumpy/{name}"),
            workspace,
            &mut loaded.files,
            &mut loaded.unknown,
        )?;
    }
    Ok(loaded)
}

fn load_one_plan_file(
    dir: &Dir,
    rel: &str,
    workspace: &Workspace,
    files: &mut Vec<BumpFile>,
    unknown: &mut Vec<(String, String)>,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(body) = read_text(dir, rel)? else {
        return Err(Box::new(CliError::new(format!(
            "`{rel}` was listed but is missing"
        ))));
    };
    let parsed = parse_migration(&body)
        .map_err(|err| CliError::new(format!("`{rel}` is not a bump file: {err}")))?;
    let mut entries = Vec::new();
    for (name, level) in parsed.entries() {
        match resolve_package_name(name, workspace) {
            Ok(id) => entries.push((id, *level)),
            Err(UnknownReason::Missing) => unknown.push((rel.to_string(), name.clone())),
            Err(UnknownReason::Ambiguous) => {
                return Err(Box::new(CliError::new(format!(
                    "package `{name}` in `{rel}` matches more than one workspace package"
                ))));
            }
        }
    }
    files.push(BumpFile {
        id: rel.to_string(),
        entries,
        note: String::from(parsed.note()),
    });
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

fn plan_fingerprint(plan: &Plan) -> BTreeMap<PackageId, (Version, Version)> {
    plan.changes()
        .iter()
        .map(|(id, change)| (id.clone(), (change.from().clone(), change.to().clone())))
        .collect()
}

fn same_bump_file(left: &str, right: &str) -> bool {
    Path::new(left).file_name() == Path::new(right).file_name()
}

fn bumped(from: &Version, level: BumpLevel) -> Option<Version> {
    apply_bump(from, level, Versioning::ZeroMajor)
        .ok()
        .map(|(next, _)| next)
}

fn is_knope_feature_versions(
    before: Option<&(Version, Version)>,
    after: Option<&(Version, Version)>,
) -> bool {
    let Some((before_from, before_to)) = before else {
        return false;
    };
    let Some((after_from, after_to)) = after else {
        return false;
    };
    if before_from != after_from {
        return false;
    }
    let Some(patch) = bumped(before_from, BumpLevel::Patch) else {
        return false;
    };
    let Some(minor) = bumped(before_from, BumpLevel::Minor) else {
        return false;
    };
    before_to == &patch && after_to == &minor
}

fn knope_feature_fallout(
    knope: bool,
    features: &BTreeSet<PackageId>,
    before: &Plan,
    after: &Plan,
    before_fp: &BTreeMap<PackageId, (Version, Version)>,
    after_fp: &BTreeMap<PackageId, (Version, Version)>,
    id: &PackageId,
) -> bool {
    if !knope {
        return false;
    }
    let canonical: BTreeSet<PackageId> = features
        .iter()
        .filter(|feature| {
            is_knope_feature_versions(before_fp.get(*feature), after_fp.get(*feature))
        })
        .cloned()
        .collect();
    if canonical.contains(id) {
        return true;
    }
    cascaded_from_feature(after, &canonical, id) || cascaded_from_feature(before, &canonical, id)
}

fn cascaded_from_feature(plan: &Plan, features: &BTreeSet<PackageId>, id: &PackageId) -> bool {
    let mut current = id.clone();
    let mut seen = BTreeSet::new();
    while seen.insert(current.clone()) {
        match plan.get(&current).map(PlannedChange::source) {
            Some(ChangeSource::Cascade { trigger }) => {
                if features.contains(trigger) {
                    return true;
                }
                current = trigger.clone();
            }
            _ => return false,
        }
    }
    false
}

fn knope_feature_ids(workspace: &Workspace, files: &[BumpFile]) -> BTreeSet<PackageId> {
    aggregate(files.to_vec())
        .into_iter()
        .filter(|(id, bump)| {
            bump.level() == BumpLevel::Minor
                && workspace
                    .get(id)
                    .is_some_and(|pkg| pkg.version().major == 0)
        })
        .map(|(id, _)| id)
        .collect()
}

fn format_side(versions: Option<&(Version, Version)>) -> String {
    match versions {
        Some((from, to)) => format!("{from} → {to}"),
        None => String::from("absent"),
    }
}

fn report_plan_comparison(
    workspace: &Workspace,
    files: &[BumpFile],
    knope: bool,
    before: &Plan,
    after: &Plan,
) -> bool {
    let before_fp = plan_fingerprint(before);
    let after_fp = plan_fingerprint(after);
    if before_fp == after_fp {
        return false;
    }
    let features = knope_feature_ids(workspace, files);
    let mut ids: BTreeSet<PackageId> = before_fp.keys().cloned().collect();
    ids.extend(after_fp.keys().cloned());
    let mut expected = Vec::new();
    let mut unexpected = Vec::new();
    for id in ids {
        let before_v = before_fp.get(&id);
        let after_v = after_fp.get(&id);
        if before_v == after_v {
            continue;
        }
        if knope_feature_fallout(knope, &features, before, after, &before_fp, &after_fp, &id) {
            expected.push((id, format_side(before_v), format_side(after_v)));
        } else {
            unexpected.push((id, format_side(before_v), format_side(after_v)));
        }
    }
    if unexpected.is_empty() {
        println!(
            "plan comparison: knope maps a pending feature on a pre-1.0 package to patch; oakum maps it to minor"
        );
        for (id, knope_side, oakum_side) in expected {
            println!("  {id}: {knope_side} (knope) vs {oakum_side} (oakum)");
        }
        return false;
    }
    println!("plan comparison: unexpected difference");
    for (id, knope_side, oakum_side) in expected.into_iter().chain(unexpected) {
        println!("  {id}: {knope_side} vs {oakum_side}");
    }
    true
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

fn plan_bump_rewrites(
    dir: &Dir,
    changeset_names: &[String],
    bumpy_names: &[String],
    knope: bool,
) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    let mut planned = Vec::new();
    let mut dests = Vec::new();
    for name in changeset_names {
        if !is_bump_file_name(name) {
            continue;
        }
        let rel = format!(".changeset/{name}");
        let Some(body) = read_text(dir, &rel)? else {
            continue;
        };
        let parsed = parse_migration(&body).map_err(|err| {
            CliError::new(format!("`.changeset/{name}` is not a bump file: {err}"))
        })?;
        refuse_knope_unsafe(&rel, &parsed, knope)?;
        let next =
            write(parsed.entries(), parsed.note(), KnopePresence::Absent).map_err(|err| {
                CliError::new(format!("failed to rewrite `.changeset/{name}`: {err}"))
            })?;
        dests.push(name.clone());
        if next != body {
            planned.push((rel, next));
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
            continue;
        };
        let parsed = parse_migration(&body)
            .map_err(|err| CliError::new(format!("`.bumpy/{name}` is not a bump file: {err}")))?;
        refuse_knope_unsafe(&src, &parsed, knope)?;
        let next = write(parsed.entries(), parsed.note(), KnopePresence::Absent)
            .map_err(|err| CliError::new(format!("failed to rewrite `.bumpy/{name}`: {err}")))?;
        planned.push((format!(".changeset/{name}"), next));
        dests.push(name.clone());
    }
    Ok(planned)
}

fn refuse_knope_unsafe(
    path: &str,
    parsed: &oakum::changeset::ChangeFile,
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

fn apply_bump_rewrites(
    dir: &Dir,
    planned: &[(String, String)],
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut rewritten = Vec::new();
    for (rel, body) in planned {
        write_file_via_rename(dir, Path::new(rel), body)?;
        rewritten.push(rel.clone());
    }
    Ok(rewritten)
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
mod knope_feature_versions {
    use super::is_knope_feature_versions;
    use semver::Version;

    fn v(major: u64, minor: u64, patch: u64) -> Version {
        Version::new(major, minor, patch)
    }

    #[test]
    fn accepts_patch_versus_minor_on_the_same_from() {
        let from = v(0, 1, 0);
        assert!(is_knope_feature_versions(
            Some(&(from.clone(), v(0, 1, 1))),
            Some(&(from, v(0, 2, 0))),
        ));
    }

    #[test]
    fn rejects_a_non_minor_oakum_target() {
        let from = v(0, 1, 0);
        assert!(!is_knope_feature_versions(
            Some(&(from.clone(), v(0, 1, 1))),
            Some(&(from, v(0, 3, 0))),
        ));
    }

    #[test]
    fn rejects_missing_sides_and_a_from_mismatch() {
        let from = v(0, 1, 0);
        assert!(!is_knope_feature_versions(None, None));
        assert!(!is_knope_feature_versions(
            None,
            Some(&(from.clone(), v(0, 2, 0))),
        ));
        assert!(!is_knope_feature_versions(
            Some(&(from.clone(), v(0, 1, 1))),
            Some(&(v(0, 2, 0), v(0, 3, 0))),
        ));
        assert!(!is_knope_feature_versions(
            Some(&(from.clone(), v(0, 2, 0))),
            Some(&(from, v(0, 2, 0))),
        ));
    }
}
