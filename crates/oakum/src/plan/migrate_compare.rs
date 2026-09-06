//! Pure migrate plan comparison (`okm-45t.3`).
//!
//! Classifies before/after fingerprints: equality, expected knope pre-1.0
//! feature fallout (patch vs minor), or unexpected drift. No I/O.

use alloc::collections::BTreeMap;
use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;

use semver::Version;

use super::aggregate::{aggregate, BumpFile};
use super::bump::{apply_bump, BumpLevel, Versioning};
use super::compose::{ChangeSource, Plan, PlannedChange};
use super::workspace::{PackageId, Workspace};

/// `(from, to)` versions for each planned package.
pub type PlanFingerprint = BTreeMap<PackageId, (Version, Version)>;

#[must_use]
pub fn plan_fingerprint(plan: &Plan) -> PlanFingerprint {
    plan.changes()
        .iter()
        .map(|(id, change)| (id.clone(), (change.from().clone(), change.to().clone())))
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageDiff {
    id: PackageId,
    before: Option<(Version, Version)>,
    after: Option<(Version, Version)>,
}

impl PackageDiff {
    #[must_use]
    pub const fn id(&self) -> &PackageId {
        &self.id
    }

    #[must_use]
    pub const fn before(&self) -> Option<&(Version, Version)> {
        self.before.as_ref()
    }

    #[must_use]
    pub const fn after(&self) -> Option<&(Version, Version)> {
        self.after.as_ref()
    }
}

/// Opaque: construct only via [`compare_plans`]. Classification lives in the
/// private inner arm so dependents cannot forge empty or reclassified values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanComparison {
    inner: PlanComparisonInner,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PlanComparisonInner {
    Equal,
    ExpectedKnopeFeature(Vec<PackageDiff>),
    Unexpected {
        expected: Vec<PackageDiff>,
        unexpected: Vec<PackageDiff>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnexpectedDiffs<'a> {
    pub expected: &'a [PackageDiff],
    pub unexpected: &'a [PackageDiff],
}

impl PlanComparison {
    /// Crate-local helper for tests; dependents use [`compare_plans`].
    #[must_use]
    pub(crate) const fn equal() -> Self {
        Self {
            inner: PlanComparisonInner::Equal,
        }
    }

    /// True when migrate should hard-fail on plan drift.
    #[must_use]
    pub const fn is_unexpected(&self) -> bool {
        matches!(self.inner, PlanComparisonInner::Unexpected { .. })
    }

    /// Knope feature fallout rows when that is the only drift.
    #[must_use]
    pub fn expected_knope_feature(&self) -> Option<&[PackageDiff]> {
        match &self.inner {
            PlanComparisonInner::ExpectedKnopeFeature(diffs) => Some(diffs.as_slice()),
            _ => None,
        }
    }

    /// Unexpected drift, with any expected knope rows alongside.
    #[must_use]
    pub fn unexpected(&self) -> Option<UnexpectedDiffs<'_>> {
        match &self.inner {
            PlanComparisonInner::Unexpected {
                expected,
                unexpected,
            } => Some(UnexpectedDiffs {
                expected: expected.as_slice(),
                unexpected: unexpected.as_slice(),
            }),
            _ => None,
        }
    }
}

/// `before_plan` is required for cascade walks on the simulated-before path;
/// source-tool before fingerprints pass `None`.
#[must_use]
pub fn compare_plans(
    workspace: &Workspace,
    files: &[BumpFile],
    knope: bool,
    before: &PlanFingerprint,
    before_plan: Option<&Plan>,
    after: &Plan,
) -> PlanComparison {
    let after_fp = plan_fingerprint(after);
    if before == &after_fp {
        return PlanComparison::equal();
    }
    let features = knope_feature_ids(workspace, files);
    let mut ids: BTreeSet<PackageId> = before.keys().cloned().collect();
    ids.extend(after_fp.keys().cloned());
    let mut expected = Vec::new();
    let mut unexpected = Vec::new();
    for id in ids {
        let before_v = before.get(&id).cloned();
        let after_v = after_fp.get(&id).cloned();
        if before_v == after_v {
            continue;
        }
        let diff = PackageDiff {
            id: id.clone(),
            before: before_v,
            after: after_v,
        };
        if knope_feature_fallout(knope, &features, before_plan, after, before, &after_fp, &id) {
            expected.push(diff);
        } else {
            unexpected.push(diff);
        }
    }
    if unexpected.is_empty() {
        PlanComparison {
            inner: PlanComparisonInner::ExpectedKnopeFeature(expected),
        }
    } else {
        PlanComparison {
            inner: PlanComparisonInner::Unexpected {
                expected,
                unexpected,
            },
        }
    }
}

#[must_use]
pub fn format_versions(versions: Option<&(Version, Version)>) -> String {
    match versions {
        Some((from, to)) => alloc::format!("{from} → {to}"),
        None => String::from("absent"),
    }
}

fn bumped(from: &Version, level: BumpLevel) -> Option<Version> {
    apply_bump(from, level, Versioning::ZeroMajor)
        .ok()
        .map(|(next, _)| next)
}

fn is_knope_feature_versions(
    before: Option<&(Version, Version)>,
    after: Option<&(Version, Version)>,
) -> bool {
    let Some((before_from, before_to)) = before else {
        return false;
    };
    let Some((after_from, after_to)) = after else {
        return false;
    };
    if before_from != after_from {
        return false;
    }
    let Some(patch) = bumped(before_from, BumpLevel::Patch) else {
        return false;
    };
    let Some(minor) = bumped(before_from, BumpLevel::Minor) else {
        return false;
    };
    before_to == &patch && after_to == &minor
}

fn knope_feature_fallout(
    knope: bool,
    features: &BTreeSet<PackageId>,
    before: Option<&Plan>,
    after: &Plan,
    before_fp: &PlanFingerprint,
    after_fp: &PlanFingerprint,
    id: &PackageId,
) -> bool {
    if !knope {
        return false;
    }
    let canonical: BTreeSet<PackageId> = features
        .iter()
        .filter(|feature| {
            is_knope_feature_versions(before_fp.get(*feature), after_fp.get(*feature))
        })
        .cloned()
        .collect();
    if canonical.contains(id) {
        return true;
    }
    cascaded_from_feature(after, &canonical, id)
        || before.is_some_and(|before| cascaded_from_feature(before, &canonical, id))
}

fn cascaded_from_feature(plan: &Plan, features: &BTreeSet<PackageId>, id: &PackageId) -> bool {
    let mut current = id.clone();
    let mut seen = BTreeSet::new();
    while seen.insert(current.clone()) {
        match plan.get(&current).map(PlannedChange::source) {
            Some(ChangeSource::Cascade { trigger }) => {
                if features.contains(trigger) {
                    return true;
                }
                current = trigger.clone();
            }
            _ => return false,
        }
    }
    false
}

fn knope_feature_ids(workspace: &Workspace, files: &[BumpFile]) -> BTreeSet<PackageId> {
    aggregate(files.to_vec())
        .into_iter()
        .filter(|(id, bump)| {
            bump.level() == BumpLevel::Minor
                && workspace
                    .get(id)
                    .is_some_and(|pkg| pkg.version().major == 0)
        })
        .map(|(id, _)| id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{
        compose, DeclaredRange, Dependency, DependencyKind, Ecosystem, Package, PackageId,
        ResolvesDependenciesAt, Workspace,
    };
    use alloc::vec;
    use semver::Version;

    fn cargo_pkg(name: &str, version: Version) -> Package {
        Package::new(
            PackageId::new(Ecosystem::Cargo, name),
            version,
            ResolvesDependenciesAt::Install,
            true,
            vec![],
        )
    }

    fn edge(on: PackageId) -> Dependency {
        let declared_as = on.name.clone();
        Dependency {
            on,
            kind: DependencyKind::Normal,
            declared_as,
            target: None,
            range: DeclaredRange::PathLinked,
        }
    }

    fn workspace(packages: Vec<Package>) -> Workspace {
        Workspace::new(packages).expect("workspace")
    }

    fn bump(id: &str, level: BumpLevel) -> BumpFile {
        BumpFile {
            id: alloc::format!("{id}.md"),
            entries: vec![(PackageId::new(Ecosystem::Cargo, id), level)],
            note: String::new(),
        }
    }

    fn oakum_plan(ws: &Workspace, files: Vec<BumpFile>) -> Plan {
        let intent = aggregate(files);
        compose(
            ws,
            &intent,
            |_| Versioning::ZeroMajor,
            super::super::cascade::CascadeAs::Patch,
            |_, dep| Some(dep.range.clone()),
            |id| ws.get(id).expect("pkg").version().clone(),
        )
        .expect("compose")
    }

    #[test]
    fn equal_fingerprints() {
        let ws = workspace(vec![cargo_pkg("core", Version::new(0, 1, 0))]);
        let files = vec![bump("core", BumpLevel::Patch)];
        let plan = oakum_plan(&ws, files.clone());
        let fp = plan_fingerprint(&plan);
        assert_eq!(
            compare_plans(&ws, &files, false, &fp, Some(&plan), &plan),
            PlanComparison::equal()
        );
    }

    #[test]
    fn knope_feature_patch_vs_minor_is_expected() {
        let ws = workspace(vec![cargo_pkg("core", Version::new(0, 1, 0))]);
        let files = vec![bump("core", BumpLevel::Minor)];
        let after = oakum_plan(&ws, files.clone());
        let mut before = plan_fingerprint(&after);
        let core = PackageId::new(Ecosystem::Cargo, "core");
        before.insert(core.clone(), (Version::new(0, 1, 0), Version::new(0, 1, 1)));
        let cmp = compare_plans(&ws, &files, true, &before, None, &after);
        let diffs = cmp
            .expected_knope_feature()
            .expect("expected knope fallout");
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].id(), &core);
    }

    #[test]
    fn unexpected_diff_without_knope_flag() {
        let ws = workspace(vec![cargo_pkg("core", Version::new(0, 1, 0))]);
        let files = vec![bump("core", BumpLevel::Minor)];
        let after = oakum_plan(&ws, files.clone());
        let mut before = plan_fingerprint(&after);
        let core = PackageId::new(Ecosystem::Cargo, "core");
        before.insert(core, (Version::new(0, 1, 0), Version::new(0, 1, 1)));
        assert!(compare_plans(&ws, &files, false, &before, None, &after).is_unexpected());
    }

    #[test]
    fn knope_true_non_fallout_shape_is_unexpected() {
        let ws = workspace(vec![cargo_pkg("core", Version::new(0, 1, 0))]);
        let files = vec![bump("core", BumpLevel::Minor)];
        let after = oakum_plan(&ws, files.clone());
        let mut before = plan_fingerprint(&after);
        let core = PackageId::new(Ecosystem::Cargo, "core");
        // Not patch→minor: oakum would go 0.1.0→0.2.0; before claims 0.1.0→0.3.0.
        before.insert(core, (Version::new(0, 1, 0), Version::new(0, 3, 0)));
        assert!(compare_plans(&ws, &files, true, &before, None, &after).is_unexpected());
    }

    #[test]
    fn knope_feature_cascade_dependent_is_expected() {
        let core_id = PackageId::new(Ecosystem::Cargo, "core");
        let cli_id = PackageId::new(Ecosystem::Cargo, "cli");
        let ws = workspace(vec![
            cargo_pkg("core", Version::new(0, 1, 0)),
            Package::new(
                cli_id.clone(),
                Version::new(0, 1, 0),
                ResolvesDependenciesAt::Install,
                true,
                vec![edge(core_id.clone())],
            ),
        ]);
        let files = vec![bump("core", BumpLevel::Minor)];
        let after = oakum_plan(&ws, files.clone());
        assert!(
            after.get(&cli_id).is_some(),
            "cli must cascade from core for this unit"
        );
        // Source-tool before often lists only the feature package; oakum after
        // adds the cascade dependent — that gap is what cascaded_from_feature covers.
        let mut before = PlanFingerprint::new();
        before.insert(
            core_id.clone(),
            (Version::new(0, 1, 0), Version::new(0, 1, 1)),
        );
        let cmp = compare_plans(&ws, &files, true, &before, None, &after);
        let diffs = cmp
            .expected_knope_feature()
            .unwrap_or_else(|| panic!("expected cascade fallout, got {cmp:?}"));
        let ids: BTreeSet<_> = diffs.iter().map(PackageDiff::id).cloned().collect();
        assert!(ids.contains(&core_id) && ids.contains(&cli_id), "{ids:?}");
        assert!(compare_plans(&ws, &files, false, &before, None, &after).is_unexpected());
    }

    #[test]
    fn knope_feature_version_shape() {
        let from = Version::new(0, 1, 0);
        assert!(is_knope_feature_versions(
            Some(&(from.clone(), Version::new(0, 1, 1))),
            Some(&(from.clone(), Version::new(0, 2, 0))),
        ));
        assert!(!is_knope_feature_versions(
            Some(&(from.clone(), Version::new(0, 1, 1))),
            Some(&(from.clone(), Version::new(0, 3, 0))),
        ));
        assert!(!is_knope_feature_versions(None, None));
        assert!(!is_knope_feature_versions(
            Some(&(from.clone(), Version::new(0, 1, 1))),
            Some(&(Version::new(0, 2, 0), Version::new(0, 3, 0))),
        ));
    }
}
