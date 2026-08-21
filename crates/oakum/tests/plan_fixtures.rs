//! Snapshot the pure planner against `tests/fixtures/plan/*/in|out`.
//!
//! Each case directory holds JSON the harness loads into [`oakum::plan`] types.
//! `out/plan.json` asserts a successful compose; `out/error.json` asserts a
//! refused workspace (cycles). Schema is fixture-only so `no_std` plan stays
//! free of serde.
#![allow(clippy::disallowed_methods)]

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use oakum::plan::aggregate::{aggregate, AggregatedBump, BumpFile};
use oakum::plan::bump::{BumpLevel, Versioning};
use oakum::plan::cascade::CascadeAs;
use oakum::plan::compose::{ChangeSource, Plan};
use oakum::plan::workspace::{
    BuildResolution, DeclaredRange, Dependency, DependencyKind, Ecosystem, Package, PackageId,
    ResolvesDependenciesAt, Tracking, Workspace,
};
use oakum::plan::Bounds;
use semver::Version;
use serde::Deserialize;

#[test]
fn plan_fixture_suite() {
    let root = support::workspace_root().join("crates/oakum/tests/fixtures/plan");
    assert!(
        root.is_dir(),
        "plan fixture root missing: {}",
        root.display()
    );

    let mut cases: Vec<PathBuf> = fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("read {}: {e}", root.display()))
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.is_dir() && path.join("in").is_dir() && path.join("out").is_dir() {
                Some(path)
            } else {
                None
            }
        })
        .collect();
    cases.sort();
    assert!(
        !cases.is_empty(),
        "no plan/*/in+out cases under {}",
        root.display()
    );

    for case in cases {
        let name = case.file_name().and_then(|s| s.to_str()).unwrap_or("?");
        let out_plan = case.join("out/plan.json");
        let out_error = case.join("out/error.json");
        match (out_plan.exists(), out_error.exists()) {
            (true, false) => {
                let plan = run_compose_case(&case).unwrap_or_else(|e| panic!("case {name}: {e}"));
                let expected = load_expected_plan(&out_plan)
                    .unwrap_or_else(|e| panic!("case {name} out/plan.json: {e}"));
                assert_eq!(snapshot_plan(&plan), expected, "case {name}: plan mismatch");
            }
            (false, true) => {
                let expected = load_expected_error(&out_error)
                    .unwrap_or_else(|e| panic!("case {name} out/error.json: {e}"));
                let err = load_workspace(&case.join("in/workspace.json"))
                    .expect_err(&format!("case {name}: expected workspace error"));
                assert_error(&err, &expected, name);
            }
            (true, true) => panic!("case {name}: out/ must have plan.json or error.json, not both"),
            (false, false) => panic!("case {name}: out/ needs plan.json or error.json"),
        }
    }
}

fn run_compose_case(case: &Path) -> Result<Plan, String> {
    let workspace = load_workspace(&case.join("in/workspace.json"))?;
    let intent = load_intent(&case.join("in/intent.json"))?;
    let options = load_options(&case.join("in/options.json"))?;
    let versioning = options.versioning();
    let cascade_as = options.cascade_as();

    oakum::plan::compose(
        &workspace,
        &intent,
        |_| versioning,
        cascade_as,
        |_, edge| Some(edge.range.clone()),
        |id| {
            workspace
                .get(id)
                .expect("package in workspace")
                .version()
                .clone()
        },
    )
    .map_err(|e| e.to_string())
}

fn assert_error(err: &str, expected: &ExpectedError, name: &str) {
    match expected {
        ExpectedError::Cycle { path } => {
            let suffix = path
                .iter()
                .map(|pkg| format!("{} ({})", pkg.name, pkg.ecosystem.as_str()))
                .collect::<Vec<_>>()
                .join(" -> ");
            let expected_msg = format!("found cycle in dependency graph: {suffix}");
            assert_eq!(
                err, expected_msg,
                "case {name}: cycle path snapshot mismatch"
            );
        }
    }
}

#[derive(Debug, Deserialize)]
struct FixtureOptions {
    #[serde(default = "default_cascade_as")]
    cascade_as: FixtureCascadeAs,
    #[serde(default = "default_versioning")]
    versioning: FixtureVersioning,
}

impl FixtureOptions {
    fn cascade_as(&self) -> CascadeAs {
        match self.cascade_as {
            FixtureCascadeAs::Patch => CascadeAs::Patch,
            FixtureCascadeAs::Minor => CascadeAs::Minor,
            FixtureCascadeAs::None => CascadeAs::None,
        }
    }

    fn versioning(&self) -> Versioning {
        match self.versioning {
            FixtureVersioning::ZeroMajor => Versioning::ZeroMajor,
            FixtureVersioning::Semver => Versioning::Semver,
        }
    }
}

fn default_cascade_as() -> FixtureCascadeAs {
    FixtureCascadeAs::Patch
}

