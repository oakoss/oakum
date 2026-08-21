//! Compose aggregated intent and a workspace into a release plan.
//!
//! Starts from packages named in bump-file intent, walks runtime dependents
//! through [`super::cascade::cascading_dependents`], and folds cascade bumps
//! with highest-wins against direct intent. Pure: no I/O (ADR-0002 / ADR-0024).

use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::vec::Vec;
use core::fmt;

use semver::Version;

use super::aggregate::AggregatedBump;
use super::bump::{apply_bump, AppliedBump, BumpError, BumpLevel, Versioning};
use super::cascade::{cascading_dependents, CascadeAs};
use super::workspace::{DeclaredRange, Dependency, Package, PackageId, Workspace};

/// Why a package appears in the plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChangeSource {
    Intent,
    Cascade { trigger: PackageId },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedChange {
    id: PackageId,
    from: Version,
    to: Version,
    applied: AppliedBump,
    source: ChangeSource,
}

impl PlannedChange {
    #[must_use]
    pub const fn id(&self) -> &PackageId {
        &self.id
    }

    #[must_use]
    pub const fn from(&self) -> &Version {
        &self.from
    }

    #[must_use]
    pub const fn to(&self) -> &Version {
        &self.to
    }

    #[must_use]
    pub const fn applied(&self) -> AppliedBump {
        self.applied
    }

    #[must_use]
    pub const fn source(&self) -> &ChangeSource {
        &self.source
    }
}

/// Walk-time plan entry: public fields plus retract bookkeeping.
#[derive(Clone, Debug, PartialEq, Eq)]
struct WorkingChange {
    id: PackageId,
    from: Version,
    to: Version,
    applied: AppliedBump,
    source: ChangeSource,
    /// Floor from bump-file intent; retained when a cascade later raises the bump.
    intent_level: Option<BumpLevel>,
    /// Highest cascade bump and its trigger. Cleared when that trigger's intermediate `to` is retracted.
    cascade_boost: Option<(BumpLevel, PackageId)>,
}

impl WorkingChange {
    fn into_planned(self) -> PlannedChange {
        PlannedChange {
            id: self.id,
            from: self.from,
            to: self.to,
            applied: self.applied,
            source: self.source,
        }
    }
}

/// Planned bumps in stable [`PackageId`] order for fixtures.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    changes: BTreeMap<PackageId, PlannedChange>,
}

impl Plan {
    #[must_use]
    pub fn changes(&self) -> &BTreeMap<PackageId, PlannedChange> {
        &self.changes
    }

    #[must_use]
    pub fn get(&self, id: &PackageId) -> Option<&PlannedChange> {
        self.changes.get(id)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.changes.len()
    }
}

/// Why [`compose`] could not produce a plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposeError {
    UnknownPackage(PackageId),
    Bump(BumpError),
}

impl fmt::Display for ComposeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownPackage(id) => write!(f, "unknown package in intent: {id}"),
            Self::Bump(err) => write!(f, "bump failed: {err}"),
        }
    }
}

impl core::error::Error for ComposeError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Bump(err) => Some(err),
            Self::UnknownPackage(_) => None,
        }
    }
}

impl From<BumpError> for ComposeError {
    fn from(value: BumpError) -> Self {
        Self::Bump(value)
    }
}

