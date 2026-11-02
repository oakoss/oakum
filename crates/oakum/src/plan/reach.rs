//! What a change reaches ([ADR-0039]): a change to a package oakum does not
//! manage also reaches the managed packages that ship it, along the runtime
//! edges a release cascades along ([ADR-0008]).
//!
//! [ADR-0039]: ../../../../docs/decisions/0039-derive-what-a-change-reaches-and-what-covers-it.md
//! [ADR-0008]: ../../../../docs/decisions/0008-cascade-only-along-runtime-edges.md

use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::vec::Vec;

use super::workspace::{Package, PackageId, Workspace};

/// Where a set of changed packages lands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reach {
    /// Each managed package reached, with the non-managed packages a change
    /// passed through to reach it, nearest to it first: `app` reached from
    /// `store` through `core` holds `[core, store]`. Empty for a direct change.
    pub managed: BTreeMap<PackageId, Vec<PackageId>>,
    /// Changed packages that are not managed and that no managed package
    /// depends on at runtime.
    pub stranded: BTreeSet<PackageId>,
}

/// Walks up from each changed package along runtime dependents, through
/// packages `managed` rejects, stopping at the first one it accepts. A direct
/// change beats a path; among paths the shortest wins, then the lowest read
/// nearest first.
#[must_use]
pub fn reach<'a>(
    workspace: &Workspace,
    changed: impl IntoIterator<Item = &'a PackageId>,
    managed: impl Fn(&Package) -> bool,
) -> Reach {
    let mut out = Reach::default();
    for id in changed {
        let Some(package) = workspace.get(id) else {
            continue;
        };
        if managed(package) {
            out.managed.insert(id.clone(), Vec::new());
            continue;
        }
        let mut depth = BTreeMap::from([(id.clone(), 0_usize)]);
        let mut hits: BTreeMap<PackageId, usize> = BTreeMap::new();
        let mut queue = VecDeque::from([id.clone()]);
        while let Some(at) = queue.pop_front() {
            let next_depth = depth[&at] + 1;
            for (dependent, _) in workspace.runtime_dependents(&at) {
                let next = dependent.id();
                if managed(dependent) {
                    hits.entry(next.clone()).or_insert(next_depth);
                } else if !depth.contains_key(next) {
                    depth.insert(next.clone(), next_depth);
                    queue.push_back(next.clone());
                }
            }
        }
        if hits.is_empty() {
            out.stranded.insert(id.clone());
        }
        for (hit, length) in hits {
            let path = lowest_path(workspace, &hit, length, &depth);
            keep_best(&mut out.managed, hit, path);
        }
    }
    out
}

/// The lowest of the shortest paths down from `hit`, nearest first: at each
/// step the lowest-named dependency one step closer to the change. Every
/// package at depth `d` has one at `d - 1` below it, so the walk always lands
/// on the change.
fn lowest_path(
    workspace: &Workspace,
    hit: &PackageId,
    length: usize,
    depth: &BTreeMap<PackageId, usize>,
) -> Vec<PackageId> {
    let mut path = Vec::with_capacity(length);
    let mut at = hit.clone();
    for step in (0..length).rev() {
        let Some(below) = workspace.get(&at).and_then(|package| {
            package
                .runtime_dependencies()
                .map(|dependency| &dependency.on)
                .filter(|on| depth.get(*on) == Some(&step))
                .min()
                .cloned()
        }) else {
            break;
        };
        path.push(below.clone());
        at = below;
    }
    path
}

