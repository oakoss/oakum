//! `oakum version`: write planned manifests, inherited pins, and lockfile rows, then delete consumed bump files.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use cap_std::fs::Dir;
use clap::Args;
use oakum::manifest::{
    retarget_cargo_lock, rewrite_dependencies, set_json_string, set_toml_string, CargoLockBump,
};
use oakum::plan::{aggregate, compose, CascadeAs, Ecosystem, Package, PackageId, Plan, Workspace};
use semver::Version;

use super::add::discover_workspace;
use super::config::{enforce_tool_version, load_config};
use super::inherited::{cargo_toml_path, plan_inherited_writes, read_text};
use super::intent::{load_plan_bump_files, COMMITS_BUMP_FILE_ID};
use super::repository;
use super::status::apply_package_overrides;
use super::write_set::{commit_write_set, PlannedDelete, PlannedWrite};
use super::CliError;

const PACKAGE_JSON: &str = "package.json";
const CARGO_LOCK: &str = "Cargo.lock";
const CHANGESET_DIR: &str = ".changeset";

#[derive(Debug, Args)]
pub(super) struct VersionArgs {
    /// Git ref to scan from (exclusive). Same default as `generate` / `status`.
    #[arg(long, value_name = "REF")]
    from: Option<String>,
}

pub(super) fn run(args: &VersionArgs) -> Result<(), Box<dyn std::error::Error>> {
    let repo = repository::discover()?;
    let config = load_config(&repo)?;
    enforce_tool_version(&config)?;
    let workspace = apply_package_overrides(&discover_workspace(repo.path())?, &config)?;
    let files = load_plan_bump_files(repo.path(), &workspace, &config, args.from.as_deref())?;
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
    let mut writes = plan_inherited_writes(&dir, &workspace, &new_versions)?;
    plan_member_writes(&dir, &workspace, &plan, &mut writes)?;
    writes.extend(plan_lock_writes(&dir, &workspace, &plan)?);
    let deletes = plan_consume_deletes(&dir, &consume_ids)?;
    commit_write_set(&dir, &writes, &deletes)
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
    writes: &mut Vec<PlannedWrite>,
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
        let (original, mut next) = source_text(dir, writes, &path)?;
        if let Some(change) = bump {
            next = bump_package_version(package.id().ecosystem, &next, change.to()).map_err(
                |err| -> Box<dyn std::error::Error> { format!("{}: {err}", path.display()).into() },
            )?;
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
        put_write(writes, path, original, next);
    }
    Ok(())
}

fn source_text(
    dir: &Dir,
    writes: &[PlannedWrite],
    path: &Path,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    if let Some(write) = writes.iter().find(|write| write.path() == path) {
        return Ok((write.original().to_owned(), write.next().to_owned()));
    }
    let text = read_text(dir, path)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} is missing", path.display()),
        )
    })?;
    Ok((text.clone(), text))
}

fn put_write(writes: &mut Vec<PlannedWrite>, path: PathBuf, original: String, next: String) {
    if let Some(write) = writes.iter_mut().find(|write| write.path() == path) {
        write.set_next(next);
        return;
    }
    writes.push(PlannedWrite::new(path, original, next));
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
