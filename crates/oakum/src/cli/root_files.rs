//! Both sides of a workspace's root files, read for the comparisons in
//! ADR-0039: a Cargo workspace's `Cargo.toml` and `Cargo.lock` for
//! [`root_changes`], and a pnpm workspace's catalogs and `pnpm-lock.yaml`.

use std::collections::{BTreeMap, BTreeSet};

use oakum::manifest::{
    changed_catalog_entries, pnpm_lock_reach, root_changes, runtime_catalog_uses, RootChanges,
    RootFile, RootFiles, RootReason,
};
use oakum::plan::{Bounds, Ecosystem, Package, PackageId, ResolvesDependenciesAt, Workspace};
use oakum::state::Through;

use super::dependency_updates::Ask;
use super::git::{Commit, Followed, Git};
use super::CliError;

/// `manifest` and `lock` are where the root files lead now, which a diff
/// names; `in_place` is where they sit, which an earlier side is read
/// through, its own links followed there.
#[derive(Debug)]
struct Paths {
    manifest: String,
    lock: String,
    in_place: (String, String),
}

impl Paths {
    fn of(workspace: &Workspace) -> Option<Self> {
        let (manifest, lock) = workspace.cargo_root_files()?;
        let root = workspace.cargo_workspace_root()?;
        Some(Self {
            manifest,
            lock,
            in_place: (joined(root, "Cargo.toml"), joined(root, "Cargo.lock")),
        })
    }

    fn in_place<'a>(&'a self, path: &'a str) -> &'a str {
        if path == self.manifest {
            &self.in_place.0
        } else if path == self.lock {
            &self.in_place.1
        } else {
            path
        }
    }

    fn linked(&self, path: &str) -> bool {
        path != self.in_place(path)
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
    /// Each npm member a pnpm root file reached, with what reached it.
    pnpm: BTreeMap<PackageId, BTreeSet<Through>>,
    /// Counts only toward the packages that build it in ([`built_into`]).
    locked: BTreeMap<PackageId, BTreeSet<Through>>,
}

/// What a root change reached a member through, for ADR-0040's classification.
#[derive(Debug)]
pub(super) enum Reason<'a> {
    /// A `[workspace.dependencies]` or catalog entry, by its key.
    Entry(&'a str),
    /// A lockfile row.
    Locked,
    Other,
}

impl RootReach {
    /// The packages to walk [`oakum::plan::reach`] from, split by what their
    /// reasons ask: the members a root change named, the build-resolving
    /// packages a lock change reached, and for a workspace-wide one every
    /// Cargo package `managed` accepts, which always asks for intent.
    pub(super) fn changed(
        &self,
        workspace: &Workspace,
        managed: impl Fn(&Package) -> bool,
        ask: impl Fn(&PackageId, &Reason<'_>) -> Ask,
    ) -> BTreeMap<PackageId, Ask> {
        let mut out: BTreeMap<PackageId, Ask> = BTreeMap::new();
        let mut add = |id: PackageId, asked: Ask| {
            let at = out.entry(id).or_insert(Ask::Nothing);
            *at = (*at).max(asked);
        };
        let strongest = |id: &PackageId, reasons: &mut dyn Iterator<Item = Reason<'_>>| {
            reasons
                .map(|reason| ask(id, &reason))
                .max()
                .unwrap_or(Ask::Intent)
        };
        for (id, reasons) in &self.changes.members {
            add(
                id.clone(),
                strongest(id, &mut reasons.iter().map(cargo_reason)),
            );
            if reasons.iter().any(|reason| reason.file == RootFile::Lock) {
                let asked = strongest(id, &mut std::iter::once(Reason::Locked));
                for build in builds_above(workspace, id, &managed) {
                    add(build, asked);
                }
            }
        }
        for (id, reasons) in &self.pnpm {
            add(
                id.clone(),
                strongest(
                    id,
                    &mut reasons.iter().map(|through| Reason::Entry(&through.name)),
                ),
            );
        }
        for (origin, reasons) in &self.locked {
            let asked = strongest(origin, &mut reasons.iter().map(|_| Reason::Locked));
            for id in built_into(workspace, origin, &managed) {
                add(id, asked);
            }
        }
        if !self.changes.everywhere.is_empty() {
            for package in cargo_packages(workspace).filter(|package| managed(package)) {
                add(package.id().clone(), Ask::Intent);
            }
        }
        out
    }

