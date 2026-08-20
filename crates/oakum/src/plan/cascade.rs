//! Cascade eligibility along a runtime edge (ADR-0008 / ADR-0009 / ADR-0026).
//!
//! ADR-0008 decides which edges can fire. ADR-0009 decides that a dependent
//! resolving dependencies at build time always fires on those edges. ADR-0026
//! decides the same for path-linked edges with no published range. The range
//! gate (ADR-0010 / `okm-tnp`) applies only to install-time dependents on
//! ranged edges.

use alloc::collections::BTreeSet;
use core::fmt;

use super::workspace::{Dependency, Package, PackageId, Workspace};

/// Whether releasing a dependency along this edge requires releasing the
/// dependent, before the published-range check runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CascadeDecision {
    /// Build-time dependent (ADR-0009) or path-linked edge (ADR-0026). Cascade
    /// regardless of whether a published range would still match.
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

/// Classify under ADR-0008, ADR-0009, and ADR-0026.
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

/// Deduped Always projection of [`cascade_decision`] for a release walk.
///
/// Each package appears at most once even if it declares several runtime edges
/// onto `id` (for example peer + optional). Self-edges are skipped — they still
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

#[cfg(test)]
mod tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use semver::Version;

    use super::*;
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
        Package::new(id, Version::new(0, 1, 3), resolves, dependencies)
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
}
