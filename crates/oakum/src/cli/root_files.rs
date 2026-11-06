//! Both sides of a Cargo workspace's root `Cargo.toml` and `Cargo.lock`, read
//! for [`root_changes`] (ADR-0039), which compares them.

use std::collections::{BTreeMap, BTreeSet};

use oakum::manifest::{root_changes, RootChanges, RootFile, RootFiles, RootReason};
use oakum::plan::{Ecosystem, Package, PackageId, Workspace};
use oakum::state::Through;

use super::git::{Commit, Git};
use super::CliError;

#[derive(Debug)]
struct Paths {
    manifest: String,
    lock: String,
}

impl Paths {
    fn of(workspace: &Workspace) -> Option<Self> {
        let root = workspace.cargo_workspace_root()?;
        let at = |name: &str| {
            if root.is_empty() {
                name.to_owned()
            } else {
                format!("{root}/{name}")
            }
        };
        Some(Self {
            manifest: at("Cargo.toml"),
            lock: at("Cargo.lock"),
        })
    }

    fn path(&self, file: RootFile) -> &str {
        match file {
            RootFile::Manifest => &self.manifest,
            RootFile::Lock => &self.lock,
        }
    }
}

/// What a change to the root files reached; empty when neither changed.
#[derive(Debug, Default)]
pub(super) struct RootReach {
    changes: RootChanges,
    paths: Option<Paths>,
}

impl RootReach {
    /// The packages to walk [`oakum::plan::reach`] from: the members a root
    /// change named, and for a workspace-wide one every Cargo package `managed`
    /// accepts.
    pub(super) fn changed(
        &self,
        workspace: &Workspace,
        managed: impl Fn(&Package) -> bool,
    ) -> BTreeSet<PackageId> {
        let mut changed: BTreeSet<PackageId> = self.changes.members.keys().cloned().collect();
        if !self.changes.everywhere.is_empty() {
            changed.extend(
                cargo_packages(workspace)
                    .filter(|package| managed(package))
                    .map(|package| package.id().clone()),
            );
        }
        changed
    }

    /// Each managed package a root change reached, with every reason that
    /// reached it: a lock row through one internal crate and a manifest entry
    /// through another both land on the package that ships them.
    pub(super) fn through(
        &self,
        workspace: &Workspace,
        managed: impl Fn(&Package) -> bool,
    ) -> BTreeMap<PackageId, Vec<Through>> {
        let Some(paths) = &self.paths else {
            return BTreeMap::new();
        };
        let through = |reason: &RootReason| Through {
            file: paths.path(reason.file).to_owned(),
            name: reason.name.clone(),
        };
        let mut out: BTreeMap<PackageId, BTreeSet<Through>> = BTreeMap::new();
        for (origin, reasons) in &self.changes.members {
            for hit in oakum::plan::reach(workspace, [origin], &managed)
                .managed
                .into_keys()
            {
                out.entry(hit)
                    .or_default()
                    .extend(reasons.iter().map(through));
            }
        }
        if !self.changes.everywhere.is_empty() {
            for package in cargo_packages(workspace).filter(|package| managed(package)) {
                out.entry(package.id().clone())
                    .or_default()
                    .extend(self.changes.everywhere.iter().map(through));
            }
        }
        out.into_iter()
            .map(|(id, through)| (id, through.into_iter().collect()))
            .collect()
    }
}

/// Whether `changed` lists the workspace's root `Cargo.toml` or `Cargo.lock`,
/// asked before the reads that compare them.
pub(super) fn touched(workspace: &Workspace, changed: &[String]) -> bool {
    Paths::of(workspace).is_some_and(|paths| {
        changed
            .iter()
            .any(|path| *path == paths.manifest || *path == paths.lock)
    })
}

fn cargo_packages(workspace: &Workspace) -> impl Iterator<Item = &Package> {
    workspace
        .packages()
        .filter(|package| package.id().ecosystem == Ecosystem::Cargo)
}

/// `base` against `head` for the root files among `changed`.
///
/// # Errors
///
/// A read git could not answer, or a side that does not parse: `unverified`,
/// since what the file changed is then unknown.
pub(super) fn between_commits(
    git: &Git,
    workspace: &Workspace,
    base: &Commit,
    head: &Commit,
    changed: &[String],
) -> Result<RootReach, CliError> {
    compare(git, workspace, base, changed, |wanted| {
        git.blobs_or_absent(head, wanted)
    })
}

/// A commit against its first parent; a root commit's every root file is new.
///
/// # Errors
///
/// As [`between_commits`].
pub(super) fn for_commit(
    git: &Git,
    workspace: &Workspace,
    hash: &str,
    changed: &[String],
) -> Result<RootReach, CliError> {
    let commit = Commit::from_log(hash)?;
    between_commits(git, workspace, &commit.first_parent(), &commit, changed)
}

/// `base` against the working tree: a root file in `on_disk` is read by
/// `read_disk`, any other at `head`.
///
/// # Errors
///
/// As [`between_commits`], or `read_disk`'s own failure.
pub(super) fn against_worktree(
    git: &Git,
    workspace: &Workspace,
    base: &Commit,
    head: &Commit,
    changed: &[String],
    on_disk: &[String],
    read_disk: impl Fn(&str) -> Result<Option<String>, CliError>,
) -> Result<RootReach, CliError> {
    compare(git, workspace, base, changed, |wanted| {
        let committed = git.blobs_or_absent(head, wanted)?;
        wanted
            .iter()
            .zip(committed)
            .map(|(path, committed)| {
                if on_disk.iter().any(|listed| listed == path) {
                    read_disk(path)
                } else {
                    Ok(committed)
                }
            })
            .collect()
    })
}

/// `texts` holds one entry per `wanted` path; a root file not wanted reads as
/// absent on both sides, which compares as unchanged.
fn side<'a>(paths: &Paths, wanted: &[&str], texts: &'a [Option<String>]) -> RootFiles<'a> {
    let text = |path: &str| {
        wanted
            .iter()
            .position(|wanted| *wanted == path)
            .and_then(|at| texts[at].as_deref())
    };
    RootFiles {
        manifest: text(&paths.manifest),
        lock: text(&paths.lock),
    }
}

fn compare(
    git: &Git,
    workspace: &Workspace,
    base: &Commit,
    changed: &[String],
    head: impl FnOnce(&[&str]) -> Result<Vec<Option<String>>, CliError>,
) -> Result<RootReach, CliError> {
    let Some(paths) = Paths::of(workspace) else {
        return Ok(RootReach::default());
    };
    let wanted: Vec<&str> = [paths.manifest.as_str(), paths.lock.as_str()]
        .into_iter()
        .filter(|path| changed.iter().any(|changed| changed == path))
        .collect();
    if wanted.is_empty() {
        return Ok(RootReach::default());
    }
    let base_texts = git.blobs_or_absent(base, &wanted)?;
    let head_texts = head(&wanted)?;
    let found = root_changes(
        workspace,
        side(&paths, &wanted, &base_texts),
        side(&paths, &wanted, &head_texts),
    )
    .map_err(|err| {
        CliError::unverified(format!(
            "unverified: `{}` does not parse on one side of the comparison, so what it changed is unknown: {}",
            paths.path(err.file),
            err.message
        ))
    })?;
    Ok(RootReach {
        changes: found,
        paths: Some(paths),
    })
}
