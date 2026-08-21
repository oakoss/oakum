//! Cascade eligibility along a runtime edge (ADR-0008 / ADR-0009 / ADR-0026)
//! and the published-range gate (ADR-0010).
//!
//! ADR-0008 decides which edges can fire. ADR-0009 decides that a dependent
//! resolving dependencies at build time always fires on those edges. ADR-0026
//! decides the same for path-linked edges with no published range. ADR-0010
//! decides whether a [`CascadeDecision::IfRangeUnsatisfied`] edge actually
//! cascades: when the dependent's published range no longer admits the
//! dependency's new version.

use alloc::collections::BTreeSet;
use core::fmt;

use semver::Version;

use super::bump::BumpLevel;
use super::workspace::{DeclaredRange, Dependency, Package, PackageId, Workspace};

/// Whether releasing a dependency along this edge requires releasing the
/// dependent, before the published-range check runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CascadeDecision {
    /// Build-time dependent (ADR-0009) or path-linked edge (ADR-0026). Cascade
    /// regardless of whether a published range would still match. On
    /// [`cascade_decision`] this is the working-tree edge; tagged `PathLinked`
    /// Always is decided via [`edge_cascades`]'s `published_range`.
    Always,
    /// The dependent re-resolves at install time on a ranged edge. Cascade only
    /// when the published range would no longer resolve (`okm-tnp`).
    IfRangeUnsatisfied,
    /// Not a runtime edge (ADR-0008).
    Never,
}

impl CascadeDecision {
    #[must_use]
    pub const fn is_always(self) -> bool {
        matches!(self, Self::Always)
    }
}

impl fmt::Display for CascadeDecision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Always => "always",
            Self::IfRangeUnsatisfied => "if-range-unsatisfied",
            Self::Never => "never",
        })
    }
}

/// How far a cascaded dependent moves when an edge fires.
///
/// Preference (ADR-0004), not a graph fact. Defaults to patch; overridable
/// globally or per package later. Distinct from [`BumpLevel`] so `none`
/// (rewrite the dependency line without bumping the dependent) stays expressible.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CascadeAs {
    #[default]
    Patch,
    Minor,
    /// Rewrite the dependency declaration only; do not bump the dependent.
    None,
}

impl CascadeAs {
    #[must_use]
    pub const fn bump_level(self) -> Option<BumpLevel> {
        match self {
            Self::Patch => Some(BumpLevel::Patch),
            Self::Minor => Some(BumpLevel::Minor),
            Self::None => None,
        }
    }
}

impl fmt::Display for CascadeAs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Patch => "patch",
            Self::Minor => "minor",
            Self::None => "none",
        })
    }
}

/// Classify under ADR-0008, ADR-0009, and ADR-0026 for the **working-tree**
/// edge. Path-linked Always here reads [`Dependency::range`]; tagged release
/// evaluation uses [`edge_cascades`] / [`cascading_dependents`], which take the
/// published range separately (ADR-0014).
///
/// `edge` must be one of `dependent`'s declared dependencies.
#[must_use]
pub fn cascade_decision(dependent: &Package, edge: &Dependency) -> CascadeDecision {
    if !edge.kind.is_runtime() {
        return CascadeDecision::Never;
    }
    if dependent.resolves_dependencies_at().is_build() || edge.range.is_path_linked() {
        return CascadeDecision::Always;
    }
    CascadeDecision::IfRangeUnsatisfied
}

/// Whether this edge cascades for a dependency moving from
/// `version_at_last_tag` to `new_version` (ADR-0010).
///
/// `published_range` is the dependent's declaration as of the last reachable
/// tag (ADR-0014), not a working-tree rewrite inside an open version PR.
/// Passing the rewritten range under-releases: a PR that already moved
/// `^0.1.3` → `^0.2.0` would treat `0.2.0` as satisfied and drop the dependent.
/// Path-linked Always (ADR-0026) is read from `published_range` for the same
/// reason: a working-tree path-only rewrite must not bypass a tagged ranged
/// declaration that still admits.
///
/// `version_at_last_tag` is the dependency's version before the bump under
/// consideration, not the version being bumped *to*.
#[must_use]
pub fn edge_cascades(
    dependent: &Package,
    edge: &Dependency,
    published_range: &DeclaredRange,
    version_at_last_tag: &Version,
    new_version: &Version,
) -> bool {
    if !edge.kind.is_runtime() {
        return false;
    }
    if dependent.resolves_dependencies_at().is_build() || published_range.is_path_linked() {
        return true;
    }
    !published_range.admits(version_at_last_tag, new_version)
}