    /// The members a root change named, each needing a verdict.
    pub(super) fn named(&self) -> impl Iterator<Item = PackageId> + '_ {
        self.changes
            .members
            .keys()
            .chain(self.pnpm.keys())
            .chain(self.locked.keys())
            .cloned()
    }

    /// Each managed package a root change reached, with every reason that
    /// reached it: a lock row through one internal crate and a manifest entry
    /// through another both land on the package that ships them.
    pub(super) fn through(
        &self,
        workspace: &Workspace,
        managed: impl Fn(&Package) -> bool,
    ) -> BTreeMap<PackageId, Vec<Through>> {
        let mut out: BTreeMap<PackageId, BTreeSet<Through>> = BTreeMap::new();
        for (origin, reasons) in &self.pnpm {
            for hit in oakum::plan::reach(workspace, [origin], &managed)
                .managed
                .into_keys()
            {
                out.entry(hit).or_default().extend(reasons.iter().cloned());
            }
        }
        for (origin, reasons) in &self.locked {
            for hit in built_into(workspace, origin, &managed) {
                out.entry(hit).or_default().extend(reasons.iter().cloned());
            }
        }
        let Some(paths) = &self.paths else {
            return out
                .into_iter()
                .map(|(id, through)| (id, through.into_iter().collect()))
                .collect();
        };
        let through = |reason: &RootReason| Through {
            file: paths.path(reason.file).to_owned(),
            name: reason.name.clone(),
        };
        for (origin, reasons) in &self.changes.members {
            for hit in oakum::plan::reach(workspace, [origin], &managed)
                .managed
                .into_keys()
            {
                out.entry(hit)
                    .or_default()
                    .extend(reasons.iter().map(through));
            }
            let locked = || {
                reasons
                    .iter()
                    .filter(|reason| reason.file == RootFile::Lock)
                    .map(through)
            };
            if locked().next().is_some() {
                for hit in builds_above(workspace, origin, &managed) {
                    out.entry(hit).or_default().extend(locked());
                }
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

fn cargo_reason(reason: &RootReason) -> Reason<'_> {
    match reason.file {
        RootFile::Lock => Reason::Locked,
        RootFile::Manifest => reason
            .name
            .strip_prefix("[workspace.dependencies] ")
            .map_or(Reason::Other, Reason::Entry),
    }
}

/// Managed build-resolving packages at or above `origin` along runtime edges,
/// past managed libraries too: a published library ships no lockfile, a build
/// bakes in what it resolved.
fn built_into(
    workspace: &Workspace,
    origin: &PackageId,
    managed: impl Fn(&Package) -> bool,
) -> BTreeSet<PackageId> {
    let mut seen = BTreeSet::from([origin.clone()]);
    let mut queue = std::collections::VecDeque::from([origin.clone()]);
    let mut out = BTreeSet::new();
    while let Some(at) = queue.pop_front() {
        if let Some(package) = workspace.get(&at) {
            if managed(package)
                && matches!(
                    package.resolves_dependencies_at(),
                    ResolvesDependenciesAt::Build(_)
                )
            {
                out.insert(at.clone());
            }
        }
        for (dependent, _) in workspace.runtime_dependents(&at) {
            if seen.insert(dependent.id().clone()) {
                queue.push_back(dependent.id().clone());
            }
        }
    }
    out
}

/// The builds [`built_into`] finds above a member a `Cargo.lock` row reached
/// that [`oakum::plan::reach`] from it does not, as past a managed library.
fn builds_above(
    workspace: &Workspace,
    member: &PackageId,
    managed: impl Fn(&Package) -> bool,
) -> BTreeSet<PackageId> {
    let reached = oakum::plan::reach(workspace, [member], &managed).managed;
    built_into(workspace, member, &managed)
        .into_iter()
        .filter(|id| !reached.contains_key(id))
        .collect()
}

/// Whether `changed` lists a root file a comparison reads, or a root file is a
/// link, which [`compare_cargo`] reads whatever a non-empty diff names; asked
/// before the reads that compare them.
pub(super) fn touched(workspace: &Workspace, changed: &[String]) -> bool {
    let listed = |wanted: &str| changed.iter().any(|path| path == wanted);
    Paths::of(workspace).is_some_and(|paths| {
        [&paths.manifest, &paths.lock]
            .into_iter()
            .any(|path| paths.linked(path) || listed(path) || listed(paths.in_place(path)))
    }) || workspace.catalog_file().is_some_and(listed)
        || pnpm_lock_path(workspace).is_some_and(|path| listed(&path))
}

pub(super) fn pnpm_lock_path(workspace: &Workspace) -> Option<String> {
    workspace
        .pnpm_lock_dir()
        .map(|dir| joined(dir, "pnpm-lock.yaml"))
}

fn joined(dir: &str, path: &str) -> String {
    match (dir, path) {
        (dir, "." | "") => dir.to_owned(),
        ("", path) => path.to_owned(),
        (dir, path) => format!("{dir}/{path}"),
    }
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
    compare(
        git,
        workspace,
        base,
        changed,
        &Head::Commit(head),
        |wanted| git.blobs_or_absent(head, wanted),
    )
}

/// `base` against the working tree: a root file in `on_disk` is read by
/// `read_disk`, a linked one as [`root_files_read`] says, any other at `head`.
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
    let side = Head::Worktree {
        commit: head,
        on_disk,
        read_disk: &read_disk,
    };
    compare(git, workspace, base, changed, &side, |wanted| {
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

/// The later side of a comparison, for reading the Cargo root files through
/// that side's own links.
enum Head<'a> {
    Commit(&'a Commit),
    /// A file in `on_disk` is read by `read_disk`, a linked root file as
    /// [`root_files_read`] says, any other at `commit`.
    Worktree {
        commit: &'a Commit,
        on_disk: &'a [String],
        read_disk: &'a dyn Fn(&str) -> Result<Option<String>, CliError>,
    },
}

fn compare(
    git: &Git,
    workspace: &Workspace,
    base: &Commit,
    changed: &[String],
    head_side: &Head<'_>,
    head: impl Fn(&[&str]) -> Result<Vec<Option<String>>, CliError>,
) -> Result<RootReach, CliError> {
    let mut reach = compare_cargo(git, workspace, base, changed, head_side)?;
    reach.pnpm = catalog_reach(git, workspace, base, changed, &head)?;
    reach.locked = lock_reach(git, workspace, base, changed, &head)?;
    Ok(reach)
}

fn lock_reach(
    git: &Git,
    workspace: &Workspace,
    base: &Commit,
    changed: &[String],
    head: impl Fn(&[&str]) -> Result<Vec<Option<String>>, CliError>,
) -> Result<BTreeMap<PackageId, BTreeSet<Through>>, CliError> {
    let (Some(dir), Some(path)) = (workspace.pnpm_lock_dir(), pnpm_lock_path(workspace)) else {
        return Ok(BTreeMap::new());
    };
    if !changed.contains(&path) {
        return Ok(BTreeMap::new());
    }
    let base_text = git.blobs_or_absent(base, &[&path])?.pop().flatten();
    let head_text = head(&[&path])?.pop().flatten();
    let reached = pnpm_lock_reach(base_text.as_deref(), head_text.as_deref()).map_err(
        |(side, err)| {
            CliError::unverified(format!(
                "unverified: `{path}` does not parse at the {side} of the comparison, so which entries it changed is unknown: {err}"
            ))
        },
    )?;
    let mut out = BTreeMap::new();
    for (importer, names) in reached {
        let project = joined(dir, &importer);
        let Some(package) = workspace.packages().find(|package| {
            package.id().ecosystem == Ecosystem::Npm && package.manifest_dir() == project
        }) else {
            continue;
        };
        out.insert(
            package.id().clone(),
            names
                .into_iter()
                .map(|name| Through {
                    file: path.clone(),
                    name,
                })
                .collect(),
        );
    }
    Ok(out)
}

/// The npm members whose runtime dependencies take a catalog entry that moved.
/// An entry that installs one workspace package on both sides, by its key or
/// an `npm:` alias, and whose new range is its old one retargeted at that
/// package's version, is the pin `version` writes, so it reaches nothing.
fn catalog_reach(
    git: &Git,
    workspace: &Workspace,
    base: &Commit,
    changed: &[String],
    head: impl Fn(&[&str]) -> Result<Vec<Option<String>>, CliError>,
) -> Result<BTreeMap<PackageId, BTreeSet<Through>>, CliError> {
    let Some(path) = workspace
        .catalog_file()
        .filter(|path| changed.iter().any(|changed| changed == path))
    else {
        return Ok(BTreeMap::new());
    };
    let base_text = git.blobs_or_absent(base, &[path])?.pop().flatten();
    let members: Vec<&Package> = workspace
        .packages()
        .filter(|package| package.id().ecosystem == Ecosystem::Npm)
        .collect();
    let manifests: Vec<String> = members
        .iter()
        .map(|package| package.manifest_path())
        .collect();
    let wanted: Vec<&str> = std::iter::once(path)
        .chain(manifests.iter().map(String::as_str))
        .collect();
    let texts = head(&wanted)?;
    let (head_text, manifest_texts) = texts.split_first().unwrap_or((&None, &[]));
    let mut moved = changed_catalog_entries(base_text.as_deref(), head_text.as_deref())
        .map_err(|(side, err)| {
            CliError::unverified(format!(
                "unverified: `{path}` does not parse at the {side} of the comparison, so which catalog entries it changed is unknown: {err}"
            ))
        })?;
    moved.retain(|_, repinned| {
        !repinned.as_ref().is_some_and(|pin| {
            workspace
                .get(&PackageId::new(Ecosystem::Npm, pin.package.clone()))
                .is_some_and(|member| {
                    Bounds::from_npm_text(&pin.from).is_ok_and(|bounds| {
                        bounds.retargeted(member.version()).as_deref() == Some(pin.to.as_str())
                    })
                })
        })
    });
    let mut out: BTreeMap<PackageId, BTreeSet<Through>> = BTreeMap::new();
    if moved.is_empty() {
        return Ok(out);
    }
    for ((package, manifest), text) in members.iter().zip(&manifests).zip(manifest_texts) {
        let Some(text) = text else {
            continue;
        };
        let uses = runtime_catalog_uses(text).map_err(|err| {
            CliError::unverified(format!(
                "unverified: `{manifest}` does not parse, so which catalog entries it takes is unknown: {err}"
            ))
        })?;
        let reasons: BTreeSet<Through> = uses
            .iter()
            .filter(|entry| moved.contains_key(*entry))
            .map(|(_, name)| Through {
                file: path.to_owned(),
                name: name.clone(),
            })
            .collect();
        if !reasons.is_empty() {
            out.insert(package.id().clone(), reasons);
        }
    }
    Ok(out)
}

fn compare_cargo(
    git: &Git,
    workspace: &Workspace,
    base: &Commit,
    changed: &[String],
    head: &Head<'_>,
) -> Result<RootReach, CliError> {
    // An empty list is a merge's: read through its first parent, a link would
    // credit it with the merged branch's lock change.
    let Some(paths) = Paths::of(workspace).filter(|_| !changed.is_empty()) else {
        return Ok(RootReach::default());
    };
    let listed = |path: &str| {
        changed
            .iter()
            .any(|changed| changed == path || changed == paths.in_place(path))
    };
    // A link is read on both sides whatever the diff names: a link along its
    // chain retargeted, or a file it led to then, changes what cargo read,
    // and neither is a path known here. One that reads the same changed nothing.
    let wanted: Vec<&str> = [paths.manifest.as_str(), paths.lock.as_str()]
        .into_iter()
        .filter(|path| paths.linked(path) || listed(path))
        .collect();
    if wanted.is_empty() {
        return Ok(RootReach::default());
    }
    let (base_reads, head_reads) = root_files_read(git, base, head, &paths, &wanted)?;
    let mut kept = Vec::new();
    let (mut base_texts, mut head_texts) = (Vec::new(), Vec::new());
    for ((path, base), head) in wanted.into_iter().zip(base_reads).zip(head_reads) {
        let named = listed(path);
        // Unnamed and unfollowable on both sides, a link is left unread, as an
        // unnamed root file in place is.
        if !named && base.is_err() && head.is_err() {
            continue;
        }
        let (base, head) = (base?, head?);
        if named || base != head {
            kept.push(path);
            base_texts.push(base);
            head_texts.push(head);
        }
    }
    let wanted = kept;
    if wanted.is_empty() {
        return Ok(RootReach::default());
    }
    // The manifest comparison reads these only to skip `version`'s own
    // retarget, so an unread base there skips nothing.
    let base_members = if wanted.contains(&paths.lock.as_str()) {
        declared_at(git, workspace, base)?
    } else {
        declared_at(git, workspace, base).unwrap_or_default()
    };
    let found = root_changes(
        workspace,
        &base_members,
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
        ..RootReach::default()
    })
}

/// One side's text of a root file, or why that side's link could not be
/// followed, which is `unverified`.
type SideText = Result<Option<String>, CliError>;

/// The root files each side holds, read where they sit and through that
/// side's own links: today's link would otherwise send an earlier commit to a
/// file it never read. Both commits are read by one child; the outer error is
/// a read that did not happen.
fn root_files_read(
    git: &Git,
    base: &Commit,
    head: &Head<'_>,
    paths: &Paths,
    wanted: &[&str],
) -> Result<(Vec<SideText>, Vec<SideText>), CliError> {
    let head_commit = match head {
        Head::Commit(commit) | Head::Worktree { commit, .. } => *commit,
    };
    // Followed always: whether a side holds a link is its own, so a link
    // removed since still leads somewhere at the base.
    let queries: Vec<(&Commit, &str)> = [base, head_commit]
        .into_iter()
        .flat_map(|commit| {
            wanted
                .iter()
                .map(move |path| (commit, paths.in_place(path)))
        })
        .collect();
    let mut texts: Vec<SideText> = queries
        .iter()
        .zip(git.followed_blobs(&queries)?)
        .map(|((commit, path), followed)| followed_text(commit, path, followed))
        .collect();
    let mut head_texts = texts.split_off(wanted.len());
    if let Head::Worktree {
        on_disk, read_disk, ..
    } = head
    {
        for (path, text) in wanted.iter().zip(&mut head_texts) {
            let in_place = paths.in_place(path);
            // A link git follows at `HEAD` is read where it leads on disk,
            // since a link along its chain may have changed there.
            if (paths.linked(path) && text.is_ok())
                || on_disk
                    .iter()
                    .any(|listed| listed == path || listed == in_place)
            {
                *text = Ok(read_disk(in_place)?);
            }
        }
    }
    Ok((texts, head_texts))
}

fn followed_text(commit: &Commit, path: &str, followed: Followed) -> SideText {
    match followed {
        Followed::File(bytes) => String::from_utf8(bytes).map(Some).map_err(|_| {
            CliError::unverified(format!(
                "unverified: `{path}` at {} is not UTF-8, so what it changed is unknown",
                commit.as_str()
            ))
        }),
        Followed::Absent => Ok(None),
        Followed::Dangling => Err(CliError::unverified(format!(
            "unverified: `{path}` at {} is a link to no file there, so what it changed is unknown",
            commit.as_str()
        ))),
        Followed::Unfollowed(why) => Err(CliError::unverified(format!(
            "unverified: `{path}` at {} could not be read as a file ({why}), so what it changed is unknown",
            commit.as_str()
        ))),
    }
}

/// Each Cargo member as cargo reads it at `base`, run from the directory
/// HEAD's discovery ran from; none when the base holds no Cargo workspace.
pub(super) fn declared_at(
    git: &Git,
    workspace: &Workspace,
    base: &Commit,
) -> Result<BTreeMap<PackageId, Package>, CliError> {
    let root = git.root();
    let start = super::workspace::cargo_start(root);
    let members: Vec<(&str, &str)> = cargo_packages(workspace)
        .map(|package| (package.manifest_dir(), package.id().name.as_str()))
        .collect();
    Ok(
        super::base_tree::base_workspace(git, base, &start, root, &members)?
            .map(|workspace| {
                cargo_packages(&workspace)
                    .map(|package| (package.id().clone(), package.clone()))
                    .collect()
            })
            .unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_root_importer_is_the_lock_directory() {
        assert_eq!(joined("", "."), "");
        assert_eq!(joined("js", "."), "js");
        assert_eq!(joined("", "packages/a"), "packages/a");
        assert_eq!(joined("js", "packages/a"), "js/packages/a");
    }
}