/// Walk intent through the cascade rules into a [`Plan`].
///
/// `version_at_tag` supplies each package's version as of the last reachable
/// tag (ADR-0014). `versioning` supplies each package's versioning policy
/// (ADR-0022). `published_range_of` supplies each install-time edge's
/// published declaration (or `None` when the edge did not exist at that tag).
///
/// Cascaded dependents take [`CascadeAs`]'s bump level (default patch). When
/// that is [`CascadeAs::None`], the edge fires for rewrite only and the
/// dependent is omitted from the plan.
///
/// The walk uses the working-tree dependency graph. Edges removed since the
/// last tag are not visible here; discovery must feed a tagged graph (or an
/// equivalent dependent set) before those can cascade.
///
/// # Errors
///
/// Returns [`ComposeError::UnknownPackage`] when intent names a package absent
/// from `workspace`, or [`ComposeError::Bump`] on version overflow.
pub fn compose<R, V, Ver>(
    workspace: &Workspace,
    intent: &BTreeMap<PackageId, AggregatedBump>,
    mut versioning: Ver,
    cascade_as: CascadeAs,
    mut published_range_of: R,
    mut version_at_tag: V,
) -> Result<Plan, ComposeError>
where
    R: FnMut(&Package, &Dependency) -> Option<DeclaredRange>,
    V: FnMut(&PackageId) -> Version,
    Ver: FnMut(&PackageId) -> Versioning,
{
    let mut changes: BTreeMap<PackageId, WorkingChange> = BTreeMap::new();
    let mut queue: VecDeque<(PackageId, Version, Version)> = VecDeque::new();

    for (id, aggregated) in intent {
        if workspace.get(id).is_none() {
            return Err(ComposeError::UnknownPackage(id.clone()));
        }
        schedule(
            &mut changes,
            &mut queue,
            id,
            aggregated.level(),
            ChangeSource::Intent,
            &mut versioning,
            &mut version_at_tag,
        )?;
    }

    while let Some((trigger_id, from, to)) = queue.pop_front() {
        // A later higher bump leaves the earlier `(from, to)` in the queue;
        // walking that intermediate `to` can false-positive the range gate.
        if changes
            .get(&trigger_id)
            .is_none_or(|change| change.to != to)
        {
            continue;
        }

        let dependents: Vec<PackageId> =
            cascading_dependents(workspace, &trigger_id, &from, &to, &mut published_range_of)
                .map(|package| package.id().clone())
                .collect();

        let Some(level) = cascade_as.bump_level() else {
            continue;
        };

        for dep_id in dependents {
            schedule(
                &mut changes,
                &mut queue,
                &dep_id,
                level,
                ChangeSource::Cascade {
                    trigger: trigger_id.clone(),
                },
                &mut versioning,
                &mut version_at_tag,
            )?;
        }
    }

    Ok(Plan {
        changes: changes
            .into_iter()
            .map(|(id, change)| (id, change.into_planned()))
            .collect(),
    })
}

fn schedule<V, Ver>(
    changes: &mut BTreeMap<PackageId, WorkingChange>,
    queue: &mut VecDeque<(PackageId, Version, Version)>,
    id: &PackageId,
    requested: BumpLevel,
    source: ChangeSource,
    versioning: &mut Ver,
    version_at_tag: &mut V,
) -> Result<(), ComposeError>
where
    V: FnMut(&PackageId) -> Version,
    Ver: FnMut(&PackageId) -> Versioning,
{
    let previous = changes.get(id).cloned();
    let from = match &previous {
        Some(existing) => existing.from.clone(),
        None => version_at_tag(id),
    };

    let intent_floor = match &source {
        ChangeSource::Intent => Some(requested),
        ChangeSource::Cascade { .. } => None,
    };
    let intent_level = match &previous {
        Some(existing) => existing.intent_level.or(intent_floor),
        None => intent_floor,
    };

    let incoming_boost = match &source {
        ChangeSource::Cascade { trigger } => Some((requested, trigger.clone())),
        ChangeSource::Intent => None,
    };

    let (requested, source, cascade_boost) = match &previous {
        None => (requested, source, incoming_boost),
        Some(existing) => {
            let merged = if requested > existing.applied.requested() {
                requested
            } else {
                existing.applied.requested()
            };
            if merged == existing.applied.requested() {
                // Keep earlier attribution; a later equal cascade must not overwrite Intent or trigger.
                return Ok(());
            }
            let source = match (&existing.source, &source) {
                (ChangeSource::Intent, _) | (_, ChangeSource::Intent) => ChangeSource::Intent,
                (ChangeSource::Cascade { .. }, ChangeSource::Cascade { .. }) => source,
            };
            let cascade_boost = match incoming_boost {
                Some(boost) => Some(boost),
                None => existing.cascade_boost.clone(),
            };
            (merged, source, cascade_boost)
        }
    };

    let policy = versioning(id);
    let (to, applied) = apply_bump(&from, requested, policy)?;
    let to_changed = previous.as_ref().is_none_or(|existing| existing.to != to);
    if to_changed && previous.is_some() {
        // Intermediate `to` may false-positive or false-negative the range gate; drop and re-walk.
        retract_cascades_from(changes, queue, id, versioning)?;
    }
    changes.insert(
        id.clone(),
        WorkingChange {
            id: id.clone(),
            from: from.clone(),
            to: to.clone(),
            applied,
            source,
            intent_level,
            cascade_boost,
        },
    );
    if to_changed {
        queue.push_back((id.clone(), from, to));
    }
    Ok(())
}

