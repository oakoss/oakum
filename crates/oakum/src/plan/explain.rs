//! Decision trace for `--explain` (ADR-0009 / ADR-0010 / ADR-0022 / ADR-0026).
//!
//! Built from a finished [`Plan`] and the workspace facts compose used. Covers
//! every planned bump (including zero-major remaps) and every edge evaluation
//! from those bumps, including decisions not to cascade. States the reason for
//! a skip; does not warn when a library correctly stays put.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use semver::Version;

use super::bump::{AppliedBump, BumpLevel};
use super::cascade::{edge_cascades, CascadeAs, CascadeDecision};
use super::compose::{ChangeSource, Plan, PlannedChange};
use super::workspace::{DeclaredRange, Dependency, DependencyKind, Package, PackageId, Workspace};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Explain {
    entries: Vec<ExplainEntry>,
}

impl Explain {
    #[must_use]
    pub fn entries(&self) -> &[ExplainEntry] {
        &self.entries
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

impl fmt::Display for Explain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, entry) in self.entries.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{entry}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExplainEntry {
    Bump {
        id: PackageId,
        from: Version,
        to: Version,
        applied: AppliedBump,
        source: ChangeSource,
    },
    /// One dependent may declare the same package under several sections,
    /// aliases, or target tables; `kind`, `declared_as`, and `target` identify
    /// which edge ([`Workspace::dependents`] yields one item per edge).
    Edge {
        trigger: PackageId,
        dependent: PackageId,
        kind: DependencyKind,
        declared_as: String,
        target: Option<String>,
        /// Same rules as [`super::cascade::cascading_dependents`], not
        /// working-tree [`super::cascade::cascade_decision`].
        decision: CascadeDecision,
        published_range: Option<DeclaredRange>,
        action: EdgeAction,
    },
}

impl fmt::Display for ExplainEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bump {
                id,
                from,
                to,
                applied,
                source,
            } => {
                write!(f, "bump {id}: {from} → {to} ({})", applied.effective())?;
                if applied.was_remapped() {
                    write!(
                        f,
                        "; {} remapped {} → {}",
                        applied.versioning(),
                        applied.requested(),
                        applied.effective()
                    )?;
                }
                match source {
                    ChangeSource::Intent => write!(f, "; intent"),
                    ChangeSource::Cascade { trigger } => {
                        write!(f, "; cascade from {trigger}")
                    }
                }
            }
            Self::Edge {
                trigger,
                dependent,
                kind,
                declared_as,
                target,
                decision,
                published_range,
                action,
            } => {
                write!(f, "edge {trigger} → {dependent} ({kind} as {declared_as}")?;
                if let Some(target) = target {
                    write!(f, ", target {target}")?;
                }
                write!(f, "): {decision}")?;
                if let Some(range) = published_range {
                    match range {
                        DeclaredRange::PathLinked => {
                            write!(f, "; published path-linked (no version)")?;
                        }
                        DeclaredRange::Catalog { bounds, .. } => {
                            write!(f, "; published {range} (= {bounds})")?;
                        }
                        _ => write!(f, "; published {range}")?,
                    }
                }
                write!(f, " → {action}")
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EdgeAction {
    Cascade(BumpLevel),
    /// Dependency rewrite only ([`CascadeAs::None`]).
    RewriteOnly,
    /// Not a runtime edge (ADR-0008).
    SkipNotRuntime,
    /// Install-time edge; the published range still admits the new version.
    SkipRangeSatisfied,
    /// Install-time edge with no declaration at the last reachable tag.
    SkipNoPublishedRange,
}

impl fmt::Display for EdgeAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cascade(level) => write!(f, "cascade {level}"),
            Self::RewriteOnly => f.write_str("rewrite only"),
            Self::SkipNotRuntime => f.write_str("skip (not runtime)"),
            Self::SkipRangeSatisfied => f.write_str("skip (range still satisfied)"),
            Self::SkipNoPublishedRange => f.write_str("skip (no published range)"),
        }
    }
}

