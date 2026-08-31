//! `oakum version`: write planned manifests, inherited pins, lockfile rows, and changelogs, then delete consumed bump files.

use std::collections::BTreeMap;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use cap_std::fs::Dir;
use clap::Args;
use oakum::manifest::{
    cargo_package_version_inherits_workspace, retarget_cargo_lock, rewrite_dependencies,
    set_json_string, set_toml_string, CargoLockBump,
};
use oakum::plan::{aggregate, compose, CascadeAs, Ecosystem, Package, PackageId, Plan, Workspace};
use semver::Version;

use super::add::discover_workspace;
use super::changelog::{plan_changelog_writes, supplied_note, utc_date, ChangelogPlan};
use super::config::{enforce_tool_version, load_config};
use super::git::Git;
use super::inherited::{cargo_toml_path, plan_inherited_writes, read_text};
use super::intent::{load_plan_bump_files, COMMITS_BUMP_FILE_ID};
use super::repository;
use super::status::apply_package_overrides;
use super::template::{load_contained_file, load_template_body};
use super::write_set::{commit_write_set, PlannedDelete, PlannedWrite, WriteSet};
use super::CliError;

const PACKAGE_JSON: &str = "package.json";
const CARGO_TOML: &str = "Cargo.toml";
const CARGO_LOCK: &str = "Cargo.lock";
const CHANGESET_DIR: &str = ".changeset";

pub(super) struct VersionWritePlan {
    pub repo_path: PathBuf,
    pub writes: Vec<PlannedWrite>,
    pub deletes: Vec<PlannedDelete>,
    pub plan: Plan,
    pub tool_version: String,
    pub title: Option<oakum::template::TemplateSource>,
    pub commit_message: Option<oakum::template::TemplateSource>,
}

impl VersionWritePlan {
    pub(super) fn needs_github(&self) -> bool {
        !self.deletes.is_empty()
            || self
                .writes
                .iter()
                .any(|write| write.original() != write.next())
    }
}

#[derive(Debug, Args)]
pub(super) struct VersionArgs {
    /// Git ref to scan from (exclusive). Same default as `generate` / `status`.
    #[arg(long, value_name = "REF")]
    from: Option<String>,
    /// Release notes body. `-` reads stdin. Path cannot escape the checkout (ADR-0006).
    #[arg(long, value_name = "PATH")]
    notes_file: Option<PathBuf>,
}

pub(super) fn run(args: &VersionArgs) -> Result<(), Box<dyn std::error::Error>> {
    let prepared = plan_writes(args)?;
    let dir = Dir::open_ambient_dir(&prepared.repo_path, cap_std::ambient_authority())?;
    commit_write_set(&dir, &prepared.writes, &prepared.deletes)
}

pub(super) fn plan_writes(
    args: &VersionArgs,
) -> Result<VersionWritePlan, Box<dyn std::error::Error>> {
    let repo = repository::discover()?;
    let config = load_config(&repo)?;
    enforce_tool_version(&config)?;
    let workspace = apply_package_overrides(&discover_workspace(repo.path())?, &config)?;
    let git = Git::at(repo.path());
    let files = load_plan_bump_files(&git, repo.path(), &workspace, &config, args.from.as_deref())?;
    let consume_ids: Vec<String> = files
        .iter()
        .filter(|file| file.id != COMMITS_BUMP_FILE_ID)
        .map(|file| file.id.clone())
        .collect();
    let intent = aggregate(files);
    let plan = compose(
        &workspace,
        &intent,
        |id| config.versioning_for(&id.name),
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
    .map_err(|err| CliError::new(err.to_string()))?;

    let dir = Dir::open_ambient_dir(repo.path(), cap_std::ambient_authority())?;
    let new_versions = versions_from_plan(&plan);
    let mut write_set = WriteSet::new();
    write_set.extend(plan_inherited_writes(&dir, &workspace, &new_versions)?);
    plan_member_writes(&dir, &workspace, &plan, &mut write_set)?;
    write_set.extend(plan_lock_writes(&dir, &workspace, &plan)?);
    let date = utc_date(SystemTime::now())?;
    let tool_version = config
        .tool_version()
        .map_or_else(|| env!("CARGO_PKG_VERSION").to_owned(), Version::to_string);
    let template_body = match config.template() {
        Some(source) => Some(load_template_body(repo.dir(), repo.path(), source)?),
        None => None,
    };
    let supplied_notes = load_supplied_notes(&repo, args.notes_file.as_deref())?;
    write_set.extend(plan_changelog_writes(
        &dir,
        &workspace,
        &plan,
        &intent,
        &ChangelogPlan::new(
            &date,
            &tool_version,
            template_body.as_deref(),
            supplied_notes.as_deref(),
        ),
    )?);
    let deletes = plan_consume_deletes(&dir, &consume_ids)?;
    Ok(VersionWritePlan {
        repo_path: repo.path().to_owned(),
        writes: write_set.writes(),
        deletes,
        plan,
        tool_version,
        title: config.title().cloned(),
        commit_message: config.commit_message().cloned(),
    })
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

fn plan_consume_deletes(
    dir: &Dir,
    ids: &[String],
) -> Result<Vec<PlannedDelete>, Box<dyn std::error::Error>> {
    let mut deletes = Vec::new();
    for id in ids {
        let path = Path::new(CHANGESET_DIR).join(id);
        let original = read_text(dir, &path)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} is missing", path.display()),
            )
        })?;
        deletes.push(PlannedDelete::new(path, original));
    }
    Ok(deletes)
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
        let path = package_manifest_path(package);
        let (original, mut next) = write_set.source_text(dir, &path)?;
        if let Some(change) = bump {
            if package.id().ecosystem == Ecosystem::Cargo
                && cargo_package_version_inherits_workspace(&next)
                    .map_err(|err| format!("{}: {err}", path.display()))?
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
                        format!("{}: {err}", path.display()).into()
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
                format!("{}: {err}", path.display()).into()
            })?;
        }
        write_set.put_write(path, original, next);
    }
    Ok(())
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
            format!("{}: {err}", path.display()).into()
        })?;
    Ok(vec![PlannedWrite::new(path, original, next)])
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
            .map_err(|err| format!("{}: {err}", path.display()).into());
    }
    let (original, next) = write_set.source_text(dir, &path)?;
    let next = set_workspace_package_version(&next, from, to).map_err(
        |err| -> Box<dyn std::error::Error> { format!("{}: {err}", path.display()).into() },
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
        let path = cargo_toml_path(package);
        let (_, text) = write_set.source_text(dir, &path)?;
        if !cargo_package_version_inherits_workspace(&text)
            .map_err(|err| format!("{}: {err}", path.display()))?
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
    let root = workspace.cargo_workspace_root().ok_or_else(|| {
        CliError::new(
            "a cargo package inherits [package].version from the workspace but no cargo workspace root is set",
        )
    })?;
    Ok(if root.is_empty() {
        PathBuf::from(CARGO_TOML)
    } else {
        Path::new(root).join(CARGO_TOML)
    })
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

fn package_manifest_path(package: &Package) -> PathBuf {
    match package.id().ecosystem {
        Ecosystem::Cargo => cargo_toml_path(package),
        Ecosystem::Npm => {
            let dir = package.manifest_dir();
            if dir.is_empty() {
                PathBuf::from(PACKAGE_JSON)
            } else {
                Path::new(dir).join(PACKAGE_JSON)
            }
        }
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