fn default_versioning() -> FixtureVersioning {
    FixtureVersioning::ZeroMajor
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum FixtureCascadeAs {
    Patch,
    Minor,
    None,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum FixtureVersioning {
    ZeroMajor,
    Semver,
}

fn load_options(path: &Path) -> Result<FixtureOptions, String> {
    if !path.exists() {
        return Ok(FixtureOptions {
            cascade_as: default_cascade_as(),
            versioning: default_versioning(),
        });
    }
    read_json(path)
}

#[derive(Debug, Deserialize)]
struct FixtureWorkspace {
    packages: Vec<FixturePackage>,
}

#[derive(Debug, Deserialize)]
struct FixturePackage {
    ecosystem: FixtureEcosystem,
    name: String,
    version: String,
    resolves: FixtureResolves,
    #[serde(default = "default_publishable")]
    publishable: bool,
    #[serde(default)]
    dependencies: Vec<FixtureDependency>,
}

fn default_publishable() -> bool {
    true
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum FixtureEcosystem {
    Cargo,
    Npm,
}

impl FixtureEcosystem {
    fn into_ecosystem(self) -> Ecosystem {
        match self {
            Self::Cargo => Ecosystem::Cargo,
            Self::Npm => Ecosystem::Npm,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Cargo => "cargo",
            Self::Npm => "npm",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum FixtureResolves {
    Install,
    BuildBinary,
    BuildDeclared,
}

impl FixtureResolves {
    fn into_resolves(self) -> ResolvesDependenciesAt {
        match self {
            Self::Install => ResolvesDependenciesAt::Install,
            Self::BuildBinary => ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget),
            Self::BuildDeclared => ResolvesDependenciesAt::Build(BuildResolution::Declared),
        }
    }
}

#[derive(Debug, Deserialize)]
struct FixtureDependency {
    on: String,
    #[serde(default = "default_dep_ecosystem")]
    ecosystem: FixtureEcosystem,
    kind: FixtureDependencyKind,
    /// Plain string, protocol object, or omitted (Cargo path-linked).
    #[serde(default)]
    range: Option<FixtureRange>,
    #[serde(default)]
    declared_as: Option<String>,
    #[serde(default)]
    target: Option<String>,
}

fn default_dep_ecosystem() -> FixtureEcosystem {
    FixtureEcosystem::Cargo
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum FixtureRange {
    Plain(String),
    Shaped(FixtureRangeShape),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum FixtureRangeShape {
    WorkspaceTracking(FixtureTracking),
    Workspace(String),
    Catalog {
        #[serde(default)]
        name: Option<String>,
        bounds: String,
    },
    PathLinked,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum FixtureTracking {
    Exact,
    Tilde,
    Caret,
}

impl FixtureTracking {
    fn into_tracking(self) -> Tracking {
        match self {
            Self::Exact => Tracking::Exact,
            Self::Tilde => Tracking::Tilde,
            Self::Caret => Tracking::Caret,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum FixtureDependencyKind {
    Normal,
    Peer,
    Optional,
    Build,
    Development,
}

impl FixtureDependencyKind {
    fn into_kind(self) -> DependencyKind {
        match self {
            Self::Normal => DependencyKind::Normal,
            Self::Peer => DependencyKind::Peer,
            Self::Optional => DependencyKind::Optional,
            Self::Build => DependencyKind::Build,
            Self::Development => DependencyKind::Development,
        }
    }
}

fn parse_range(
    ecosystem: Ecosystem,
    range: Option<FixtureRange>,
    pkg: &str,
    dep: &str,
) -> Result<DeclaredRange, String> {
    match (ecosystem, range) {
        (Ecosystem::Cargo, None) => Ok(DeclaredRange::PathLinked),
        (Ecosystem::Npm, None) => Err(format!("package {pkg} dep {dep}: npm edges need a range")),
        (Ecosystem::Cargo, Some(FixtureRange::Plain(text))) => Ok(DeclaredRange::Plain(
            Bounds::from_cargo_text(&text).map_err(|e| format!("package {pkg} dep {dep}: {e}"))?,
        )),
        (Ecosystem::Npm, Some(FixtureRange::Plain(text))) => Ok(DeclaredRange::Plain(
            Bounds::from_npm_text(&text).map_err(|e| format!("package {pkg} dep {dep}: {e}"))?,
        )),
        (_, Some(FixtureRange::Shaped(FixtureRangeShape::PathLinked))) => {
            Ok(DeclaredRange::PathLinked)
        }
        (_, Some(FixtureRange::Shaped(FixtureRangeShape::WorkspaceTracking(t)))) => {
            Ok(DeclaredRange::WorkspaceTracking(t.into_tracking()))
        }
        (Ecosystem::Npm, Some(FixtureRange::Shaped(FixtureRangeShape::Workspace(text)))) => {
            Ok(DeclaredRange::Workspace(
                Bounds::from_npm_text(&text)
                    .map_err(|e| format!("package {pkg} dep {dep}: {e}"))?,
            ))
        }
        (Ecosystem::Cargo, Some(FixtureRange::Shaped(FixtureRangeShape::Workspace(text)))) => {
            Ok(DeclaredRange::Workspace(
                Bounds::from_cargo_text(&text)
                    .map_err(|e| format!("package {pkg} dep {dep}: {e}"))?,
            ))
        }
        (eco, Some(FixtureRange::Shaped(FixtureRangeShape::Catalog { name, bounds }))) => {
            let bounds = match eco {
                Ecosystem::Npm => Bounds::from_npm_text(&bounds),
                Ecosystem::Cargo => Bounds::from_cargo_text(&bounds),
            }
            .map_err(|e| format!("package {pkg} dep {dep}: {e}"))?;
            Ok(DeclaredRange::Catalog { name, bounds })
        }
    }
}

fn load_workspace(path: &Path) -> Result<Workspace, String> {
    let raw: FixtureWorkspace = read_json(path)?;
    let packages: Result<Vec<_>, String> = raw
        .packages
        .into_iter()
        .map(|pkg| {
            let ecosystem = pkg.ecosystem.into_ecosystem();
            let id = PackageId::new(ecosystem, pkg.name.clone());
            let version: Version = pkg
                .version
                .parse()
                .map_err(|e| format!("package {}: bad version: {e}", pkg.name))?;
            let dependencies = pkg
                .dependencies
                .into_iter()
                .map(|dep| {
                    let on = PackageId::new(dep.ecosystem.into_ecosystem(), dep.on.clone());
                    let declared_as = dep.declared_as.unwrap_or_else(|| dep.on.clone());
                    let range = parse_range(ecosystem, dep.range, &pkg.name, &dep.on)?;
                    Ok(Dependency {
                        on,
                        kind: dep.kind.into_kind(),
                        declared_as,
                        target: dep.target,
                        range,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            Ok(Package::new(
                id,
                version,
                pkg.resolves.into_resolves(),
                pkg.publishable,
                dependencies,
            ))
        })
        .collect();
    Workspace::new(packages?).map_err(|e| e.to_string())
}

#[derive(Debug, Deserialize)]
struct FixtureIntent {
    entries: Vec<FixtureIntentEntry>,
}

#[derive(Debug, Deserialize)]
struct FixtureIntentEntry {
    ecosystem: FixtureEcosystem,
    name: String,
    level: FixtureBumpLevel,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum FixtureBumpLevel {
    Patch,
    Minor,
    Major,
}

impl FixtureBumpLevel {
    fn into_level(self) -> BumpLevel {
        match self {
            Self::Patch => BumpLevel::Patch,
            Self::Minor => BumpLevel::Minor,
            Self::Major => BumpLevel::Major,
        }
    }
}

fn load_intent(path: &Path) -> Result<BTreeMap<PackageId, AggregatedBump>, String> {
    let raw: FixtureIntent = read_json(path)?;
    let entries = raw
        .entries
        .into_iter()
        .map(|e| {
            (
                PackageId::new(e.ecosystem.into_ecosystem(), e.name),
                e.level.into_level(),
            )
        })
        .collect::<Vec<_>>();
    Ok(aggregate([BumpFile {
        id: String::from("fixture"),
        entries,
        note: String::from("fixture intent"),
    }]))
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct ExpectedPlan {
    changes: Vec<ExpectedChange>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct ExpectedChange {
    ecosystem: FixtureEcosystem,
    name: String,
    from: String,
    to: String,
    requested: FixtureBumpLevel,
    source: ExpectedSource,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum ExpectedSource {
    Intent,
    Cascade { trigger: ExpectedPackageRef },
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct ExpectedPackageRef {
    ecosystem: FixtureEcosystem,
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum ExpectedError {
    Cycle { path: Vec<ExpectedPackageRef> },
}

fn load_expected_plan(path: &Path) -> Result<ExpectedPlan, String> {
    read_json(path)
}

fn load_expected_error(path: &Path) -> Result<ExpectedError, String> {
    read_json(path)
}

fn snapshot_plan(plan: &Plan) -> ExpectedPlan {
    ExpectedPlan {
        changes: plan
            .changes()
            .values()
            .map(|change| ExpectedChange {
                ecosystem: match change.id().ecosystem {
                    Ecosystem::Cargo => FixtureEcosystem::Cargo,
                    Ecosystem::Npm => FixtureEcosystem::Npm,
                },
                name: change.id().name.clone(),
                from: change.from().to_string(),
                to: change.to().to_string(),
                requested: match change.applied().requested() {
                    BumpLevel::Patch => FixtureBumpLevel::Patch,
                    BumpLevel::Minor => FixtureBumpLevel::Minor,
                    BumpLevel::Major => FixtureBumpLevel::Major,
                },
                source: match change.source() {
                    ChangeSource::Intent => ExpectedSource::Intent,
                    ChangeSource::Cascade { trigger } => ExpectedSource::Cascade {
                        trigger: ExpectedPackageRef {
                            ecosystem: match trigger.ecosystem {
                                Ecosystem::Cargo => FixtureEcosystem::Cargo,
                                Ecosystem::Npm => FixtureEcosystem::Npm,
                            },
                            name: trigger.name.clone(),
                        },
                    },
                },
            })
            .collect(),
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}