/// Trace a finished plan under the same cascade inputs compose used.
///
/// Every planned package is a trigger; each declared edge onto it is recorded
/// (including development). Self-edges are omitted: they cannot cascade onto
/// another package.
///
/// Classification matches [`super::cascade::cascading_dependents`]: build-time
/// Always short-circuits before `published_range_of`; install-time edges use the
/// published declaration (ADR-0014), including path-linked Always (ADR-0026).
///
/// `cascade_as` and `published_range_of` must match the values passed to
/// [`super::compose::compose`] for `plan`. Mismatched inputs produce a trace that
/// does not describe that plan.
pub fn explain_plan<R>(
    workspace: &Workspace,
    plan: &Plan,
    cascade_as: CascadeAs,
    mut published_range_of: R,
) -> Explain
where
    R: FnMut(&Package, &Dependency) -> Option<DeclaredRange>,
{
    let mut entries = Vec::new();

    for change in plan.changes().values() {
        entries.push(bump_entry(change));

        for (dependent, edge) in workspace.dependents(change.id()) {
            if dependent.id() == change.id() {
                continue;
            }
            entries.push(edge_entry(
                change,
                dependent,
                edge,
                cascade_as,
                &mut published_range_of,
            ));
        }
    }

    Explain { entries }
}

fn bump_entry(change: &PlannedChange) -> ExplainEntry {
    ExplainEntry::Bump {
        id: change.id().clone(),
        from: change.from().clone(),
        to: change.to().clone(),
        applied: change.applied(),
        source: change.source().clone(),
    }
}

fn edge_entry<R>(
    trigger: &PlannedChange,
    dependent: &Package,
    edge: &Dependency,
    cascade_as: CascadeAs,
    published_range_of: &mut R,
) -> ExplainEntry
where
    R: FnMut(&Package, &Dependency) -> Option<DeclaredRange>,
{
    let identity = |decision, published_range, action| ExplainEntry::Edge {
        trigger: trigger.id().clone(),
        dependent: dependent.id().clone(),
        kind: edge.kind,
        declared_as: edge.declared_as.clone(),
        target: edge.target.clone(),
        decision,
        published_range,
        action,
    };

    if !edge.kind.is_runtime() {
        return identity(CascadeDecision::Never, None, EdgeAction::SkipNotRuntime);
    }

    if dependent.resolves_dependencies_at().is_build() {
        return identity(CascadeDecision::Always, None, fire_action(cascade_as));
    }

    match published_range_of(dependent, edge) {
        None => identity(
            CascadeDecision::IfRangeUnsatisfied,
            None,
            EdgeAction::SkipNoPublishedRange,
        ),
        Some(published) => {
            let decision = if published.is_path_linked() {
                CascadeDecision::Always
            } else {
                CascadeDecision::IfRangeUnsatisfied
            };
            let fires = edge_cascades(dependent, edge, &published, trigger.from(), trigger.to());
            let action = if fires {
                fire_action(cascade_as)
            } else {
                EdgeAction::SkipRangeSatisfied
            };
            identity(decision, Some(published), action)
        }
    }
}

