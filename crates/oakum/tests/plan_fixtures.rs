//! Snapshot the pure planner against `tests/fixtures/plan/*/in|out`.
//!
//! Each case directory holds JSON the harness loads into [`oakum::plan`] types,
//! runs [`oakum::plan::compose`], and compares to `out/plan.json`. Schema is
//! fixture-only (not on the public plan types) so `no_std` plan stays free of
//! serde.
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
    ResolvesDependenciesAt, Workspace,
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
        let plan = run_case(&case).unwrap_or_else(|e| panic!("case {name}: {e}"));
        let expected = load_expected_plan(&case.join("out/plan.json"))
            .unwrap_or_else(|e| panic!("case {name} out/plan.json: {e}"));
        assert_eq!(snapshot_plan(&plan), expected, "case {name}: plan mismatch");
    }
}

fn run_case(case: &Path) -> Result<Plan, String> {
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
    #[serde(default)]
    dependencies: Vec<FixtureDependency>,
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
    #[serde(default)]
    range: Option<String>,
    #[serde(default)]
    declared_as: Option<String>,
    #[serde(default)]
    target: Option<String>,
}

fn default_dep_ecosystem() -> FixtureEcosystem {
    FixtureEcosystem::Cargo
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
                    let range = match (ecosystem, dep.range.as_deref()) {
                        (Ecosystem::Cargo, None) => DeclaredRange::PathLinked,
                        (Ecosystem::Cargo, Some(text)) => DeclaredRange::Plain(
                            Bounds::from_cargo_text(text)
                                .map_err(|e| format!("package {} dep {}: {e}", pkg.name, dep.on))?,
                        ),
                        (Ecosystem::Npm, Some(text)) => DeclaredRange::Plain(
                            Bounds::from_npm_text(text)
                                .map_err(|e| format!("package {} dep {}: {e}", pkg.name, dep.on))?,
                        ),
                        (Ecosystem::Npm, None) => {
                            return Err(format!(
                                "package {} dep {}: npm edges need a range",
                                pkg.name, dep.on
                            ));
                        }
                    };
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

fn load_expected_plan(path: &Path) -> Result<ExpectedPlan, String> {
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