/// Deduped Always projection of working-tree [`cascade_decision`].
///
/// For explain / current-manifest classification, not a tagged release walk.
/// Path-linked Always here follows [`Dependency::range`]. Release planning that
/// must honour ADR-0014 uses [`cascading_dependents`] instead.
///
/// Each package appears at most once even if it declares several runtime edges
/// onto `id` (for example peer + optional). Self-edges are skipped: they still
/// appear in [`Workspace::runtime_dependents`] but cannot cascade onto another
/// package. Order follows [`PackageId`] via the workspace's package map.
pub fn always_cascading_dependents<'a, 'b>(
    workspace: &'a Workspace,
    id: &'b PackageId,
) -> impl Iterator<Item = &'a Package> + use<'a, 'b> {
    let mut seen = BTreeSet::new();
    workspace
        .runtime_dependents(id)
        .filter(move |(package, edge)| {
            package.id() != id && cascade_decision(package, edge).is_always()
        })
        .filter_map(move |(package, _)| {
            if seen.insert(package.id().clone()) {
                Some(package)
            } else {
                None
            }
        })
}

/// Dependents that cascade when `id` moves from `version_at_last_tag` to
/// `new_version`, including Always edges and range-unsatisfied install edges.
///
/// Delivery artifacts (ADR-0009) short-circuit before `published_range_of`: a
/// build-time edge added after the last tag has no historical declaration, and
/// the cascade does not need one. For install-time edges, `published_range_of`
/// returns the declaration as of the last reachable tag (ADR-0014), or `None`
/// when that edge did not exist then (newly added); those do not cascade under
/// the published-range rule. Discovery owns the lookup.
///
/// Edges removed since the last tag are not visible on the working-tree
/// workspace; discovery must feed those separately (deferred to compose /
/// tagged-graph wiring).
///
/// Deduped and ordered like [`always_cascading_dependents`]. Self-edges skipped.
pub fn cascading_dependents<'a, 'b, F>(
    workspace: &'a Workspace,
    id: &'b PackageId,
    version_at_last_tag: &'b Version,
    new_version: &'b Version,
    mut published_range_of: F,
) -> impl Iterator<Item = &'a Package> + use<'a, 'b, F>
where
    F: FnMut(&Package, &Dependency) -> Option<DeclaredRange> + 'b,
{
    let mut seen = BTreeSet::new();
    workspace
        .runtime_dependents(id)
        .filter(move |(package, edge)| {
            if package.id() == id || !edge.kind.is_runtime() {
                return false;
            }
            if package.resolves_dependencies_at().is_build() {
                return true;
            }
            published_range_of(package, edge).is_some_and(|published| {
                edge_cascades(package, edge, &published, version_at_last_tag, new_version)
            })
        })
        .filter_map(move |(package, _)| {
            if seen.insert(package.id().clone()) {
                Some(package)
            } else {
                None
            }
        })
}

#[cfg(test)]
mod tests {
    use alloc::format;
    use alloc::vec;
    use alloc::vec::Vec;

    use semver::Version;

    use super::*;
    use crate::plan::bump::BumpLevel;
    use crate::plan::workspace::{
        BuildResolution, DeclaredRange, Dependency, DependencyKind, Ecosystem, Package, PackageId,
        ResolvesDependenciesAt, Workspace,
    };

    fn cargo(name: &str) -> PackageId {
        PackageId::new(Ecosystem::Cargo, name)
    }

    fn npm(name: &str) -> PackageId {
        PackageId::new(Ecosystem::Npm, name)
    }

