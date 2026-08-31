use std::collections::BTreeSet;

use oakum::commits::packages_for_paths;
use oakum::plan::{BumpFile, PackageId, Workspace};

use super::generate::resolve_from_ref;
use super::git::{Git, Op};
use super::CliError;

pub(super) fn uncovered_packages(
    git: &Git,
    workspace: &Workspace,
    files: &[BumpFile],
    from: Option<&str>,
) -> Result<Vec<PackageId>, CliError> {
    let changed = changed_packages(git, workspace, from)?;
    if changed.is_empty() {
        return Ok(Vec::new());
    }
    let covered = covered_packages(files, &changed);
    Ok(changed
        .into_iter()
        .filter(|id| !covered.contains(id))
        .collect())
}

fn covered_packages(files: &[BumpFile], changed: &BTreeSet<PackageId>) -> BTreeSet<PackageId> {
    let mut covered = BTreeSet::new();
    let mut empty_file = false;
    for file in files {
        if file.entries.is_empty() {
            empty_file = true;
            continue;
        }
        for (id, _) in &file.entries {
            covered.insert(id.clone());
        }
    }
    if empty_file {
        covered.extend(changed.iter().cloned());
    }
    covered
}

fn changed_packages(
    git: &Git,
    workspace: &Workspace,
    from: Option<&str>,
) -> Result<BTreeSet<PackageId>, CliError> {
    let base = resolve_from_ref(git, from).map_err(CliError::from_boxed)?;
    let paths = diff_paths(git, &base)?
        .into_iter()
        .filter(|path| !is_intent_path(path))
        .collect::<Vec<_>>();
    let dirs: Vec<(PackageId, String)> = workspace
        .packages()
        .map(|package| (package.id().clone(), package.manifest_dir().to_owned()))
        .collect();
    Ok(packages_for_paths(&paths, &dirs))
}

fn diff_paths(git: &Git, from: &str) -> Result<Vec<String>, CliError> {
    git.paths(Op::ChangedPaths { from })
}

fn is_intent_path(path: &str) -> bool {
    let path = path.trim_start_matches("./");
    path == ".changeset" || path.starts_with(".changeset/")
}
