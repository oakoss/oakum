//! Which of a change's origins are dependency updates (ADR-0040): one verdict
//! per member, read from both sides, and each changed path or root reason
//! classified by it.

use std::collections::{BTreeMap, BTreeSet};

use oakum::commits::changed_packages;
use oakum::manifest::{
    changed_outside_dependencies, npm_declaration_changes, npm_member_keys, parses_equal, NpmSide,
};
use oakum::plan::{declaration_changes, Change, Ecosystem, Outside, Package, PackageId, Workspace};

use super::git::{Commit, Followed, Git};
use super::root_files::{self, Reason, RootReach};
use super::CliError;

/// What one origin asks of the packages it reaches, ordered so the strongest
/// of a package's origins wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Ask {
    Nothing,
    Update,
    Intent,
}

#[derive(Debug)]
struct Verdict {
    change: Change,
    /// Both sides of its own manifest parse to the same thing.
    untouched: bool,
    /// Keys of the entries whose declaration moved.
    moved: BTreeSet<String>,
    /// Keys whose entry names a member on either side.
    member_keys: BTreeSet<String>,
}

#[derive(Debug, Default)]
pub(super) struct Verdicts(BTreeMap<PackageId, Verdict>);

impl Verdicts {
    /// A missing verdict asks for intent: a package nothing compared is
    /// never taken to have moved only dependencies.
    fn own_manifest(&self, id: &PackageId) -> Ask {
        let Some(verdict) = self.0.get(id) else {
            return Ask::Intent;
        };
        // A comment or formatting edit keeps asking as any other edit does;
        // entries that resolve the same but are written differently ask nothing.
        match verdict.change {
            Change::NeedsIntent => Ask::Intent,
            Change::Unchanged if verdict.untouched => Ask::Intent,
            Change::DependencyUpdate => Ask::Update,
            Change::DevelopmentOnly | Change::Unchanged => Ask::Nothing,
        }
    }

    pub(super) fn reason(&self, id: &PackageId, reason: &Reason<'_>) -> Ask {
        let Some(verdict) = self.0.get(id) else {
            return Ask::Intent;
        };
        match reason {
            Reason::Entry(key) if verdict.member_keys.contains(*key) => Ask::Intent,
            Reason::Entry(key) if verdict.moved.contains(*key) => Ask::Update,
            Reason::Entry(_) if verdict.change == Change::NeedsIntent => Ask::Intent,
            Reason::Entry(_) => Ask::Nothing,
            // Rows a declaration added or removed come with that manifest
            // edit, which asks for intent on its own.
            Reason::Locked => Ask::Update,
            Reason::Other => Ask::Intent,
        }
    }
}

/// The root files a lockfile or catalog update reaches packages through by
/// their entries, never as files in the directory of a package holding them.
fn read_by_entries(workspace: &Workspace) -> BTreeSet<String> {
    let mut files = BTreeSet::new();
    if let Some((_, lock)) = workspace.cargo_root_files() {
        files.insert(lock);
    }
    files.extend(root_files::pnpm_lock_path(workspace));
    files.extend(workspace.catalog_file().map(str::to_owned));
    files
}

/// Each package owning a changed path, with what its paths ask: any path but
/// its own manifest asks for intent, the manifest what its verdict says.
pub(super) fn owned(
    workspace: &Workspace,
    paths: &[String],
    verdicts: &Verdicts,
) -> BTreeMap<PackageId, Ask> {
    let mut asks = BTreeMap::new();
    for (id, only_manifest) in owners(workspace, paths) {
        let ask = if only_manifest {
            verdicts.own_manifest(&id)
        } else {
            Ask::Intent
        };
        asks.insert(id, ask);
    }
    asks
}

/// Each owner of a changed path, and whether its manifest is all it changed.
fn owners(workspace: &Workspace, paths: &[String]) -> BTreeMap<PackageId, bool> {
    let skipped = read_by_entries(workspace);
    let mut out: BTreeMap<PackageId, bool> = BTreeMap::new();
    for path in paths.iter().filter(|path| !skipped.contains(*path)) {
        for id in changed_packages(std::slice::from_ref(path), workspace) {
            let is_manifest = workspace
                .get(&id)
                .is_some_and(|package| package.manifest_path() == *path);
            let only = out.entry(id).or_insert(true);
            *only = *only && is_manifest;
        }
    }
    out
}

/// Reads one side's text of each path, `None` where absent.
pub(super) type Read<'a> = dyn Fn(&[&str]) -> Result<Vec<Option<String>>, CliError> + 'a;

