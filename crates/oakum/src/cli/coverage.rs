use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use oakum::commits::packages_for_paths;
use oakum::plan::{BumpFile, PackageId, Workspace};

use super::generate::resolve_from_ref;
use super::CliError;

pub(super) fn uncovered_packages(
    repo: &Path,
    workspace: &Workspace,
    files: &[BumpFile],
    from: Option<&str>,
) -> Result<Vec<PackageId>, CliError> {
    let changed = changed_packages(repo, workspace, from)?;
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
    repo: &Path,
    workspace: &Workspace,
    from: Option<&str>,
) -> Result<BTreeSet<PackageId>, CliError> {
    let base = resolve_from_ref(repo, from).map_err(CliError::from_boxed)?;
    let paths = diff_paths(repo, &base)?
        .into_iter()
        .filter(|path| !is_intent_path(path))
        .collect::<Vec<_>>();
    let dirs: Vec<(PackageId, String)> = workspace
        .packages()
        .map(|package| (package.id().clone(), package.manifest_dir().to_owned()))
        .collect();
    Ok(packages_for_paths(&paths, &dirs))
}

fn diff_paths(repo: &Path, from: &str) -> Result<Vec<String>, CliError> {
    let output = Command::new("git")
        .args(["diff", "-z", "--name-only", &format!("{from}...HEAD")])
        .current_dir(repo)
        .output()
        .map_err(|err| CliError::unverified(format!("failed to run git diff: {err}")))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::unverified(format!(
            "git diff {from}...HEAD failed: {err}"
        )));
    }
    parse_nul_paths(&output.stdout)
}

fn is_intent_path(path: &str) -> bool {
    let path = path.trim_start_matches("./");
    path == ".changeset" || path.starts_with(".changeset/")
}

fn parse_nul_paths(stdout: &[u8]) -> Result<Vec<String>, CliError> {
    let mut paths = Vec::new();
    for record in stdout.split(|byte| *byte == 0) {
        if record.is_empty() {
            continue;
        }
        let path = String::from_utf8(record.to_vec()).map_err(|_| {
            CliError::unverified(
                "git diff listed a path that is not valid UTF-8; oakum cannot attribute it to a package",
            )
        })?;
        paths.push(path);
    }
    Ok(paths)
}
