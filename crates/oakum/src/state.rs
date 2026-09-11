//! Versioned release state for `status --json`, templates, and PR surfaces
//! ([ADR-0016](../../../docs/decisions/0016-emit-release-state-render-it-never-deliver-it.md)).
//!
//! An external parser of this JSON is the schema-crate trigger in
//! [ADR-0002](../../../docs/decisions/0002-single-crate-until-io.md); do not split ahead of it.

use semver::Version;
use serde::{Deserialize, Deserializer, Serialize};

use crate::plan::{BumpLevel, ChangeSource, PackageId, Plan, PlannedChange};

/// The coverage look's two answers, carried together so neither can be
/// reported without the other having been asked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Coverage {
    pub uncovered: Vec<PackageId>,
    pub unmanaged: Vec<PackageId>,
}

/// What happened to the coverage look. Three outcomes, not two: a report built
/// from a plan already in hand never asks the question, and saying "we could
/// not look" there blames git for a look nobody attempted.
///
/// Separate from [`CoverageOutcome`] rather than tagging it, because the wire
/// keeps `uncovered` and `unmanaged` unconditionally present: user templates
/// read them (ADR-0006) and `template::render` treats an absent key as an
/// error, so a flattened tag would break a template the first time git could
/// not diff the tree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CoverageLook {
    /// The look ran; `uncovered` and `unmanaged` are its answers.
    Ran,
    /// The look was attempted and could not answer.
    Failed,
    /// Nothing asked. The default, which is what every document predating the
    /// field meant.
    #[default]
    NotAsked,
}

/// The look's outcome and, when it ran, its answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoverageOutcome {
    Ran(Coverage),
    Failed,
    NotAsked,
}

/// Bump when a consumer must distinguish shapes.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseState {
    #[serde(deserialize_with = "schema_v1")]
    schema_version: u32,
    target: RenderTarget,
    packages: Vec<PackageRelease>,
    /// Packages that changed with no bump file (coverage gate). Meaningful
    /// only when `coverage` is `Ran`: an empty list otherwise means the look
    /// did not happen, not that nothing changed.
    uncovered: Vec<PackageRef>,
    /// Changed packages the config keeps but cannot version, so no bump file
    /// could ever cover them. Reported, never gated: ADR-0027 makes the
    /// private-package opt-in a choice, and `status` reports while `check`
    /// decides.
    #[serde(default)]
    unmanaged: Vec<PackageRef>,
    /// What happened to the look, so an empty `uncovered` can be read
    /// correctly: found nothing, could not look, or was never asked.
    #[serde(default)]
    coverage: CoverageLook,
    /// No selected package can produce work on either axis, so an empty
    /// `packages` means "this config can never release" rather than "nothing is
    /// pending". Defaulted rather than versioned: a reader that ignores it sees
    /// the shape it always saw.
    #[serde(default)]
    manages_nothing: bool,
    /// `include`/`exclude` left no package selected, so an empty `packages` is a
    /// config that can never release — for a different reason than
    /// `manages_nothing`, and one someone wrote down deliberately. `check` stays
    /// silent on it because a gate must not fail on a written decision; `status`
    /// reports it because `status` is not a gate ([ADR-0016]).
    ///
    /// [ADR-0016]: ../../docs/decisions/0016-emit-release-state-render-it-never-deliver-it.md
    #[serde(default)]
    selection_empty: bool,
}

impl ReleaseState {
    /// The look and its answers are one argument, so a caller cannot claim the
    /// first without the second.
    #[must_use]
    pub fn from_plan(plan: &Plan, coverage: CoverageOutcome, target: RenderTarget) -> Self {
        let (uncovered, unmanaged, look) = match coverage {
            CoverageOutcome::Ran(Coverage {
                uncovered,
                unmanaged,
            }) => (uncovered, unmanaged, CoverageLook::Ran),
            CoverageOutcome::Failed => (Vec::new(), Vec::new(), CoverageLook::Failed),
            CoverageOutcome::NotAsked => (Vec::new(), Vec::new(), CoverageLook::NotAsked),
        };
        let mut packages: Vec<PackageRelease> =
            plan.changes().values().map(PackageRelease::from).collect();
        packages.sort_by(|left, right| {
            left.id
                .ecosystem
                .cmp(&right.id.ecosystem)
                .then(left.id.name.cmp(&right.id.name))
        });
        let mut uncovered: Vec<PackageRef> = uncovered.into_iter().map(PackageRef::from).collect();
        uncovered.sort_by(PackageRef::wire_order);
        let mut unmanaged: Vec<PackageRef> = unmanaged.into_iter().map(PackageRef::from).collect();
        unmanaged.sort_by(PackageRef::wire_order);
        Self {
            schema_version: SCHEMA_VERSION,
            target,
            packages,
            uncovered,
            unmanaged,
            coverage: look,
            manages_nothing: false,
            selection_empty: false,
        }
    }