fn fire_action(cascade_as: CascadeAs) -> EdgeAction {
    match cascade_as.bump_level() {
        Some(level) => EdgeAction::Cascade(level),
        None => EdgeAction::RewriteOnly,
    }
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeMap;
    use alloc::string::{String, ToString};
    use alloc::vec;
    use alloc::vec::Vec;

    use semver::Version;

    use super::*;
    use crate::plan::aggregate::{aggregate, AggregatedBump, BumpFile};
    use crate::plan::bump::{BumpLevel, Versioning};
    use crate::plan::compose::compose;
    use crate::plan::workspace::{
        BuildResolution, DeclaredRange, Dependency, DependencyKind, Ecosystem, Package, PackageId,
        ResolvesDependenciesAt, Workspace,
    };

    fn cargo(name: &str) -> PackageId {
        PackageId::new(Ecosystem::Cargo, name)
    }

    fn edge(on: PackageId, kind: DependencyKind) -> Dependency {
        let declared_as = on.name.clone();
        let range =
            DeclaredRange::Plain(crate::plan::Bounds::from_cargo_text("^0.1.3").expect("range"));
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

    fn intent(entries: Vec<(PackageId, BumpLevel)>) -> BTreeMap<PackageId, AggregatedBump> {
        aggregate([BumpFile {
            id: String::from("change.md"),
            entries,
            note: String::from("note"),
        }])
    }

    fn plan_and_explain(
        workspace: &Workspace,
        entries: Vec<(PackageId, BumpLevel)>,
    ) -> (Plan, Explain) {
        let plan = compose(
            workspace,
            &intent(entries),
            |_| Versioning::ZeroMajor,
            CascadeAs::Patch,
            |_, edge| Some(edge.range.clone()),
            |id| {
                workspace
                    .get(id)
                    .expect("package in workspace")
                    .version()
                    .clone()
            },
        )
        .expect("plan");
        let explain = explain_plan(workspace, &plan, CascadeAs::Patch, |_, edge| {
            Some(edge.range.clone())
        });
        (plan, explain)
    }

    fn edge_actions(explain: &Explain) -> Vec<(&PackageId, &PackageId, &EdgeAction)> {
        explain
            .entries()
            .iter()
            .filter_map(|entry| match entry {
                ExplainEntry::Edge {
                    trigger,
                    dependent,
                    action,
                    ..
                } => Some((trigger, dependent, action)),
                ExplainEntry::Bump { .. } => None,
            })
            .collect()
    }

    fn edge_between<'a>(
        explain: &'a Explain,
        trigger: &PackageId,
        dependent: &PackageId,
    ) -> &'a ExplainEntry {
        explain
            .entries()
            .iter()
            .find(|entry| match entry {
                ExplainEntry::Edge {
                    trigger: t,
                    dependent: d,
                    ..
                } => t == trigger && d == dependent,
                ExplainEntry::Bump { .. } => false,
            })
            .expect("edge entry")
    }

    #[test]
    fn library_skip_names_range_still_satisfied() {
        let workspace = Workspace::new([
            package(cargo("core"), ResolvesDependenciesAt::Install, vec![]),
            package(
                cargo("lib"),
                ResolvesDependenciesAt::Install,
                vec![edge(cargo("core"), DependencyKind::Normal)],
            ),
        ])
        .expect("workspace");

        let (plan, explain) = plan_and_explain(&workspace, vec![(cargo("core"), BumpLevel::Patch)]);
        assert!(plan.get(&cargo("lib")).is_none());

        match edge_between(&explain, &cargo("core"), &cargo("lib")) {
            ExplainEntry::Edge {
                decision,
                action,
                published_range,
                ..
            } => {
                assert_eq!(*decision, CascadeDecision::IfRangeUnsatisfied);
                assert_eq!(*action, EdgeAction::SkipRangeSatisfied);
                assert!(published_range.is_some());
            }
            ExplainEntry::Bump { .. } => panic!("expected edge"),
        }

        let text = explain.to_string();
        assert!(text.contains("if-range-unsatisfied"), "{text}");
        assert!(text.contains("skip (range still satisfied)"), "{text}");
        assert!(text.contains("^0.1.3"), "{text}");
    }

    #[test]
    fn binary_always_cascade_is_stated() {
        let workspace = Workspace::new([
            package(cargo("core"), ResolvesDependenciesAt::Install, vec![]),
            package(
                cargo("cli"),
                ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget),
                vec![edge(cargo("core"), DependencyKind::Normal)],
            ),
        ])
        .expect("workspace");

        let (plan, explain) = plan_and_explain(&workspace, vec![(cargo("core"), BumpLevel::Patch)]);
        assert!(plan.get(&cargo("cli")).is_some());

        match edge_between(&explain, &cargo("core"), &cargo("cli")) {
            ExplainEntry::Edge {
                decision,
                published_range,
                action,
                ..
            } => {
                assert_eq!(*decision, CascadeDecision::Always);
                assert!(published_range.is_none());
                assert_eq!(*action, EdgeAction::Cascade(BumpLevel::Patch));
            }
            ExplainEntry::Bump { .. } => panic!("expected edge"),
        }

        let text = explain.to_string();
        assert!(text.contains("always"), "{text}");
        assert!(text.contains("cascade from"), "{text}");
        assert!(text.contains("; intent"), "{text}");
    }

    #[test]
    fn development_edge_is_never_not_wallpaper_warn() {
        let workspace = Workspace::new([
            package(cargo("core"), ResolvesDependenciesAt::Install, vec![]),
            package(
                cargo("tool"),
                ResolvesDependenciesAt::Install,
                vec![edge(cargo("core"), DependencyKind::Development)],
            ),
        ])
        .expect("workspace");

        let (_, explain) = plan_and_explain(&workspace, vec![(cargo("core"), BumpLevel::Minor)]);
        let text = explain.to_string();
        assert!(text.contains("never"), "{text}");
        assert!(text.contains("skip (not runtime)"), "{text}");
    }

    #[test]
    fn zero_major_remap_is_named_on_bump() {
        let workspace = Workspace::new([package(
            cargo("core"),
            ResolvesDependenciesAt::Install,
            vec![],
        )])
        .expect("workspace");

        let (plan, explain) = plan_and_explain(&workspace, vec![(cargo("core"), BumpLevel::Major)]);
        let core = plan.get(&cargo("core")).expect("core");
        assert!(core.applied().was_remapped());
        assert_eq!(core.to(), &Version::new(0, 2, 0));

        let text = explain.to_string();
        assert!(text.contains("zero-major remapped major → minor"), "{text}");
    }

    #[test]
    fn rewrite_only_when_cascade_as_none() {
        let workspace = Workspace::new([
            package(cargo("core"), ResolvesDependenciesAt::Install, vec![]),
            package(
                cargo("cli"),
                ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget),
                vec![edge(cargo("core"), DependencyKind::Normal)],
            ),
        ])
        .expect("workspace");

        let plan = compose(
            &workspace,
            &intent(vec![(cargo("core"), BumpLevel::Patch)]),
            |_| Versioning::ZeroMajor,
            CascadeAs::None,
            |_, edge| Some(edge.range.clone()),
            |id| {
                workspace
                    .get(id)
                    .expect("package in workspace")
                    .version()
                    .clone()
            },
        )
        .expect("plan");
        assert!(plan.get(&cargo("cli")).is_none());

        let explain = explain_plan(&workspace, &plan, CascadeAs::None, |_, edge| {
            Some(edge.range.clone())
        });
        assert_eq!(
            edge_actions(&explain)
                .into_iter()
                .find(|(_, d, _)| **d == cargo("cli"))
                .expect("edge")
                .2,
            &EdgeAction::RewriteOnly
        );
        assert!(explain.to_string().contains("rewrite only"));
    }

    #[test]
    fn no_published_range_is_stated() {
        let workspace = Workspace::new([
            package(cargo("core"), ResolvesDependenciesAt::Install, vec![]),
            package(
                cargo("lib"),
                ResolvesDependenciesAt::Install,
                vec![edge(cargo("core"), DependencyKind::Normal)],
            ),
        ])
        .expect("workspace");

        let plan = compose(
            &workspace,
            &intent(vec![(cargo("core"), BumpLevel::Minor)]),
            |_| Versioning::ZeroMajor,
            CascadeAs::Patch,
            |_, _| None,
            |id| {
                workspace
                    .get(id)
                    .expect("package in workspace")
                    .version()
                    .clone()
            },
        )
        .expect("plan");
        assert!(plan.get(&cargo("lib")).is_none());

        let explain = explain_plan(&workspace, &plan, CascadeAs::Patch, |_, _| None);
        assert_eq!(
            edge_actions(&explain)[0].2,
            &EdgeAction::SkipNoPublishedRange
        );
        assert!(explain.to_string().contains("skip (no published range)"));
    }

    #[test]
    fn path_linked_published_range_always_cascades() {
        let workspace = Workspace::new([
            package(cargo("core"), ResolvesDependenciesAt::Install, vec![]),
            package(
                cargo("lib"),
                ResolvesDependenciesAt::Install,
                vec![edge(cargo("core"), DependencyKind::Normal)],
            ),
        ])
        .expect("workspace");

        let plan = compose(
            &workspace,
            &intent(vec![(cargo("core"), BumpLevel::Patch)]),
            |_| Versioning::ZeroMajor,
            CascadeAs::Patch,
            |_, _| Some(DeclaredRange::PathLinked),
            |id| {
                workspace
                    .get(id)
                    .expect("package in workspace")
                    .version()
                    .clone()
            },
        )
        .expect("plan");
        assert!(plan.get(&cargo("lib")).is_some());

        let explain = explain_plan(&workspace, &plan, CascadeAs::Patch, |_, _| {
            Some(DeclaredRange::PathLinked)
        });
        match edge_between(&explain, &cargo("core"), &cargo("lib")) {
            ExplainEntry::Edge {
                decision,
                action,
                published_range,
                ..
            } => {
                assert_eq!(*decision, CascadeDecision::Always);
                assert_eq!(*action, EdgeAction::Cascade(BumpLevel::Patch));
                assert_eq!(*published_range, Some(DeclaredRange::PathLinked));
            }
            ExplainEntry::Bump { .. } => panic!("expected edge"),
        }
        let text = explain.to_string();
        assert!(text.contains("always"), "{text}");
        assert!(text.contains("path-linked (no version)"), "{text}");
    }

    #[test]
    fn library_cascades_when_published_range_excludes_new_version() {
        let workspace = Workspace::new([
            package(cargo("core"), ResolvesDependenciesAt::Install, vec![]),
            package(
                cargo("lib"),
                ResolvesDependenciesAt::Install,
                vec![edge(cargo("core"), DependencyKind::Normal)],
            ),
        ])
        .expect("workspace");

        let (plan, explain) = plan_and_explain(&workspace, vec![(cargo("core"), BumpLevel::Minor)]);
        assert!(plan.get(&cargo("lib")).is_some());

        match edge_between(&explain, &cargo("core"), &cargo("lib")) {
            ExplainEntry::Edge {
                decision, action, ..
            } => {
                assert_eq!(*decision, CascadeDecision::IfRangeUnsatisfied);
                assert_eq!(*action, EdgeAction::Cascade(BumpLevel::Patch));
            }
            ExplainEntry::Bump { .. } => panic!("expected edge"),
        }
        let text = explain.to_string();
        assert!(text.contains("if-range-unsatisfied"), "{text}");
        assert!(text.contains("cascade patch"), "{text}");
    }

    #[test]
    fn build_time_always_does_not_call_published_range_of() {
        let workspace = Workspace::new([
            package(cargo("core"), ResolvesDependenciesAt::Install, vec![]),
            package(
                cargo("cli"),
                ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget),
                vec![edge(cargo("core"), DependencyKind::Normal)],
            ),
        ])
        .expect("workspace");

        let plan = compose(
            &workspace,
            &intent(vec![(cargo("core"), BumpLevel::Patch)]),
            |_| Versioning::ZeroMajor,
            CascadeAs::Patch,
            |_, _| panic!("build-time Always must not look up published range"),
            |id| {
                workspace
                    .get(id)
                    .expect("package in workspace")
                    .version()
                    .clone()
            },
        )
        .expect("plan");

        let explain = explain_plan(&workspace, &plan, CascadeAs::Patch, |_, _| {
            panic!("build-time Always must not look up published range")
        });
        match edge_between(&explain, &cargo("core"), &cargo("cli")) {
            ExplainEntry::Edge {
                decision,
                published_range,
                action,
                ..
            } => {
                assert_eq!(*decision, CascadeDecision::Always);
                assert!(published_range.is_none());
                assert_eq!(*action, EdgeAction::Cascade(BumpLevel::Patch));
            }
            ExplainEntry::Bump { .. } => panic!("expected edge"),
        }
    }

    #[test]
    fn working_tree_path_linked_uses_published_plain_for_gate() {
        let mut dep = edge(cargo("core"), DependencyKind::Normal);
        dep.range = DeclaredRange::PathLinked;
        let workspace = Workspace::new([
            package(cargo("core"), ResolvesDependenciesAt::Install, vec![]),
            package(cargo("lib"), ResolvesDependenciesAt::Install, vec![dep]),
        ])
        .expect("workspace");

        let published =
            DeclaredRange::Plain(crate::plan::Bounds::from_cargo_text("^0.1.3").expect("range"));
        let plan = compose(
            &workspace,
            &intent(vec![(cargo("core"), BumpLevel::Patch)]),
            |_| Versioning::ZeroMajor,
            CascadeAs::Patch,
            |_, _| Some(published.clone()),
            |id| {
                workspace
                    .get(id)
                    .expect("package in workspace")
                    .version()
                    .clone()
            },
        )
        .expect("plan");
        assert!(plan.get(&cargo("lib")).is_none());

        let explain = explain_plan(&workspace, &plan, CascadeAs::Patch, |_, _| {
            Some(published.clone())
        });
        match edge_between(&explain, &cargo("core"), &cargo("lib")) {
            ExplainEntry::Edge {
                decision, action, ..
            } => {
                assert_eq!(*decision, CascadeDecision::IfRangeUnsatisfied);
                assert_eq!(*action, EdgeAction::SkipRangeSatisfied);
            }
            ExplainEntry::Bump { .. } => panic!("expected edge"),
        }
    }

    #[test]
    fn catalog_display_is_protocol_token_with_bounds_beside() {
        use crate::plan::bounds::Bounds;
        use crate::plan::workspace::Tracking;

        let unnamed = DeclaredRange::Catalog {
            name: None,
            bounds: Bounds::from_npm_text("1.5.0").expect("bounds"),
        };
        let named = DeclaredRange::Catalog {
            name: Some(String::from("default")),
            bounds: Bounds::from_npm_text("1.5.0").expect("bounds"),
        };
        assert_eq!(unnamed.to_string(), "catalog:");
        assert_eq!(named.to_string(), "catalog:default");
        assert_eq!(Tracking::Exact.to_string(), "*");
        assert_eq!(Tracking::Tilde.to_string(), "~");
        assert_eq!(Tracking::Caret.to_string(), "^");
        assert_eq!(
            DeclaredRange::Workspace(Bounds::from_cargo_text("^1.5.0").expect("bounds"))
                .to_string(),
            "workspace:^1.5.0"
        );
        assert_eq!(
            DeclaredRange::WorkspaceTracking(Tracking::Exact).to_string(),
            "workspace:*"
        );

        let unnamed_text = ExplainEntry::Edge {
            trigger: cargo("core"),
            dependent: cargo("lib"),
            kind: DependencyKind::Normal,
            declared_as: String::from("core"),
            target: None,
            decision: CascadeDecision::IfRangeUnsatisfied,
            published_range: Some(unnamed),
            action: EdgeAction::SkipRangeSatisfied,
        }
        .to_string();
        assert!(
            unnamed_text.contains("published catalog: (= 1.5.0)"),
            "{unnamed_text}"
        );

        let named_text = ExplainEntry::Edge {
            trigger: cargo("core"),
            dependent: cargo("lib"),
            kind: DependencyKind::Normal,
            declared_as: String::from("core"),
            target: None,
            decision: CascadeDecision::IfRangeUnsatisfied,
            published_range: Some(named),
            action: EdgeAction::SkipRangeSatisfied,
        }
        .to_string();
        assert!(
            named_text.contains("published catalog:default (= 1.5.0)"),
            "{named_text}"
        );
    }

    #[test]
    fn self_edge_is_omitted_from_explain() {
        let id = cargo("core");
        let workspace = Workspace::new([package(
            id.clone(),
            ResolvesDependenciesAt::Install,
            vec![edge(id.clone(), DependencyKind::Normal)],
        )])
        .expect("workspace");

        let (_, explain) = plan_and_explain(&workspace, vec![(id, BumpLevel::Patch)]);
        assert!(
            edge_actions(&explain).is_empty(),
            "self-edges must not appear: {explain}"
        );
    }

    #[test]
    fn multi_edge_entries_carry_manifest_identity() {
        let mut targeted = edge(cargo("core"), DependencyKind::Normal);
        targeted.target = Some(String::from("cfg(windows)"));
        let workspace = Workspace::new([
            package(cargo("core"), ResolvesDependenciesAt::Install, vec![]),
            package(
                cargo("plugin"),
                ResolvesDependenciesAt::Install,
                vec![targeted, edge(cargo("core"), DependencyKind::Development)],
            ),
        ])
        .expect("workspace");

        let (_, explain) = plan_and_explain(&workspace, vec![(cargo("core"), BumpLevel::Patch)]);
        let edges: Vec<_> = explain
            .entries()
            .iter()
            .filter_map(|e| match e {
                ExplainEntry::Edge {
                    kind,
                    target,
                    action,
                    ..
                } => Some((*kind, target.as_deref(), action)),
                ExplainEntry::Bump { .. } => None,
            })
            .collect();
        assert_eq!(edges.len(), 2);
        assert!(edges.iter().any(|(k, t, a)| {
            *k == DependencyKind::Normal
                && *t == Some("cfg(windows)")
                && **a == EdgeAction::SkipRangeSatisfied
        }));
        assert!(edges.iter().any(|(k, t, a)| {
            *k == DependencyKind::Development && t.is_none() && **a == EdgeAction::SkipNotRuntime
        }));
        let text = explain.to_string();
        assert!(
            text.contains("normal as core, target cfg(windows)"),
            "{text}"
        );
        assert!(text.contains("development as core"), "{text}");
    }
}
