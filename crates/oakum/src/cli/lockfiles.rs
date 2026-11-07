//! `check`'s lockfiles look: the specifiers `pnpm-lock.yaml` records for each
//! workspace project and catalog entry, against what the manifests declare.
//! Reads only; `pnpm install` is named, never run.

use std::path::Path;

use oakum::manifest::{pnpm_lock_drift, PnpmManifest};
use oakum::plan::Workspace;

use super::fs::{read_resolved_text, repo_path_display};
use super::repository::Repository;
use super::verdict::{LookReport, Refusal};
use super::version::{pnpm_importer, PNPM_LOCK};
use super::CliError;

/// Says nothing without a pnpm workspace or without its lockfile.
pub(super) fn evaluate_pnpm_lock(repo: &Repository, workspace: &Workspace) -> LookReport {
    match pnpm_lock_report(repo, workspace) {
        Ok(report) => report,
        Err(refusal) => LookReport::refusing(vec![Refusal::bare(refusal)]),
    }
}

fn pnpm_lock_report(repo: &Repository, workspace: &Workspace) -> Result<LookReport, CliError> {
    let Some(lock_dir) = workspace.pnpm_lock_dir() else {
        return Ok(LookReport::default());
    };
    let path = Path::new(lock_dir).join(PNPM_LOCK);
    let Some(text) = read_resolved_text(repo.dir(), &path)? else {
        return Ok(LookReport::default());
    };
    let shown = repo_path_display(&path);

    // The projects pnpm lists today, not every importer the lockfile kept.
    let mut projects: Vec<&str> = vec![lock_dir];
    for project in workspace.pnpm_projects() {
        if !projects.contains(&project.as_str()) {
            projects.push(project);
        }
    }
    let manifests: Vec<PnpmManifest> = projects
        .into_iter()
        .map(|dir| PnpmManifest {
            importer: pnpm_importer(lock_dir, dir),
            text: read_resolved_text(repo.dir(), &Path::new(dir).join("package.json"))
                .map_err(|err| err.detail()),
        })
        .collect();
    let workspace_file =
        read_resolved_text(repo.dir(), &Path::new(lock_dir).join("pnpm-workspace.yaml"))?;

    let drift = pnpm_lock_drift(&text, lock_dir, &manifests, workspace_file.as_deref())
        .map_err(|why| CliError::unverified(format!("unverified: `{shown}`: {why}")))?;
    let mut refusals = Vec::new();
    if !drift.drifted.is_empty() {
        let fix = if lock_dir.is_empty() {
            String::from("run `pnpm install` to update it")
        } else {
            format!("run `pnpm install` in `{lock_dir}` to update it")
        };
        refusals.push(Refusal {
            error: CliError::new(format!(
                "`{shown}` has drifted from {} manifest or catalog entr{}; {fix}",
                drift.drifted.len(),
                plural(drift.drifted.len())
            )),
            lines: drift.drifted.iter().map(ToString::to_string).collect(),
        });
    }
    if !drift.unread.is_empty() {
        refusals.push(Refusal {
            error: CliError::unverified(format!(
                "unverified: {} `{shown}` entr{} could not be compared",
                drift.unread.len(),
                plural(drift.unread.len())
            )),
            lines: drift.unread,
        });
    }
    if !refusals.is_empty() {
        return Ok(LookReport::refusing(refusals));
    }
    // A clean comparison stays silent; a skip is said, so it never reads as compared.
    if drift.skipped == 0 {
        return Ok(LookReport::default());
    }
    Ok(LookReport {
        lines: vec![format!(
            "lockfiles: `{shown}` compared importer and catalog specifiers; {} entr{} under overrides, ignoredOptionalDependencies or packageExtensions skipped",
            drift.skipped,
            plural(drift.skipped)
        )],
        ..LookReport::default()
    })
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        "y"
    } else {
        "ies"
    }
}
