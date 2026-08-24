//! `oakum migrate`: transform data, report tooling (ADR-0003 / ADR-0023).
//!
//! Version gate first.

use std::io::{self, Read};
use std::path::Path;

use cap_std::fs::{Dir, OpenOptions};
use clap::{Args, ValueEnum};
use oakum::changeset::{
    instruction_occupants, is_bump_file_name, parse_migration, write, KnopePresence,
};
use oakum::detect::ReleaseTool;
use oakum::plan::{BumpLevel, Versioning};

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
        return already_migrated(&source, args.versioning);
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

    let planned = plan_bump_rewrites(repo.dir(), &changeset_names, &bumpy_names, knope)?;
    let dropped = parse_dropped_config_keys(repo.dir())?;

    let binary = binary_version()?;
    ensure_changeset_dir(repo.dir())?;
    let rewritten = apply_bump_rewrites(repo.dir(), &planned)?;
    let created = write_owned_files(repo.dir(), &binary, versioning)?;

    for path in &rewritten {
        println!("rewrote {path}");
    }
    for path in &created {
        println!("created {path}");
    }
    for key in &dropped {
        println!("dropped `{key}` from `.changeset/config.json` (not an oakum config key)");
    }
    print_remaining_steps(&report.detections, knope);
    print_workflow_and_footer(&binary);
    Ok(())
}

fn already_migrated(
    source: &super::config::ConfigSource,
    versioning_flag: Option<VersioningArg>,
) -> Result<(), Box<dyn std::error::Error>> {
    let parsed = oakum::config::parse(source.text()).map_err(|err| {
        CliError::new(format!(
            "`.changeset/_config.toml` is not a valid oakum config: {err}"
        ))
    })?;
    let loaded = LoadedConfig::from_parsed(parsed);
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