fn keep_best(
    managed: &mut BTreeMap<PackageId, Vec<PackageId>>,
    id: PackageId,
    path: Vec<PackageId>,
) {
    match managed.get(&id) {
        Some(held) if (held.len(), held) <= (path.len(), &path) => {}
        _ => {
            managed.insert(id, path);
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use semver::Version;

    use super::reach;
    use crate::plan::workspace::{
        DeclaredRange, Dependency, DependencyKind, Ecosystem, Package, PackageId,
        ResolvesDependenciesAt, Workspace,
    };
    use crate::plan::Bounds;

    fn id(name: &str) -> PackageId {
        PackageId::new(Ecosystem::Cargo, name)
    }

    fn on(name: &str, kind: DependencyKind) -> Dependency {
        Dependency {
            on: id(name),
            kind,
            declared_as: name.into(),
            target: None,
            range: DeclaredRange::Plain(Bounds::from_cargo_text("^0.1.0").expect("range")),
        }
    }

    fn package(name: &str, dependencies: Vec<Dependency>) -> Package {
        Package::new(
            id(name),
            Version::new(0, 1, 0),
            ResolvesDependenciesAt::Install,
            true,
            dependencies,
        )
    }

    fn workspace(packages: Vec<Package>) -> Workspace {
        Workspace::new(packages).expect("workspace")
    }

    fn managed_set<'a>(names: &'a [&'a str]) -> impl Fn(&Package) -> bool + 'a {
        move |package| names.contains(&package.id().name.as_str())
    }

    #[test]
    fn a_normal_or_build_edge_reaches_the_shipping_package() {
        for kind in [DependencyKind::Normal, DependencyKind::Build] {
            let ws = workspace(vec![
                package("app", vec![on("store", kind)]),
                package("store", vec![]),
            ]);
            let got = reach(&ws, [&id("store")], managed_set(&["app"]));
            assert_eq!(
                got.managed.get(&id("app")),
                Some(&vec![id("store")]),
                "{kind:?}"
            );
            assert!(got.stranded.is_empty(), "{kind:?}");
        }
    }

    #[test]
    fn a_dev_edge_reaches_nothing() {
        let ws = workspace(vec![
            package("app", vec![on("store", DependencyKind::Development)]),
            package("store", vec![]),
        ]);
        let got = reach(&ws, [&id("store")], managed_set(&["app"]));
        assert!(got.managed.is_empty());
        assert_eq!(got.stranded.into_iter().collect::<Vec<_>>(), [id("store")]);
    }

    #[test]
    fn the_walk_passes_two_unmanaged_packages_and_stops_at_a_managed_one() {
        let ws = workspace(vec![
            package("cli", vec![on("app", DependencyKind::Normal)]),
            package("app", vec![on("core", DependencyKind::Normal)]),
            package("core", vec![on("store", DependencyKind::Normal)]),
            package("store", vec![]),
        ]);
        let got = reach(&ws, [&id("store")], managed_set(&["app", "cli"]));
        assert_eq!(
            got.managed.get(&id("app")),
            Some(&vec![id("core"), id("store")])
        );
        assert!(!got.managed.contains_key(&id("cli")), "{got:?}");
    }

    #[test]
    fn a_direct_change_beats_a_path_and_the_shortest_path_wins() {
        let ws = workspace(vec![
            package(
                "app",
                vec![
                    on("a-long", DependencyKind::Normal),
                    on("b-short", DependencyKind::Normal),
                ],
            ),
            package("a-long", vec![on("store", DependencyKind::Normal)]),
            package("b-short", vec![on("store", DependencyKind::Normal)]),
            package("store", vec![]),
        ]);
        let managed = managed_set(&["app"]);
        let got = reach(&ws, [&id("store")], &managed);
        assert_eq!(
            got.managed.get(&id("app")),
            Some(&vec![id("a-long"), id("store")]),
            "equal lengths fall to the lower name"
        );
        let got = reach(&ws, [&id("store"), &id("b-short")], &managed);
        assert_eq!(
            got.managed.get(&id("app")),
            Some(&vec![id("b-short")]),
            "the shorter path wins"
        );
        let got = reach(&ws, [&id("store"), &id("app")], &managed);
        assert_eq!(got.managed.get(&id("app")), Some(&Vec::new()));
    }

    /// `store→a→z→app` and `store→b→y→app`: equal length, and the nearest
    /// package decides, so `[y, b, store]` beats `[z, a, store]`.
    #[test]
    fn equal_paths_from_one_change_fall_to_the_lowest_name_nearest_first() {
        let ws = workspace(vec![
            package(
                "app",
                vec![
                    on("y", DependencyKind::Normal),
                    on("z", DependencyKind::Normal),
                ],
            ),
            package("z", vec![on("a", DependencyKind::Normal)]),
            package("y", vec![on("b", DependencyKind::Normal)]),
            package("a", vec![on("store", DependencyKind::Normal)]),
            package("b", vec![on("store", DependencyKind::Normal)]),
            package("store", vec![]),
        ]);
        let got = reach(&ws, [&id("store")], managed_set(&["app"]));
        assert_eq!(
            got.managed.get(&id("app")),
            Some(&vec![id("y"), id("b"), id("store")])
        );
    }

    /// Two changes reaching `app` by paths of one length: the order they are
    /// read in does not decide.
    #[test]
    fn equal_paths_from_two_changes_fall_to_the_lowest_name() {
        let ws = workspace(vec![
            package(
                "app",
                vec![
                    on("w", DependencyKind::Normal),
                    on("z", DependencyKind::Normal),
                ],
            ),
            package("z", vec![on("x", DependencyKind::Normal)]),
            package("w", vec![on("y", DependencyKind::Normal)]),
            package("x", vec![]),
            package("y", vec![]),
        ]);
        let managed = managed_set(&["app"]);
        for changed in [[id("x"), id("y")], [id("y"), id("x")]] {
            let got = reach(&ws, &changed, &managed);
            assert_eq!(got.managed.get(&id("app")), Some(&vec![id("w"), id("y")]));
        }
    }

    #[test]
    fn a_self_edge_ends_the_walk() {
        let ws = workspace(vec![
            package("app", vec![on("store", DependencyKind::Normal)]),
            package("store", vec![on("store", DependencyKind::Development)]),
        ]);
        let got = reach(&ws, [&id("store")], managed_set(&["app"]));
        assert_eq!(got.managed.get(&id("app")), Some(&vec![id("store")]));
    }

    #[test]
    fn a_managed_change_is_direct_and_never_stranded() {
        let ws = workspace(vec![package("app", vec![])]);
        let got = reach(&ws, [&id("app")], managed_set(&["app"]));
        assert_eq!(got.managed.get(&id("app")), Some(&Vec::new()));
        assert!(got.stranded.is_empty());
    }
}
