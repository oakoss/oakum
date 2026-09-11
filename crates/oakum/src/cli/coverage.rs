use std::collections::{BTreeMap, BTreeSet};

use oakum::commits::packages_for_paths;
use oakum::plan::{BumpFile, Package, PackageId, Workspace};
use oakum::state::Coverage;

use super::generate::resolve_from_ref;
use super::git::{Git, Op};
use super::CliError;

/// What the config says about a changed package. `Excluded` is a decision
/// someone wrote in `include`/`exclude`; `Unmanaged` is the absence of one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Standing {
    Excluded,
    Unmanaged,
    Managed,
}

/// Changed packages the selection keeps, split by whether the config manages
/// them. `unmanaged` never reaches coverage: a package the config cannot plan
/// cannot be covered by intent either, so the two are separate reports. Both
/// are sorted and disjoint.
pub(super) fn changed_by_standing(
    git: &Git,
    workspace: &Workspace,
    files: &[BumpFile],
    from: Option<&str>,
    standing: impl Fn(&Package) -> Standing,
) -> Result<Coverage, CliError> {
    // Classified once per package and read from the map thereafter: asking
    // twice would let an inconsistent answer report an excluded package as
    // unmanaged, which accuses someone of an omission they did not make.
    let classified: BTreeMap<PackageId, Standing> = workspace
        .packages()
        .map(|package| (package.id().clone(), standing(package)))
        .collect();
    let kept = changed_packages(git, workspace, from, &|package: &Package| {
        classified.get(package.id()) != Some(&Standing::Excluded)
    })?;
    let (managed, unmanaged): (Vec<PackageId>, Vec<PackageId>) = kept
        .into_iter()
        .partition(|id| classified.get(id) == Some(&Standing::Managed));
    let managed: BTreeSet<PackageId> = managed.into_iter().collect();
    let covered = covered_packages(files, &managed);
    Ok(Coverage {
        uncovered: managed
            .into_iter()
            .filter(|id| !covered.contains(id))
            .collect(),
        unmanaged,
    })
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
    managed: &impl Fn(&Package) -> bool,
) -> Result<BTreeSet<PackageId>, CliError> {
    // At depth 1 the default base resolves to HEAD itself, so the diff comes
    // back empty and every changed package looks covered. `actions/checkout`
    // clones that way by default, which makes this the common CI shape rather
    // than an edge one.
    if super::tags::is_shallow(git)? {
        return Err(CliError::unverified(
            "unverified: shallow clone; changed files cannot be listed against a base that was not fetched — use `fetch-depth: 0`, or `git fetch --unshallow`",
        ));
    }
    let base = resolve_from_ref(git, from).map_err(CliError::from_boxed)?;
    let paths = diff_paths(git, &base)?
        .into_iter()
        .filter(|path| !is_intent_path(path))
        .collect::<Vec<_>>();
    // Attribute on the full workspace so nested unmanaged packages keep
    // longest-prefix ownership; then drop unmanaged ids.
    let dirs: Vec<(PackageId, String)> = workspace
        .packages()
        .map(|package| (package.id().clone(), package.manifest_dir().to_owned()))
        .collect();
    Ok(packages_for_paths(&paths, &dirs)
        .into_iter()
        .filter(|id| workspace.get(id).is_some_and(managed))
        .collect())
}

fn diff_paths(git: &Git, from: &str) -> Result<Vec<String>, CliError> {
    git.paths(Op::ChangedPaths { from })
}

fn is_intent_path(path: &str) -> bool {
    let path = path.trim_start_matches("./");
    path == ".changeset" || path.starts_with(".changeset/")
}