    #[must_use]
    pub const fn coverage(&self) -> CoverageLook {
        self.coverage
    }

    #[must_use]
    pub fn unmanaged(&self) -> &[PackageRef] {
        match self.coverage {
            CoverageLook::Ran => &self.unmanaged,
            CoverageLook::Failed | CoverageLook::NotAsked => &[],
        }
    }

    /// Records that the config manages nothing. Set by the caller that asked,
    /// because deciding it needs the workspace and the config, neither of which
    /// a plan carries.
    ///
    /// # Panics
    ///
    /// In debug builds, when a package is planned: a config that can never
    /// release cannot have planned a release, and the pair would tell a reader
    /// two contradictory things.
    #[must_use]
    pub fn managing_nothing(mut self) -> Self {
        debug_assert!(
            self.packages.is_empty(),
            "manages_nothing beside a planned release"
        );
        debug_assert!(
            !self.selection_empty,
            "a selection with no package cannot also hold an unmanaged one"
        );
        self.manages_nothing = true;
        self
    }

    /// The two reasons a plan can never be non-empty are exclusive: a selection
    /// with nothing in it has no package left to be unmanaged.
    #[must_use]
    pub fn selection_emptied(mut self) -> Self {
        debug_assert!(
            self.packages.is_empty(),
            "selection_empty beside a planned release"
        );
        debug_assert!(
            !self.manages_nothing,
            "a selection with no package cannot also hold an unmanaged one"
        );
        self.selection_empty = true;
        self
    }

    /// Guarded like [`Self::uncovered`], and for the same reason: `Deserialize`
    /// is a second constructor that runs no builder, and `debug_assert` is
    /// compiled out of the shipped binary. A flag beside a planned release is
    /// not a fact about this state, so it does not become one by being written
    /// into the JSON.
    #[must_use]
    pub fn manages_nothing(&self) -> bool {
        self.manages_nothing && self.packages.is_empty()
    }

    /// The two are exclusive, and `manages_nothing` wins when a document claims
    /// both: a selection with nothing in it has no package left to be unmanaged,
    /// so a document asserting both is describing neither.
    #[must_use]
    pub fn selection_empty(&self) -> bool {
        self.selection_empty && !self.manages_nothing && self.packages.is_empty()
    }

    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    #[must_use]
    pub const fn target(&self) -> RenderTarget {
        self.target
    }

    #[must_use]
    pub fn packages(&self) -> &[PackageRelease] {
        &self.packages
    }

    /// Empty unless the look ran. `Deserialize` is a second constructor that
    /// `from_plan`'s match cannot reach, so a document can carry a tag that
    /// disagrees with these lists; the accessor is where both readers meet.
    #[must_use]
    pub fn uncovered(&self) -> &[PackageRef] {
        match self.coverage {
            CoverageLook::Ran => &self.uncovered,
            CoverageLook::Failed | CoverageLook::NotAsked => &[],
        }
    }
}

/// Same discriminator bumpy uses so a template can drop a date from a GitHub
/// release body that already shows one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RenderTarget {
    Status,
    Changelog,
    GithubRelease,
    Comment,
    Summary,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageRef {
    name: String,
    ecosystem: EcosystemName,
}

impl From<PackageId> for PackageRef {
    fn from(id: PackageId) -> Self {
        Self {
            name: id.name,
            ecosystem: EcosystemName::from_plan(id.ecosystem),
        }
    }
}