    fn edge(on: PackageId, kind: DependencyKind) -> Dependency {
        let declared_as = on.name.clone();
        let range = match on.ecosystem {
            Ecosystem::Cargo => {
                DeclaredRange::Plain(crate::plan::Bounds::from_cargo_text("^0.1.3").expect("range"))
            }
            Ecosystem::Npm => {
                DeclaredRange::Plain(crate::plan::Bounds::from_npm_text("^0.1.3").expect("range"))
            }
        };
        Dependency {
            on,
            kind,
            declared_as,
            target: None,
            range,
        }
    }

    fn package(
        id: PackageId,
        resolves: ResolvesDependenciesAt,
        dependencies: Vec<Dependency>,
    ) -> Package {
        Package::new(id, Version::new(0, 1, 3), resolves, true, dependencies)
    }

    #[test]
    fn a_binary_always_cascades_on_a_runtime_edge() {
        let binary = package(
            cargo("cli"),
            ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget),
            vec![edge(cargo("core"), DependencyKind::Normal)],
        );
        let edge = &binary.dependencies()[0];
        assert_eq!(
            cascade_decision(&binary, edge),
            CascadeDecision::Always,
            "linesmith shape: binary + ^0.1.3 must bump when core patches"
        );
        assert!(cascade_decision(&binary, edge).is_always());
    }

    #[test]
    fn a_declared_build_library_always_cascades_on_a_runtime_edge() {
        let bundled = package(
            cargo("plugin"),
            ResolvesDependenciesAt::Build(BuildResolution::Declared),
            vec![edge(cargo("core"), DependencyKind::Normal)],
        );
        assert_eq!(
            cascade_decision(&bundled, &bundled.dependencies()[0]),
            CascadeDecision::Always
        );
    }

    #[test]
    fn an_install_library_defers_to_the_range_gate() {
        let library = package(
            cargo("lib"),
            ResolvesDependenciesAt::Install,
            vec![edge(cargo("core"), DependencyKind::Normal)],
        );
        assert_eq!(
            cascade_decision(&library, &library.dependencies()[0]),
            CascadeDecision::IfRangeUnsatisfied
        );
        assert!(!cascade_decision(&library, &library.dependencies()[0]).is_always());
    }

    #[test]
    fn a_path_linked_edge_always_cascades_even_for_install_time() {
        let mut dep = edge(cargo("core"), DependencyKind::Normal);
        dep.range = DeclaredRange::PathLinked;
        let library = package(cargo("lib"), ResolvesDependenciesAt::Install, vec![dep]);
        assert_eq!(
            cascade_decision(&library, &library.dependencies()[0]),
            CascadeDecision::Always
        );
    }

    #[test]
    fn always_cascading_dependents_includes_path_linked_install_libraries() {
        let mut path = edge(cargo("core"), DependencyKind::Normal);
        path.range = DeclaredRange::PathLinked;
        let workspace = Workspace::new([
            package(cargo("core"), ResolvesDependenciesAt::Install, vec![]),
            package(cargo("lib"), ResolvesDependenciesAt::Install, vec![path]),
        ])
        .expect("workspace");

        let ids: Vec<&PackageId> = always_cascading_dependents(&workspace, &cargo("core"))
            .map(Package::id)
            .collect();
        assert_eq!(ids, vec![&cargo("lib")]);
    }

    #[test]
    fn an_install_library_defers_even_on_a_build_dependency() {
        let library = package(
            cargo("lib"),
            ResolvesDependenciesAt::Install,
            vec![edge(cargo("codegen"), DependencyKind::Build)],
        );
        assert_eq!(
            cascade_decision(&library, &library.dependencies()[0]),
            CascadeDecision::IfRangeUnsatisfied
        );
    }

    #[test]
    fn a_development_edge_never_cascades_even_for_a_binary() {
        let binary = package(
            cargo("cli"),
            ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget),
            vec![edge(cargo("config"), DependencyKind::Development)],
        );
        assert_eq!(
            cascade_decision(&binary, &binary.dependencies()[0]),
            CascadeDecision::Never
        );
        assert!(!cascade_decision(&binary, &binary.dependencies()[0]).is_always());
    }

    #[test]
    fn a_development_edge_never_cascades_for_a_declared_build_library() {
        let bundled = package(
            cargo("plugin"),
            ResolvesDependenciesAt::Build(BuildResolution::Declared),
            vec![edge(cargo("config"), DependencyKind::Development)],
        );
        assert_eq!(
            cascade_decision(&bundled, &bundled.dependencies()[0]),
            CascadeDecision::Never
        );
    }

    #[test]
    fn build_dependencies_are_runtime_and_can_always_cascade() {
        let binary = package(
            cargo("cli"),
            ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget),
            vec![edge(cargo("codegen"), DependencyKind::Build)],
        );
        assert_eq!(
            cascade_decision(&binary, &binary.dependencies()[0]),
            CascadeDecision::Always
        );
    }

    #[test]
    fn peer_and_optional_edges_follow_the_same_cascade_rules() {
        for kind in [DependencyKind::Peer, DependencyKind::Optional] {
            let binary = package(
                npm("cli"),
                ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget),
                vec![edge(npm("host"), kind)],
            );
            assert_eq!(
                cascade_decision(&binary, &binary.dependencies()[0]),
                CascadeDecision::Always,
                "{kind} on a build-time dependent"
            );

            let library = package(
                npm("lib"),
                ResolvesDependenciesAt::Install,
                vec![edge(npm("host"), kind)],
            );
            assert_eq!(
                cascade_decision(&library, &library.dependencies()[0]),
                CascadeDecision::IfRangeUnsatisfied,
                "{kind} on an install-time dependent"
            );
        }
    }

    #[test]
    fn always_cascading_dependents_lists_binaries_once() {
        let workspace = Workspace::new([
            package(npm("host"), ResolvesDependenciesAt::Install, vec![]),
            package(
                npm("plugin"),
                ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget),
                vec![
                    edge(npm("host"), DependencyKind::Peer),
                    edge(npm("host"), DependencyKind::Optional),
                ],
            ),
            package(
                npm("lib"),
                ResolvesDependenciesAt::Install,
                vec![edge(npm("host"), DependencyKind::Normal)],
            ),
            package(
                npm("tool"),
                ResolvesDependenciesAt::Build(BuildResolution::Declared),
                vec![edge(npm("host"), DependencyKind::Normal)],
            ),
            package(
                npm("dev-only"),
                ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget),
                vec![edge(npm("host"), DependencyKind::Development)],
            ),
        ])
        .expect("workspace");

        let ids: Vec<&PackageId> = always_cascading_dependents(&workspace, &npm("host"))
            .map(Package::id)
            .collect();
        assert_eq!(ids, vec![&npm("plugin"), &npm("tool")]);
    }

    #[test]
    fn a_self_edge_is_not_an_always_cascading_dependent() {
        let id = cargo("cli");
        let workspace = Workspace::new([package(
            id.clone(),
            ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget),
            vec![edge(id.clone(), DependencyKind::Normal)],
        )])
        .expect("self-edge workspace");

        assert!(always_cascading_dependents(&workspace, &id)
            .next()
            .is_none());
    }

    #[test]
    fn install_library_cascades_only_when_the_published_range_excludes_the_new_version() {
        let library = package(
            cargo("lib"),
            ResolvesDependenciesAt::Install,
            vec![edge(cargo("core"), DependencyKind::Normal)],
        );
        let edge = &library.dependencies()[0];
        let at_tag = Version::new(0, 1, 3);

        assert!(!edge_cascades(
            &library,
            edge,
            &edge.range,
            &at_tag,
            &Version::new(0, 1, 4)
        ));
        // release-plz / linesmith ADR-0027 row
        assert!(edge_cascades(
            &library,
            edge,
            &edge.range,
            &at_tag,
            &Version::new(0, 2, 0)
        ));
    }

    #[test]
    fn open_pr_rewrite_must_not_suppress_cascade() {
        let mut working = edge(cargo("core"), DependencyKind::Normal);
        working.range =
            DeclaredRange::Plain(crate::plan::Bounds::from_cargo_text("^0.2.0").expect("range"));
        let published =
            DeclaredRange::Plain(crate::plan::Bounds::from_cargo_text("^0.1.3").expect("range"));
        let library = package(cargo("lib"), ResolvesDependenciesAt::Install, vec![working]);
        let edge = &library.dependencies()[0];
        let at_tag = Version::new(0, 1, 3);
        let new = Version::new(0, 2, 0);

        assert!(
            edge_cascades(&library, edge, &published, &at_tag, &new),
            "tagged ^0.1.3 still excludes 0.2.0"
        );
        assert!(
            !edge_cascades(&library, edge, &edge.range, &at_tag, &new),
            "working-tree ^0.2.0 would wrongly suppress"
        );
    }

    #[test]
    fn working_tree_path_linked_does_not_bypass_published_range() {
        let mut working = edge(cargo("core"), DependencyKind::Normal);
        working.range = DeclaredRange::PathLinked;
        let published =
            DeclaredRange::Plain(crate::plan::Bounds::from_cargo_text("^0.1.3").expect("range"));
        let library = package(cargo("lib"), ResolvesDependenciesAt::Install, vec![working]);
        let edge = &library.dependencies()[0];
        let at_tag = Version::new(0, 1, 3);

        assert!(
            !edge_cascades(&library, edge, &published, &at_tag, &Version::new(0, 1, 4)),
            "tagged ^0.1.3 still admits 0.1.4"
        );
        assert!(edge_cascades(
            &library,
            edge,
            &DeclaredRange::PathLinked,
            &at_tag,
            &Version::new(0, 1, 4)
        ));
    }

    #[test]
    fn development_edges_never_cascade() {
        let library = package(
            cargo("lib"),
            ResolvesDependenciesAt::Install,
            vec![edge(cargo("core"), DependencyKind::Development)],
        );
        let edge = &library.dependencies()[0];
        let at_tag = Version::new(0, 1, 3);
        assert!(!edge_cascades(
            &library,
            edge,
            &edge.range,
            &at_tag,
            &Version::new(0, 2, 0)
        ));
    }

    #[test]
    fn caret_above_one_only_cascades_on_major() {
        let mut dep = edge(cargo("core"), DependencyKind::Normal);
        dep.range =
            DeclaredRange::Plain(crate::plan::Bounds::from_cargo_text("^1.5.0").expect("range"));
        let library = package(cargo("lib"), ResolvesDependenciesAt::Install, vec![dep]);
        let edge = &library.dependencies()[0];
        let at_tag = Version::new(1, 5, 0);

        assert!(!edge_cascades(
            &library,
            edge,
            &edge.range,
            &at_tag,
            &Version::new(1, 6, 0)
        ));
        assert!(edge_cascades(
            &library,
            edge,
            &edge.range,
            &at_tag,
            &Version::new(2, 0, 0)
        ));
    }

    #[test]
    fn exact_tracking_cascades_on_any_bump() {
        let mut dep = edge(npm("core"), DependencyKind::Normal);
        dep.range = DeclaredRange::WorkspaceTracking(crate::plan::workspace::Tracking::Exact);
        let library = package(npm("lib"), ResolvesDependenciesAt::Install, vec![dep]);
        let edge = &library.dependencies()[0];
        let at_tag = Version::new(1, 5, 0);
        let new = Version::new(1, 5, 1);

        assert!(!edge_cascades(
            &library,
            edge,
            &edge.range,
            &at_tag,
            &at_tag
        ));
        assert!(edge_cascades(&library, edge, &edge.range, &at_tag, &new));
        assert!(
            !edge_cascades(&library, edge, &edge.range, &new, &new),
            "passing new_version as version_at_last_tag expands Exact to a self-match"
        );
    }

    #[test]
    fn tilde_and_caret_tracking_follow_published_expansion() {
        let at_tag = Version::new(1, 5, 0);
        let mut tilde = edge(npm("core"), DependencyKind::Normal);
        tilde.range = DeclaredRange::WorkspaceTracking(crate::plan::workspace::Tracking::Tilde);
        let tilde_lib = package(npm("lib"), ResolvesDependenciesAt::Install, vec![tilde]);
        let tilde_edge = &tilde_lib.dependencies()[0];
        assert!(!edge_cascades(
            &tilde_lib,
            tilde_edge,
            &tilde_edge.range,
            &at_tag,
            &Version::new(1, 5, 1)
        ));
        assert!(edge_cascades(
            &tilde_lib,
            tilde_edge,
            &tilde_edge.range,
            &at_tag,
            &Version::new(1, 6, 0)
        ));

        let mut caret = edge(npm("core"), DependencyKind::Normal);
        caret.range = DeclaredRange::WorkspaceTracking(crate::plan::workspace::Tracking::Caret);
        let caret_lib = package(npm("lib"), ResolvesDependenciesAt::Install, vec![caret]);
        let caret_edge = &caret_lib.dependencies()[0];
        assert!(!edge_cascades(
            &caret_lib,
            caret_edge,
            &caret_edge.range,
            &at_tag,
            &Version::new(1, 6, 0)
        ));
        assert!(edge_cascades(
            &caret_lib,
            caret_edge,
            &caret_edge.range,
            &at_tag,
            &Version::new(2, 0, 0)
        ));
    }

    #[test]
    fn exact_pin_cascades_on_any_bump() {
        let mut dep = edge(npm("core"), DependencyKind::Normal);
        dep.range =
            DeclaredRange::Plain(crate::plan::Bounds::from_npm_text("1.5.0").expect("range"));
        let library = package(npm("lib"), ResolvesDependenciesAt::Install, vec![dep]);
        let edge = &library.dependencies()[0];
        let at_tag = Version::new(1, 5, 0);

        assert!(!edge_cascades(
            &library,
            edge,
            &edge.range,
            &at_tag,
            &at_tag
        ));
        assert!(edge_cascades(
            &library,
            edge,
            &edge.range,
            &at_tag,
            &Version::new(1, 5, 1)
        ));
    }

    #[test]
    fn tilde_cascades_on_minor_and_major() {
        let mut dep = edge(cargo("core"), DependencyKind::Normal);
        dep.range =
            DeclaredRange::Plain(crate::plan::Bounds::from_cargo_text("~1.5.0").expect("range"));
        let library = package(cargo("lib"), ResolvesDependenciesAt::Install, vec![dep]);
        let edge = &library.dependencies()[0];
        let at_tag = Version::new(1, 5, 0);

        assert!(!edge_cascades(
            &library,
            edge,
            &edge.range,
            &at_tag,
            &Version::new(1, 5, 1)
        ));
        assert!(edge_cascades(
            &library,
            edge,
            &edge.range,
            &at_tag,
            &Version::new(1, 6, 0)
        ));
        assert!(edge_cascades(
            &library,
            edge,
            &edge.range,
            &at_tag,
            &Version::new(2, 0, 0)
        ));
    }

    #[test]
    fn always_edges_ignore_the_range_gate() {
        let binary = package(
            cargo("cli"),
            ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget),
            vec![edge(cargo("core"), DependencyKind::Normal)],
        );
        let edge = &binary.dependencies()[0];
        let at_tag = Version::new(0, 1, 3);
        // Range admits; Always still fires.
        assert!(edge_cascades(
            &binary,
            edge,
            &edge.range,
            &at_tag,
            &Version::new(0, 1, 4)
        ));
    }

    #[test]
    fn cascading_dependents_includes_range_unsatisfied_install_libraries() {
        let workspace = Workspace::new([
            package(cargo("core"), ResolvesDependenciesAt::Install, vec![]),
            package(
                cargo("lib"),
                ResolvesDependenciesAt::Install,
                vec![edge(cargo("core"), DependencyKind::Normal)],
            ),
            package(
                cargo("cli"),
                ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget),
                vec![edge(cargo("core"), DependencyKind::Normal)],
            ),
            package(
                cargo("tool"),
                ResolvesDependenciesAt::Install,
                vec![edge(cargo("core"), DependencyKind::Development)],
            ),
        ])
        .expect("workspace");

        let at_tag = Version::new(0, 1, 3);
        let patch = Version::new(0, 1, 4);
        let minor = Version::new(0, 2, 0);

        let patch_ids: Vec<&PackageId> =
            cascading_dependents(&workspace, &cargo("core"), &at_tag, &patch, |_, edge| {
                Some(edge.range.clone())
            })
            .map(Package::id)
            .collect();
        assert_eq!(patch_ids, vec![&cargo("cli")]);

        let minor_ids: Vec<&PackageId> =
            cascading_dependents(&workspace, &cargo("core"), &at_tag, &minor, |_, edge| {
                Some(edge.range.clone())
            })
            .map(Package::id)
            .collect();
        assert_eq!(minor_ids, vec![&cargo("cli"), &cargo("lib")]);
        assert!(
            !minor_ids.contains(&&cargo("tool")),
            "Development edges stay Never"
        );
    }

    #[test]
    fn cascading_dependents_uses_published_range_lookup() {
        let mut rewritten = edge(cargo("core"), DependencyKind::Normal);
        rewritten.range =
            DeclaredRange::Plain(crate::plan::Bounds::from_cargo_text("^0.2.0").expect("range"));
        let published =
            DeclaredRange::Plain(crate::plan::Bounds::from_cargo_text("^0.1.3").expect("range"));
        let workspace = Workspace::new([
            package(cargo("core"), ResolvesDependenciesAt::Install, vec![]),
            package(
                cargo("lib"),
                ResolvesDependenciesAt::Install,
                vec![rewritten],
            ),
        ])
        .expect("workspace");

        let at_tag = Version::new(0, 1, 3);
        let new = Version::new(0, 2, 0);

        let from_working_tree: Vec<&PackageId> =
            cascading_dependents(&workspace, &cargo("core"), &at_tag, &new, |_, edge| {
                Some(edge.range.clone())
            })
            .map(Package::id)
            .collect();
        assert!(
            from_working_tree.is_empty(),
            "rewritten ^0.2.0 on the edge under-releases without a tagged lookup"
        );

        let from_tag: Vec<&PackageId> =
            cascading_dependents(&workspace, &cargo("core"), &at_tag, &new, |_, _| {
                Some(published.clone())
            })
            .map(Package::id)
            .collect();
        assert_eq!(from_tag, vec![&cargo("lib")]);
    }

    #[test]
    fn cascading_dependents_path_linked_uses_published_range_of() {
        let mut path_linked = edge(cargo("core"), DependencyKind::Normal);
        path_linked.range = DeclaredRange::PathLinked;
        let published =
            DeclaredRange::Plain(crate::plan::Bounds::from_cargo_text("^0.1.3").expect("range"));
        let workspace = Workspace::new([
            package(cargo("core"), ResolvesDependenciesAt::Install, vec![]),
            package(
                cargo("lib"),
                ResolvesDependenciesAt::Install,
                vec![path_linked],
            ),
        ])
        .expect("workspace");

        let at_tag = Version::new(0, 1, 3);
        let patch = Version::new(0, 1, 4);

        let from_tag: Vec<&PackageId> =
            cascading_dependents(&workspace, &cargo("core"), &at_tag, &patch, |_, _| {
                Some(published.clone())
            })
            .map(Package::id)
            .collect();
        assert!(
            from_tag.is_empty(),
            "tagged ^0.1.3 still admits 0.1.4 despite WT PathLinked"
        );

        let published_path: Vec<&PackageId> =
            cascading_dependents(&workspace, &cargo("core"), &at_tag, &patch, |_, _| {
                Some(DeclaredRange::PathLinked)
            })
            .map(Package::id)
            .collect();
        assert_eq!(published_path, vec![&cargo("lib")]);
    }

    #[test]
    fn newly_added_install_edge_without_published_range_does_not_cascade() {
        let workspace = Workspace::new([
            package(cargo("core"), ResolvesDependenciesAt::Install, vec![]),
            package(
                cargo("lib"),
                ResolvesDependenciesAt::Install,
                vec![edge(cargo("core"), DependencyKind::Normal)],
            ),
        ])
        .expect("workspace");

        let at_tag = Version::new(0, 1, 3);
        let ids: Vec<&PackageId> = cascading_dependents(
            &workspace,
            &cargo("core"),
            &at_tag,
            &Version::new(0, 2, 0),
            |_, _| None,
        )
        .map(Package::id)
        .collect();
        assert!(ids.is_empty());
    }

    #[test]
    fn build_time_dependents_do_not_require_published_range() {
        let workspace = Workspace::new([
            package(cargo("core"), ResolvesDependenciesAt::Install, vec![]),
            package(
                cargo("cli"),
                ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget),
                vec![edge(cargo("core"), DependencyKind::Normal)],
            ),
        ])
        .expect("workspace");

        let at_tag = Version::new(0, 1, 3);
        let ids: Vec<&PackageId> = cascading_dependents(
            &workspace,
            &cargo("core"),
            &at_tag,
            &Version::new(0, 1, 4),
            |_, _| panic!("delivery Always must not look up a published range"),
        )
        .map(Package::id)
        .collect();
        assert_eq!(ids, vec![&cargo("cli")]);
    }

    #[test]
    fn catalog_range_is_not_waived_at_the_gate() {
        let mut dep = edge(npm("core"), DependencyKind::Normal);
        dep.range = DeclaredRange::Catalog {
            name: None,
            bounds: crate::plan::Bounds::from_npm_text("^0.1.3").expect("range"),
        };
        let library = package(npm("lib"), ResolvesDependenciesAt::Install, vec![dep]);
        let edge = &library.dependencies()[0];
        let at_tag = Version::new(0, 1, 3);

        assert!(!edge_cascades(
            &library,
            edge,
            &edge.range,
            &at_tag,
            &Version::new(0, 1, 4)
        ));
        assert!(edge_cascades(
            &library,
            edge,
            &edge.range,
            &at_tag,
            &Version::new(0, 2, 0)
        ));
    }

    #[test]
    fn a_self_edge_is_not_a_cascading_dependent() {
        let id = cargo("lib");
        let workspace = Workspace::new([package(
            id.clone(),
            ResolvesDependenciesAt::Install,
            vec![edge(id.clone(), DependencyKind::Normal)],
        )])
        .expect("self-edge workspace");

        let at_tag = Version::new(0, 1, 3);
        assert!(cascading_dependents(
            &workspace,
            &id,
            &at_tag,
            &Version::new(0, 2, 0),
            |_, edge| Some(edge.range.clone()),
        )
        .next()
        .is_none());
    }

    #[test]
    fn cascading_dependents_lists_install_libraries_once() {
        let mut peer = edge(npm("host"), DependencyKind::Peer);
        peer.range =
            DeclaredRange::Plain(crate::plan::Bounds::from_npm_text("^0.1.3").expect("range"));
        let mut optional = edge(npm("host"), DependencyKind::Optional);
        optional.range =
            DeclaredRange::Plain(crate::plan::Bounds::from_npm_text("^0.1.3").expect("range"));
        let workspace = Workspace::new([
            package(npm("host"), ResolvesDependenciesAt::Install, vec![]),
            package(
                npm("plugin"),
                ResolvesDependenciesAt::Install,
                vec![peer, optional],
            ),
        ])
        .expect("workspace");

        let at_tag = Version::new(0, 1, 3);
        let ids: Vec<&PackageId> = cascading_dependents(
            &workspace,
            &npm("host"),
            &at_tag,
            &Version::new(0, 2, 0),
            |_, edge| Some(edge.range.clone()),
        )
        .map(Package::id)
        .collect();
        assert_eq!(ids, vec![&npm("plugin")]);
    }

    #[test]
    fn cascade_as_maps_to_bump_level() {
        assert_eq!(CascadeAs::default(), CascadeAs::Patch);
        assert_eq!(CascadeAs::Patch.bump_level(), Some(BumpLevel::Patch));
        assert_eq!(CascadeAs::Minor.bump_level(), Some(BumpLevel::Minor));
        assert_eq!(CascadeAs::None.bump_level(), None);
        assert_eq!(format!("{}", CascadeAs::None), "none");
    }
}