/// Each path's text at `commit`, read through its links, as [`Read`] reads it.
///
/// # Errors
///
/// Unverified when the read failed, or a path is a link git cannot follow
/// there or is not UTF-8: what that side held is then unknown.
pub(super) fn followed(
    git: &Git,
    commit: &Commit,
    paths: &[&str],
) -> Result<Vec<Option<String>>, CliError> {
    let queries: Vec<(&Commit, &str)> = paths.iter().map(|path| (commit, *path)).collect();
    git.followed_blobs(&queries)?
        .into_iter()
        .zip(paths)
        .map(|(followed, path)| {
            let why = match followed {
                Followed::File(bytes) => match String::from_utf8(bytes) {
                    Ok(text) => return Ok(Some(text)),
                    Err(_) => "it is not UTF-8",
                },
                Followed::Absent => return Ok(None),
                Followed::Dangling => "it is a link to no file there",
                Followed::Unfollowed(why) => why,
            };
            Err(unreadable(path, commit.as_str(), why))
        })
        .collect()
}

fn unreadable(path: &str, side: &str, why: &str) -> CliError {
    CliError::unverified(format!(
        "unverified: `{path}` at {side} cannot be read ({why}), so whether it moved only dependencies is unknown"
    ))
}

/// The side the head texts were read from, for its Cargo packages.
#[derive(Clone, Copy)]
pub(super) enum HeadPackages<'a> {
    Worktree,
    Commit(&'a Commit),
}