impl PackageRef {
    /// The order both wire lists are emitted in, so a consumer diffing two
    /// documents sees a change only when the set changed.
    fn wire_order(left: &Self, right: &Self) -> std::cmp::Ordering {
        left.ecosystem
            .cmp(&right.ecosystem)
            .then(left.name.cmp(&right.name))
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn ecosystem(&self) -> EcosystemName {
        self.ecosystem
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EcosystemName {
    Cargo,
    Npm,
}

impl EcosystemName {
    const fn from_plan(ecosystem: crate::plan::Ecosystem) -> Self {
        match ecosystem {
            crate::plan::Ecosystem::Cargo => Self::Cargo,
            crate::plan::Ecosystem::Npm => Self::Npm,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageRelease {
    #[serde(flatten)]
    id: PackageRef,
    #[serde(with = "exact_version")]
    from: Version,
    #[serde(with = "exact_version")]
    to: Version,
    bump: BumpName,
    source: ReleaseSource,
}

impl From<&PlannedChange> for PackageRelease {
    fn from(change: &PlannedChange) -> Self {
        Self {
            id: PackageRef::from(change.id().clone()),
            from: change.from().clone(),
            to: change.to().clone(),
            bump: BumpName::from_level(change.applied().effective()),
            source: ReleaseSource::from(change.source()),
        }
    }
}

impl PackageRelease {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.id.name
    }

    #[must_use]
    pub const fn ecosystem(&self) -> EcosystemName {
        self.id.ecosystem
    }

    #[must_use]
    pub fn from_version(&self) -> &Version {
        &self.from
    }

    #[must_use]
    pub fn to_version(&self) -> &Version {
        &self.to
    }

    #[must_use]
    pub const fn bump(&self) -> BumpName {
        self.bump
    }

    #[must_use]
    pub const fn source(&self) -> &ReleaseSource {
        &self.source
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BumpName {
    Patch,
    Minor,
    Major,
}

impl BumpName {
    fn from_level(level: BumpLevel) -> Self {
        match level {
            BumpLevel::Patch => Self::Patch,
            BumpLevel::Minor => Self::Minor,
            BumpLevel::Major => Self::Major,
            BumpLevel::None => unreachable!("compose does not plan a none bump"),
        }
    }
}

fn schema_v1<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let version = u32::deserialize(deserializer)?;
    if version == SCHEMA_VERSION {
        Ok(version)
    } else {
        Err(serde::de::Error::custom(format!(
            "unsupported schema_version {version}, expected {SCHEMA_VERSION}"
        )))
    }
}

mod exact_version {
    use super::{Deserialize, Deserializer, Version};
    use serde::Serializer;

    pub fn serialize<S: Serializer>(version: &Version, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&version.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Version, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Version::parse(&raw).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum ReleaseSource {
    Intent,
    Cascade { trigger: PackageRef },
}

impl From<&ChangeSource> for ReleaseSource {
    fn from(source: &ChangeSource) -> Self {
        match source {
            ChangeSource::Intent => Self::Intent,
            ChangeSource::Cascade { trigger } => Self::Cascade {
                trigger: PackageRef::from(trigger.clone()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{
        aggregate, compose, Bounds, BuildResolution, BumpFile, BumpLevel, CascadeAs, DeclaredRange,
        Dependency, DependencyKind, Ecosystem, Package, PackageId, ResolvesDependenciesAt,
        Versioning, Workspace,
    };
    use semver::Version;

    fn cargo(name: &str) -> PackageId {
        PackageId::new(Ecosystem::Cargo, name)
    }

    fn workspace_one(name: &str, version: Version) -> Workspace {
        Workspace::new([Package::new(
            cargo(name),
            version,
            ResolvesDependenciesAt::Install,
            true,
            Vec::new(),
        )])
        .expect("workspace")
    }

    /// A document saying the look never ran while carrying its findings is a
    /// contradiction the accessors refuse to pass on: otherwise one comment
    /// body could name an uncovered package and say the look never happened.
    #[test]
    fn lists_are_hidden_when_the_tag_says_no_look_ran() {
        for (look, label) in [("not-asked", "nobody asked"), ("failed", "the look failed")] {
            let json = format!(
                r#"{{"schema_version":1,"target":"status","packages":[],
                     "uncovered":[{{"name":"gap","ecosystem":"npm"}}],
                     "unmanaged":[{{"name":"beta","ecosystem":"cargo"}}],
                     "coverage":"{look}","manages_nothing":false,"selection_empty":false}}"#
            );
            let state: ReleaseState = serde_json::from_str(&json).expect("deserializes");
            assert!(
                state.uncovered().is_empty(),
                "{label}, so there are no findings to report"
            );
            assert!(state.unmanaged().is_empty(), "{label}");
        }
    }

    /// The same fields are reported when the tag says the look ran, so the
    /// guard above hides a contradiction rather than the answer.
    #[test]
    fn lists_are_reported_when_the_tag_says_the_look_ran() {
        let json = r#"{"schema_version":1,"target":"status","packages":[],
             "uncovered":[{"name":"gap","ecosystem":"npm"}],
             "unmanaged":[{"name":"beta","ecosystem":"cargo"}],
             "coverage":"ran","manages_nothing":false,"selection_empty":false}"#;
        let state: ReleaseState = serde_json::from_str(json).expect("deserializes");
        assert_eq!(state.uncovered().len(), 1);
        assert_eq!(state.unmanaged().len(), 1);
    }

    #[test]
    fn schema_version_is_one() {
        let ws = workspace_one("demo", Version::new(0, 1, 0));
        let id = cargo("demo");
        let intent = aggregate([BumpFile {
            id: String::from("one.md"),
            entries: vec![(id, BumpLevel::Patch)],
            note: String::new(),
        }]);
        let plan = compose(
            &ws,
            &intent,
            |_| Versioning::ZeroMajor,
            CascadeAs::Patch,
            |_, _| None,
            |pkg| ws.get(pkg).expect("pkg").version().clone(),
        )
        .expect("plan");
        let state = ReleaseState::from_plan(&plan, CoverageOutcome::NotAsked, RenderTarget::Status);
        assert_eq!(state.schema_version(), SCHEMA_VERSION);
        assert_eq!(state.target(), RenderTarget::Status);
        assert_eq!(state.packages().len(), 1);
        assert_eq!(state.packages()[0].name(), "demo");
        assert_eq!(state.packages()[0].from_version(), &Version::new(0, 1, 0));
        assert_eq!(state.packages()[0].bump(), BumpName::Patch);
        assert_eq!(state.packages()[0].source(), &ReleaseSource::Intent);
        let json = serde_json::to_string(&state).expect("json");
        assert!(json.contains("\"schema_version\":1"), "{json}");
        let back: ReleaseState = serde_json::from_str(&json).expect("roundtrip");
        assert_eq!(back, state);
    }

    #[test]
    fn target_survives_json() {
        let ws = workspace_one("demo", Version::new(0, 1, 0));
        let intent = aggregate([BumpFile {
            id: String::from("one.md"),
            entries: vec![(cargo("demo"), BumpLevel::Patch)],
            note: String::new(),
        }]);
        let plan = compose(
            &ws,
            &intent,
            |_| Versioning::ZeroMajor,
            CascadeAs::Patch,
            |_, _| None,
            |pkg| ws.get(pkg).expect("pkg").version().clone(),
        )
        .expect("plan");
        let state = ReleaseState::from_plan(
            &plan,
            CoverageOutcome::Ran(Coverage {
                uncovered: vec![npm("gap")],
                unmanaged: Vec::new(),
            }),
            RenderTarget::GithubRelease,
        );
        assert_eq!(state.target(), RenderTarget::GithubRelease);
        assert_eq!(state.uncovered()[0].name(), "gap");
        let json = serde_json::to_string(&state).expect("json");
        assert!(json.contains("github-release"), "{json}");
        let back: ReleaseState = serde_json::from_str(&json).expect("roundtrip");
        assert_eq!(back, state);
        assert_eq!(back.target(), RenderTarget::GithubRelease);
    }

    #[test]
    fn foreign_schema_version_is_rejected() {
        let err = serde_json::from_str::<ReleaseState>(
            r#"{"schema_version":2,"target":"status","packages":[],"uncovered":[]}"#,
        )
        .expect_err("v2");
        assert!(err.to_string().contains("schema_version"), "{err}");
        let err = serde_json::from_str::<ReleaseState>(
            r#"{"schema_version":1,"target":"status","packages":[{"name":"x","ecosystem":"cargo","from":"^1.0.0","to":"1.0.1","bump":"patch","source":{"kind":"intent"}}],"uncovered":[]}"#,
        )
        .expect_err("range");
        assert!(
            err.to_string().contains("unexpected character") || err.to_string().contains('^'),
            "{err}"
        );
    }

    fn npm(name: &str) -> PackageId {
        PackageId::new(Ecosystem::Npm, name)
    }

    fn edge(on: PackageId) -> Dependency {
        let declared_as = on.name.clone();
        Dependency {
            on,
            kind: DependencyKind::Normal,
            declared_as,
            target: None,
            range: DeclaredRange::Plain(Bounds::from_cargo_text("^0.1.0").expect("range")),
        }
    }

    #[test]
    fn from_plan_maps_cascade_uncovered_and_wire_names() {
        let core = cargo("core");
        let cli = cargo("cli");
        let ws = Workspace::new([
            Package::new(
                core.clone(),
                Version::new(0, 1, 0),
                ResolvesDependenciesAt::Install,
                true,
                Vec::new(),
            ),
            Package::new(
                cli.clone(),
                Version::new(0, 1, 0),
                ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget),
                true,
                vec![edge(core.clone())],
            ),
        ])
        .expect("workspace");
        let intent = aggregate([BumpFile {
            id: String::from("one.md"),
            entries: vec![(core.clone(), BumpLevel::Patch)],
            note: String::new(),
        }]);
        let plan = compose(
            &ws,
            &intent,
            |_| Versioning::ZeroMajor,
            CascadeAs::Patch,
            |_, dep| Some(dep.range.clone()),
            |pkg| ws.get(pkg).expect("pkg").version().clone(),
        )
        .expect("plan");
        let gap = npm("gap");
        let state = ReleaseState::from_plan(
            &plan,
            CoverageOutcome::Ran(Coverage {
                uncovered: vec![gap.clone(), cargo("extra")],
                unmanaged: Vec::new(),
            }),
            RenderTarget::Status,
        );
        assert_eq!(state.packages().len(), 2);
        let core_pkg = state
            .packages()
            .iter()
            .find(|pkg| pkg.name() == "core")
            .expect("core");
        let cli_pkg = state
            .packages()
            .iter()
            .find(|pkg| pkg.name() == "cli")
            .expect("cli");
        assert_eq!(core_pkg.source(), &ReleaseSource::Intent);
        assert_eq!(
            cli_pkg.source(),
            &ReleaseSource::Cascade {
                trigger: PackageRef::from(core.clone()),
            }
        );
        assert_eq!(state.uncovered()[0].name(), "extra");
        assert_eq!(state.uncovered()[0].ecosystem(), EcosystemName::Cargo);
        assert_eq!(state.uncovered()[1].name(), "gap");
        assert_eq!(state.uncovered()[1].ecosystem(), EcosystemName::Npm);
        let json = serde_json::to_string(&state).expect("json");
        assert!(json.contains("\"kind\":\"intent\""), "{json}");
        assert!(json.contains("\"kind\":\"cascade\""), "{json}");
        assert!(json.contains("\"bump\":\"patch\""), "{json}");
        assert!(json.contains("\"ecosystem\":\"cargo\""), "{json}");
        assert!(json.contains("\"uncovered\""), "{json}");
        let back: ReleaseState = serde_json::from_str(&json).expect("roundtrip");
        assert_eq!(back, state);
    }

    #[test]
    fn from_plan_uses_effective_bump() {
        let ws = workspace_one("demo", Version::new(0, 1, 0));
        let intent = aggregate([BumpFile {
            id: String::from("one.md"),
            entries: vec![(cargo("demo"), BumpLevel::Major)],
            note: String::new(),
        }]);
        let plan = compose(
            &ws,
            &intent,
            |_| Versioning::ZeroMajor,
            CascadeAs::Patch,
            |_, _| None,
            |pkg| ws.get(pkg).expect("pkg").version().clone(),
        )
        .expect("plan");
        let state = ReleaseState::from_plan(&plan, CoverageOutcome::NotAsked, RenderTarget::Status);
        assert_eq!(state.packages()[0].to_version(), &Version::new(0, 2, 0));
        assert_eq!(state.packages()[0].bump(), BumpName::Minor);
        let json = serde_json::to_string(&state).expect("json");
        assert!(json.contains("\"bump\":\"minor\""), "{json}");
    }

    #[test]
    fn packages_sort_cargo_before_npm() {
        let ws = Workspace::new([
            Package::new(
                npm("app"),
                Version::new(1, 0, 0),
                ResolvesDependenciesAt::Install,
                true,
                Vec::new(),
            ),
            Package::new(
                cargo("lib"),
                Version::new(0, 1, 0),
                ResolvesDependenciesAt::Install,
                true,
                Vec::new(),
            ),
        ])
        .expect("workspace");
        let intent = aggregate([BumpFile {
            id: String::from("one.md"),
            entries: vec![
                (npm("app"), BumpLevel::Patch),
                (cargo("lib"), BumpLevel::Patch),
            ],
            note: String::new(),
        }]);
        let plan = compose(
            &ws,
            &intent,
            |_| Versioning::Semver,
            CascadeAs::Patch,
            |_, _| None,
            |pkg| ws.get(pkg).expect("pkg").version().clone(),
        )
        .expect("plan");
        let state = ReleaseState::from_plan(&plan, CoverageOutcome::NotAsked, RenderTarget::Status);
        assert_eq!(state.packages()[0].ecosystem(), EcosystemName::Cargo);
        assert_eq!(state.packages()[1].ecosystem(), EcosystemName::Npm);
    }
}