fn cascade_descendants(
    changes: &BTreeMap<PackageId, WorkingChange>,
    root: &PackageId,
) -> BTreeSet<PackageId> {
    let mut remove = BTreeSet::new();
    let mut growing = true;
    while growing {
        growing = false;
        for (id, change) in changes {
            if remove.contains(id) {
                continue;
            }
            if let ChangeSource::Cascade { trigger: t } = &change.source {
                if t == root || remove.contains(t) {
                    remove.insert(id.clone());
                    growing = true;
                }
            }
        }
    }
    remove
}

/// Undo cascade effects of an intermediate `to` before re-walking the raise.
///
/// Removes pure [`ChangeSource::Cascade`] descendants of `trigger`, clears
/// cascade boosts attributed to that trigger (rolling Intent packages back to
/// their intent floor), and re-queues survivors so independent cascade paths
/// can restore dependents that still need a bump.
fn retract_cascades_from<Ver>(
    changes: &mut BTreeMap<PackageId, WorkingChange>,
    queue: &mut VecDeque<(PackageId, Version, Version)>,
    trigger: &PackageId,
    versioning: &mut Ver,
) -> Result<(), ComposeError>
where
    Ver: FnMut(&PackageId) -> Versioning,
{
    let mut remove = cascade_descendants(changes, trigger);

    // Intent packages boosted by this trigger (source merge hid the edge), or by a package about to be removed.
    let mut clear_boost = BTreeSet::new();
    let mut growing = true;
    while growing {
        growing = false;
        let changes_ref: &BTreeMap<PackageId, WorkingChange> = changes;
        for (id, change) in changes_ref {
            if remove.contains(id) || clear_boost.contains(id) {
                continue;
            }
            if let Some((_, boost_trigger)) = &change.cascade_boost {
                if boost_trigger == trigger
                    || remove.contains(boost_trigger)
                    || clear_boost.contains(boost_trigger)
                {
                    clear_boost.insert(id.clone());
                    growing = true;
                }
            }
        }
        for id in clear_boost.clone() {
            let extra = cascade_descendants(changes, &id);
            if !extra.is_subset(&remove) {
                remove.extend(extra);
                growing = true;
            }
        }
    }

    if remove.is_empty() && clear_boost.is_empty() {
        return Ok(());
    }

    for id in &clear_boost {
        let Some(change) = changes.get(id).cloned() else {
            continue;
        };
        let Some(floor) = change.intent_level else {
            // Pure cascade with a recorded boost but no intent: drop it.
            remove.insert(id.clone());
            continue;
        };
        let policy = versioning(id);
        let (to, applied) = apply_bump(&change.from, floor, policy)?;
        changes.insert(
            id.clone(),
            WorkingChange {
                id: id.clone(),
                from: change.from.clone(),
                to,
                applied,
                source: ChangeSource::Intent,
                intent_level: Some(floor),
                cascade_boost: None,
            },
        );
    }

    let rewalk: Vec<(PackageId, Version, Version)> = changes
        .iter()
        .filter(|(id, _)| !remove.contains(*id))
        .map(|(id, change)| (id.clone(), change.from.clone(), change.to.clone()))
        .collect();
    for id in &remove {
        changes.remove(id);
    }
    queue.retain(|(id, _, _)| !remove.contains(id));
    for entry in rewalk {
        queue.push_back(entry);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use alloc::collections::BTreeMap;
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;

    use semver::Version;

    use super::*;
    use crate::plan::aggregate::{aggregate, BumpFile};
    use crate::plan::bump::BumpLevel;
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

    fn tag_versions(workspace: &Workspace) -> impl FnMut(&PackageId) -> Version + '_ {
        |id| {
            workspace
                .get(id)
                .expect("package in workspace")
                .version()
                .clone()
        }
    }

    #[test]
    fn binary_always_cascades_despite_satisfied_range() {
        let workspace = Workspace::new([
            package(cargo("core"), ResolvesDependenciesAt::Install, vec![]),
            package(
                cargo("cli"),
                ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget),
                vec![edge(cargo("core"), DependencyKind::Normal)],
            ),
        ])
        .expect("workspace");

        // Patch stays inside ^0.1.3; binary Always still fires (linesmith shape).
        let plan = compose(
            &workspace,
            &intent(vec![(cargo("core"), BumpLevel::Patch)]),
            |_| Versioning::ZeroMajor,
            CascadeAs::Patch,
            |_, edge| Some(edge.range.clone()),
            tag_versions(&workspace),
        )
        .expect("plan");

        assert!(plan.get(&cargo("core")).is_some());
        let cli = plan.get(&cargo("cli")).expect("cli cascades");
        assert_eq!(cli.applied().requested(), BumpLevel::Patch);
        assert_eq!(
            cli.source(),
            &ChangeSource::Cascade {
                trigger: cargo("core")
            }
        );
        assert_eq!(cli.to(), &Version::new(0, 1, 4));
    }

    #[test]
    fn library_skips_when_published_range_still_resolves() {
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
            |_, edge| Some(edge.range.clone()),
            tag_versions(&workspace),
        )
        .expect("plan");

        assert!(plan.get(&cargo("core")).is_some());
        assert!(
            plan.get(&cargo("lib")).is_none(),
            "^0.1.3 still admits 0.1.4"
        );
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

        let plan = compose(
            &workspace,
            &intent(vec![(cargo("core"), BumpLevel::Minor)]),
            |_| Versioning::ZeroMajor,
            CascadeAs::Patch,
            |_, edge| Some(edge.range.clone()),
            tag_versions(&workspace),
        )
        .expect("plan");

        let core = plan.get(&cargo("core")).expect("core");
        assert_eq!(core.to(), &Version::new(0, 2, 0));
        let lib = plan.get(&cargo("lib")).expect("lib cascades");
        assert_eq!(lib.applied().requested(), BumpLevel::Patch);
        assert_eq!(lib.to(), &Version::new(0, 1, 4));
    }

    #[test]
    fn development_edge_never_cascades() {
        let workspace = Workspace::new([
            package(cargo("core"), ResolvesDependenciesAt::Install, vec![]),
            package(
                cargo("tool"),
                ResolvesDependenciesAt::Install,
                vec![edge(cargo("core"), DependencyKind::Development)],
            ),
            package(
                cargo("cli"),
                ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget),
                vec![edge(cargo("core"), DependencyKind::Normal)],
            ),
        ])
        .expect("workspace");

        let plan = compose(
            &workspace,
            &intent(vec![(cargo("core"), BumpLevel::Minor)]),
            |_| Versioning::ZeroMajor,
            CascadeAs::Patch,
            |_, edge| Some(edge.range.clone()),
            tag_versions(&workspace),
        )
        .expect("plan");

        assert!(plan.get(&cargo("cli")).is_some());
        assert!(
            plan.get(&cargo("tool")).is_none(),
            "Development edges stay Never"
        );
    }

    #[test]
    fn unknown_intent_package_errors() {
        let workspace = Workspace::new([package(
            cargo("core"),
            ResolvesDependenciesAt::Install,
            vec![],
        )])
        .expect("workspace");

        let err = compose(
            &workspace,
            &intent(vec![(cargo("missing"), BumpLevel::Patch)]),
            |_| Versioning::ZeroMajor,
            CascadeAs::Patch,
            |_, edge| Some(edge.range.clone()),
            tag_versions(&workspace),
        )
        .expect_err("missing package");
        assert_eq!(err, ComposeError::UnknownPackage(cargo("missing")));
    }

    #[test]
    fn cascade_as_none_omits_dependent_bump() {
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
            tag_versions(&workspace),
        )
        .expect("plan");

        assert!(plan.get(&cargo("core")).is_some());
        assert!(plan.get(&cargo("cli")).is_none());
    }

    #[test]
    fn intent_highest_wins_over_cascade_patch() {
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
            &intent(vec![
                (cargo("core"), BumpLevel::Patch),
                (cargo("cli"), BumpLevel::Minor),
            ]),
            |_| Versioning::ZeroMajor,
            CascadeAs::Patch,
            |_, edge| Some(edge.range.clone()),
            tag_versions(&workspace),
        )
        .expect("plan");

        let cli = plan.get(&cargo("cli")).expect("cli");
        assert_eq!(cli.applied().requested(), BumpLevel::Minor);
        assert_eq!(cli.source(), &ChangeSource::Intent);
        assert_eq!(cli.to(), &Version::new(0, 2, 0));
    }

    #[test]
    fn multi_hop_always_chain() {
        let workspace = Workspace::new([
            package(cargo("core"), ResolvesDependenciesAt::Install, vec![]),
            package(
                cargo("mid"),
                ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget),
                vec![edge(cargo("core"), DependencyKind::Normal)],
            ),
            package(
                cargo("app"),
                ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget),
                vec![edge(cargo("mid"), DependencyKind::Normal)],
            ),
        ])
        .expect("workspace");

        let plan = compose(
            &workspace,
            &intent(vec![(cargo("core"), BumpLevel::Patch)]),
            |_| Versioning::ZeroMajor,
            CascadeAs::Patch,
            |_, edge| Some(edge.range.clone()),
            tag_versions(&workspace),
        )
        .expect("plan");

        let mid = plan.get(&cargo("mid")).expect("mid");
        assert_eq!(
            mid.source(),
            &ChangeSource::Cascade {
                trigger: cargo("core")
            }
        );
        let app = plan.get(&cargo("app")).expect("app");
        assert_eq!(
            app.source(),
            &ChangeSource::Cascade {
                trigger: cargo("mid")
            }
        );
    }

    #[test]
    fn diamond_schedules_app_once() {
        let workspace = Workspace::new([
            package(cargo("core"), ResolvesDependenciesAt::Install, vec![]),
            package(
                cargo("left"),
                ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget),
                vec![edge(cargo("core"), DependencyKind::Normal)],
            ),
            package(
                cargo("right"),
                ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget),
                vec![edge(cargo("core"), DependencyKind::Normal)],
            ),
            package(
                cargo("app"),
                ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget),
                vec![
                    edge(cargo("left"), DependencyKind::Normal),
                    edge(cargo("right"), DependencyKind::Normal),
                ],
            ),
        ])
        .expect("workspace");

        let plan = compose(
            &workspace,
            &intent(vec![(cargo("core"), BumpLevel::Patch)]),
            |_| Versioning::ZeroMajor,
            CascadeAs::Patch,
            |_, edge| Some(edge.range.clone()),
            tag_versions(&workspace),
        )
        .expect("plan");

        assert_eq!(plan.len(), 4);
        assert!(plan.get(&cargo("app")).is_some());
    }

    #[test]
    fn retract_rewalks_surviving_cascade_path() {
        // left raises to 0.2.0 (admits ^0.2.0); right stays at 0.1.4 (excludes
        // =0.1.3). App may be attributed to left first; after left retracts,
        // right must restore the cascade.
        let mut from_left = edge(cargo("aaa"), DependencyKind::Normal);
        from_left.range =
            DeclaredRange::Plain(crate::plan::Bounds::from_cargo_text("^0.2.0").expect("range"));
        let mut from_right = edge(cargo("right"), DependencyKind::Normal);
        from_right.range =
            DeclaredRange::Plain(crate::plan::Bounds::from_cargo_text("=0.1.3").expect("range"));
        let workspace = Workspace::new([
            package(cargo("core"), ResolvesDependenciesAt::Install, vec![]),
            package(
                cargo("aaa"),
                ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget),
                vec![edge(cargo("core"), DependencyKind::Normal)],
            ),
            package(
                cargo("right"),
                ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget),
                vec![edge(cargo("core"), DependencyKind::Normal)],
            ),
            package(
                cargo("app"),
                ResolvesDependenciesAt::Install,
                vec![from_left, from_right],
            ),
        ])
        .expect("workspace");

        let plan = compose(
            &workspace,
            &intent(vec![
                (cargo("core"), BumpLevel::Patch),
                (cargo("aaa"), BumpLevel::Patch),
                (cargo("right"), BumpLevel::Patch),
            ]),
            |_| Versioning::ZeroMajor,
            CascadeAs::Minor,
            |_, edge| Some(edge.range.clone()),
            tag_versions(&workspace),
        )
        .expect("plan");

        let app = plan.get(&cargo("app")).expect("right still requires app");
        assert_eq!(
            app.source(),
            &ChangeSource::Cascade {
                trigger: cargo("right")
            }
        );
    }

    #[test]
    fn cascade_as_minor_raises_and_skips_stale_queue_entry() {
        let mut leaf_edge = edge(cargo("aaa"), DependencyKind::Normal);
        leaf_edge.range =
            DeclaredRange::Plain(crate::plan::Bounds::from_cargo_text("^0.2.0").expect("range"));
        // `aaa` sorts before `core`, so Intent Patch is walked before core's
        // CascadeAs::Minor raise — the incomplete-fix order that stale-queue
        // discard alone cannot fix without retracting.
        let workspace = Workspace::new([
            package(cargo("core"), ResolvesDependenciesAt::Install, vec![]),
            package(
                cargo("aaa"),
                ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget),
                vec![edge(cargo("core"), DependencyKind::Normal)],
            ),
            package(
                cargo("leaf"),
                ResolvesDependenciesAt::Install,
                vec![leaf_edge],
            ),
        ])
        .expect("workspace");

        let plan = compose(
            &workspace,
            &intent(vec![
                (cargo("core"), BumpLevel::Patch),
                (cargo("aaa"), BumpLevel::Patch),
            ]),
            |_| Versioning::ZeroMajor,
            CascadeAs::Minor,
            |_, edge| Some(edge.range.clone()),
            tag_versions(&workspace),
        )
        .expect("plan");

        let mid = plan.get(&cargo("aaa")).expect("aaa");
        assert_eq!(mid.applied().requested(), BumpLevel::Minor);
        assert_eq!(mid.to(), &Version::new(0, 2, 0));
        assert_eq!(mid.source(), &ChangeSource::Intent);
        assert!(
            plan.get(&cargo("leaf")).is_none(),
            "final aaa 0.2.0 still admits ^0.2.0"
        );
    }

    #[test]
    fn retract_rolls_back_intent_elevated_by_intermediate_cascade() {
        let mut leaf_edge = edge(cargo("aaa"), DependencyKind::Normal);
        leaf_edge.range =
            DeclaredRange::Plain(crate::plan::Bounds::from_cargo_text("^0.2.0").expect("range"));
        let workspace = Workspace::new([
            package(cargo("core"), ResolvesDependenciesAt::Install, vec![]),
            package(
                cargo("aaa"),
                ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget),
                vec![edge(cargo("core"), DependencyKind::Normal)],
            ),
            package(
                cargo("leaf"),
                ResolvesDependenciesAt::Install,
                vec![leaf_edge],
            ),
        ])
        .expect("workspace");

        // Intent Patch on leaf is raised to Minor by aaa@0.1.4; after aaa
        // rises to 0.2.0 the range admits and leaf must return to Patch.
        let plan = compose(
            &workspace,
            &intent(vec![
                (cargo("core"), BumpLevel::Patch),
                (cargo("aaa"), BumpLevel::Patch),
                (cargo("leaf"), BumpLevel::Patch),
            ]),
            |_| Versioning::ZeroMajor,
            CascadeAs::Minor,
            |_, edge| Some(edge.range.clone()),
            tag_versions(&workspace),
        )
        .expect("plan");

        let leaf = plan.get(&cargo("leaf")).expect("leaf stays in plan");
        assert_eq!(leaf.source(), &ChangeSource::Intent);
        assert_eq!(leaf.applied().requested(), BumpLevel::Patch);
        assert_eq!(leaf.to(), &Version::new(0, 1, 4));
    }

    #[test]
    fn retract_clears_boost_transitively_through_intent() {
        // Crafted plan: aaa raised; mid Intent boosted by aaa; leaf Intent
        // boosted by mid. Retracting aaa must clear both boosts — compose
        // walk order can heal leaf before an assert, so call retract directly.
        let from = Version::new(0, 1, 3);
        let mut changes = BTreeMap::new();
        let (aaa_to, aaa_applied) =
            apply_bump(&from, BumpLevel::Minor, Versioning::ZeroMajor).expect("aaa");
        changes.insert(
            cargo("aaa"),
            WorkingChange {
                id: cargo("aaa"),
                from: from.clone(),
                to: aaa_to,
                applied: aaa_applied,
                source: ChangeSource::Intent,
                intent_level: Some(BumpLevel::Patch),
                cascade_boost: Some((BumpLevel::Minor, cargo("seed"))),
            },
        );
        let (mid_to, mid_applied) =
            apply_bump(&from, BumpLevel::Minor, Versioning::ZeroMajor).expect("mid");
        changes.insert(
            cargo("mid"),
            WorkingChange {
                id: cargo("mid"),
                from: from.clone(),
                to: mid_to,
                applied: mid_applied,
                source: ChangeSource::Intent,
                intent_level: Some(BumpLevel::Patch),
                cascade_boost: Some((BumpLevel::Minor, cargo("aaa"))),
            },
        );
        let (leaf_to, leaf_applied) =
            apply_bump(&from, BumpLevel::Minor, Versioning::ZeroMajor).expect("leaf");
        changes.insert(
            cargo("leaf"),
            WorkingChange {
                id: cargo("leaf"),
                from: from.clone(),
                to: leaf_to,
                applied: leaf_applied,
                source: ChangeSource::Intent,
                intent_level: Some(BumpLevel::Patch),
                cascade_boost: Some((BumpLevel::Minor, cargo("mid"))),
            },
        );

        let mut queue = alloc::collections::VecDeque::new();
        retract_cascades_from(&mut changes, &mut queue, &cargo("aaa"), &mut |_| {
            Versioning::ZeroMajor
        })
        .expect("retract");

        assert_eq!(
            changes.get(&cargo("mid")).expect("mid").applied.requested(),
            BumpLevel::Patch
        );
        assert_eq!(
            changes
                .get(&cargo("leaf"))
                .expect("leaf")
                .applied
                .requested(),
            BumpLevel::Patch,
            "leaf boost from mid must clear when mid is clear_boost'd"
        );
    }

    #[test]
    fn raised_trigger_still_cascades_when_final_version_excludes_range() {
        let workspace = Workspace::new([
            package(cargo("core"), ResolvesDependenciesAt::Install, vec![]),
            package(
                cargo("aaa"),
                ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget),
                vec![edge(cargo("core"), DependencyKind::Normal)],
            ),
            package(
                cargo("leaf"),
                ResolvesDependenciesAt::Install,
                vec![edge(cargo("aaa"), DependencyKind::Normal)],
            ),
        ])
        .expect("workspace");

        // ^0.1.3 admits patch 0.1.4 but excludes minor 0.2.0 — after the raise,
        // leaf must cascade (false-negative if only the stale entry was walked).
        let plan = compose(
            &workspace,
            &intent(vec![
                (cargo("core"), BumpLevel::Patch),
                (cargo("aaa"), BumpLevel::Patch),
            ]),
            |_| Versioning::ZeroMajor,
            CascadeAs::Minor,
            |_, edge| Some(edge.range.clone()),
            tag_versions(&workspace),
        )
        .expect("plan");

        assert!(plan.get(&cargo("leaf")).is_some());
    }

    #[test]
    fn equal_level_cascade_keeps_intent_source() {
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
            &intent(vec![
                (cargo("core"), BumpLevel::Patch),
                (cargo("cli"), BumpLevel::Patch),
            ]),
            |_| Versioning::ZeroMajor,
            CascadeAs::Patch,
            |_, edge| Some(edge.range.clone()),
            tag_versions(&workspace),
        )
        .expect("plan");

        let cli = plan.get(&cargo("cli")).expect("cli");
        assert_eq!(cli.source(), &ChangeSource::Intent);
        assert_eq!(cli.to(), &Version::new(0, 1, 4));
    }

    #[test]
    fn version_at_tag_is_plan_from() {
        let workspace = Workspace::new([Package::new(
            cargo("core"),
            Version::new(0, 1, 4),
            ResolvesDependenciesAt::Install,
            true,
            vec![],
        )])
        .expect("workspace");

        let plan = compose(
            &workspace,
            &intent(vec![(cargo("core"), BumpLevel::Patch)]),
            |_| Versioning::ZeroMajor,
            CascadeAs::Patch,
            |_, edge| Some(edge.range.clone()),
            |_| Version::new(0, 1, 3),
        )
        .expect("plan");

        let core = plan.get(&cargo("core")).expect("core");
        assert_eq!(core.from(), &Version::new(0, 1, 3));
        assert_eq!(core.to(), &Version::new(0, 1, 4));
    }

    #[test]
    fn bump_overflow_surfaces_as_compose_error() {
        let workspace = Workspace::new([package(
            cargo("core"),
            ResolvesDependenciesAt::Install,
            vec![],
        )])
        .expect("workspace");

        let err = compose(
            &workspace,
            &intent(vec![(cargo("core"), BumpLevel::Patch)]),
            |_| Versioning::ZeroMajor,
            CascadeAs::Patch,
            |_, edge| Some(edge.range.clone()),
            |_| Version::new(0, 0, u64::MAX),
        )
        .expect_err("overflow");
        assert_eq!(err, ComposeError::Bump(BumpError::Overflow));
    }
}