/// A verdict for each package whose manifest is the only path it changed,
/// and for each a root reason names. Cargo's sides are read by one cargo run
/// each, and only when a package's manifest did not already change outside
/// its dependency tables. A package absent on one side asks for intent.
///
/// # Errors
///
/// Unverified when a side cannot be read or parsed, or cargo cannot read it
/// (ADR-0040): what it changed is then unknown.
pub(super) fn verdicts(
    git: &Git,
    workspace: &Workspace,
    (base, head_packages): (&Commit, HeadPackages<'_>),
    paths: &[String],
    root: &RootReach,
    head: &Read<'_>,
) -> Result<Verdicts, CliError> {
    let manifest_changed: BTreeSet<PackageId> = owners(workspace, paths)
        .into_iter()
        .filter_map(|(id, only)| only.then_some(id))
        .collect();
    let wanted: BTreeSet<PackageId> = manifest_changed
        .iter()
        .cloned()
        .chain(root.named())
        .collect();
    if wanted.is_empty() {
        return Ok(Verdicts::default());
    }
    let packages: Vec<&Package> = wanted.iter().filter_map(|id| workspace.get(id)).collect();
    let mut files: Vec<String> = packages
        .iter()
        .map(|package| package.manifest_path())
        .collect();
    let catalog = workspace.catalog_file();
    files.extend(catalog.map(str::to_owned));
    let wanted_paths: Vec<&str> = files.iter().map(String::as_str).collect();
    let base_texts = followed(git, base, &wanted_paths)?;
    let head_texts = head(&wanted_paths)?;
    let text = |texts: &[Option<String>], path: &str| {
        wanted_paths
            .iter()
            .position(|wanted| *wanted == path)
            .and_then(|at| texts[at].clone())
    };
    let catalog_sides = catalog.map(|path| (text(&base_texts, path), text(&head_texts, path)));

    let mut out = Verdicts::default();
    let mut cargo_pending = Vec::new();
    for package in packages {
        let names = member_names(workspace, package);
        let path = package.manifest_path();
        let sides = (text(&base_texts, &path), text(&head_texts, &path));
        let (Some(before), Some(after)) = sides else {
            out.0.insert(package.id().clone(), intent());
            continue;
        };
        let own = manifest_changed.contains(package.id());
        let not_compared = |why: String| {
            CliError::unverified(format!(
                "unverified: `{path}` cannot be compared ({why}), so whether it moved only dependencies is unknown"
            ))
        };
        let (outside, untouched) =
            compare_text(package, &before, &after, own, &names).map_err(not_compared)?;
        if outside == Outside::Changed {
            out.0.insert(package.id().clone(), intent());
        } else if package.id().ecosystem == Ecosystem::Npm {
            let (base_yaml, head_yaml) = catalog_sides.clone().unwrap_or_default();
            let sides = (
                NpmSide {
                    manifest: &before,
                    workspace_file: base_yaml.as_deref(),
                },
                NpmSide {
                    manifest: &after,
                    workspace_file: head_yaml.as_deref(),
                },
            );
            let verdict = npm_verdict(sides, outside, untouched, &names).map_err(not_compared)?;
            out.0.insert(package.id().clone(), verdict);
        } else {
            cargo_pending.push((package, outside, untouched));
        }
    }
    if cargo_pending.is_empty() {
        return Ok(out);
    }
    let at_base = root_files::declared_at(git, workspace, base)?;
    let at_head = match head_packages {
        HeadPackages::Commit(commit)
            if !worktree_matches(git, workspace, commit, &cargo_pending)? =>
        {
            Some(root_files::declared_at(git, workspace, commit)?)
        }
        _ => None,
    };
    for (package, outside, untouched) in cargo_pending {
        let now = at_head
            .as_ref()
            .map_or(Some(package), |at| at.get(package.id()));
        let names = member_names(workspace, package);
        let verdict = match (at_base.get(package.id()), now) {
            (Some(was), Some(now)) => cargo_verdict(was, now, outside, untouched, &names),
            _ => intent(),
        };
        out.0.insert(package.id().clone(), verdict);
    }
    Ok(out)
}

/// Whether the manifests the pending packages' declarations come from, their
/// own and the workspace root's, read the same on disk as at `commit`: then
/// discovery's packages are that commit's, and cargo need not run again.
fn worktree_matches(
    git: &Git,
    workspace: &Workspace,
    commit: &Commit,
    pending: &[(&Package, Outside, bool)],
) -> Result<bool, CliError> {
    let mut paths: Vec<String> = pending
        .iter()
        .map(|(package, ..)| package.manifest_path())
        .collect();
    paths.extend(workspace.cargo_root_files().map(|(manifest, _)| manifest));
    // Where it sits, too: a link retargeted on disk leads elsewhere than at
    // `commit` though the file it leads to now is unchanged.
    paths.extend(workspace.cargo_workspace_root().map(|root| match root {
        "" => String::from("Cargo.toml"),
        root => format!("{root}/Cargo.toml"),
    }));
    let wanted: Vec<&str> = paths.iter().map(String::as_str).collect();
    let committed = followed(git, commit, &wanted)?;
    Ok(wanted
        .iter()
        .zip(committed)
        .all(|(path, committed)| std::fs::read_to_string(git.root().join(path)).ok() == committed))
}

/// Where the package's own manifest did not change, nothing outside it did.
fn compare_text(
    package: &Package,
    before: &str,
    after: &str,
    own: bool,
    names: &BTreeSet<String>,
) -> Result<(Outside, bool), String> {
    let ecosystem = package.id().ecosystem;
    let outside = if own {
        changed_outside_dependencies(ecosystem, before, after, names)?
    } else {
        Outside::Same
    };
    Ok((outside, parses_equal(ecosystem, before, after)?))
}

fn npm_verdict(
    (base, head): (NpmSide<'_>, NpmSide<'_>),
    outside: Outside,
    untouched: bool,
    names: &BTreeSet<String>,
) -> Result<Verdict, String> {
    let changes = npm_declaration_changes(base, head, names)?;
    let mut member_keys = npm_member_keys(base, names)?;
    member_keys.extend(npm_member_keys(head, names)?);
    Ok(Verdict {
        change: changes.change(outside),
        untouched,
        moved: changes
            .moved()
            .iter()
            .map(|moved| moved.declared_as.clone())
            .collect(),
        member_keys,
    })
}

fn cargo_verdict(
    was: &Package,
    package: &Package,
    outside: Outside,
    untouched: bool,
    names: &BTreeSet<String>,
) -> Verdict {
    let changes = declaration_changes(
        was.external_dependencies(),
        package.external_dependencies(),
        names,
    );
    let member_keys = [was, package]
        .into_iter()
        .flat_map(|side| {
            let edges = side
                .dependencies()
                .iter()
                .map(|edge| edge.declared_as.clone());
            let patched = side
                .external_dependencies()
                .iter()
                .filter(|dependency| names.contains(&dependency.name))
                .map(|dependency| dependency.declared_as.clone());
            edges.chain(patched).collect::<Vec<_>>()
        })
        .collect();
    Verdict {
        change: changes.change(outside),
        untouched,
        moved: changes
            .moved()
            .iter()
            .map(|moved| moved.after.declared_as.clone())
            .collect(),
        member_keys,
    }
}

fn intent() -> Verdict {
    Verdict {
        change: Change::NeedsIntent,
        untouched: false,
        moved: BTreeSet::new(),
        member_keys: BTreeSet::new(),
    }
}

/// Every key `package` declares a member under: a member's name, or a rename
/// or alias one of its own edges carries, which elsewhere may name another
/// package.
fn member_names(workspace: &Workspace, package: &Package) -> BTreeSet<String> {
    workspace
        .packages()
        .map(|member| member.id().name.clone())
        .chain(
            package
                .dependencies()
                .iter()
                .map(|edge| edge.declared_as.clone()),
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::Ask;

    #[test]
    fn the_strongest_ask_wins() {
        assert_eq!(Ask::Nothing.max(Ask::Update), Ask::Update);
        assert_eq!(Ask::Update.max(Ask::Intent), Ask::Intent);
    }
}
