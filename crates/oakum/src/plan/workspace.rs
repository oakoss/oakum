//! The workspace the planner reasons over: packages, their current versions, and
//! the intra-workspace edges between them.
//!
//! `discover` builds this by asking each package manager; nothing here reads a
//! manifest, so the cascade rules can be replayed against a recorded release
//! history.
//!
//! `String`, `Vec`, and friends come from `alloc` rather than the std prelude,
//! and the graph is a `BTreeMap` because `alloc` has no `HashMap` — see `plan`'s
//! module doc for what enforces that. Ordering is wanted independently: ADR-0012
//! checks the planner against `in/` + `out/` snapshot fixtures.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use semver::{Comparator, Op, Version, VersionReq};

use super::bounds::Bounds;

/// The registry namespace within which a package name is unique.
///
/// This is the identity axis, not the adapter axis. Discovery adapts per package
/// manager — pnpm and npm answer differently about the same directory — but npm,
/// pnpm, yarn, and bun packages all draw their names from one registry, so names
/// collide across those four and never across ecosystems.
///
/// Variant order sets [`PackageId`]'s sort order, which plans are compared by.
/// Reordering changes plan output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Ecosystem {
    Npm,
    Cargo,
}

impl Ecosystem {
    /// Whether a range may be written with a [`RangeProtocol`] prefix. Cargo has
    /// no grammar for either one, and its `workspace = true` inheritance is a
    /// different mechanism that `cargo metadata` resolves to a plain range
    /// before oakum sees it.
    const fn has_range_protocols(self) -> bool {
        match self {
            Self::Npm => true,
            Self::Cargo => false,
        }
    }

    const fn has_target_tables(self) -> bool {
        match self {
            Self::Npm => false,
            Self::Cargo => true,
        }
    }
}

impl fmt::Display for Ecosystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Npm => "npm",
            Self::Cargo => "cargo",
        })
    }
}

/// What identifies a package inside a workspace.
///
/// The ecosystem is part of the identity because a polyglot repository can hold a
/// crate and an npm package under the same name. Keyed by name alone the two
/// would collapse into one node carrying both packages' edges, and a release of
/// either would cascade into the other's dependents.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageId {
    pub ecosystem: Ecosystem,
    pub name: String,
}

impl PackageId {
    #[must_use]
    pub fn new(ecosystem: Ecosystem, name: impl Into<String>) -> Self {
        Self {
            ecosystem,
            name: name.into(),
        }
    }
}

impl fmt::Display for PackageId {
    /// Carries the ecosystem, because a bare name is what makes a message
    /// ambiguous in the repositories this has to describe.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.name, self.ecosystem)
    }
}

/// The manifest section an edge was declared in, kept rather than reduced to
/// whether it cascades.
///
/// ADR-0008 decides which kinds are eligible, but the section survives into the
/// plan because `version` rewrites a range in the section it came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DependencyKind {
    Normal,
    Peer,
    Optional,
    Build,
    Development,
}

impl DependencyKind {
    /// The manifest section this kind is declared under, or `None` where the
    /// ecosystem has no such section — `Peer` and `Optional` are npm shapes,
    /// `Build` is a Cargo one.
    ///
    /// Cargo's optional dependencies are `optional = true` entries in
    /// `[dependencies]` rather than a section of their own, so a Cargo optional
    /// dependency is [`DependencyKind::Normal`].
    #[must_use]
    pub const fn section(self, ecosystem: Ecosystem) -> Option<&'static str> {
        match (ecosystem, self) {
            (Ecosystem::Npm | Ecosystem::Cargo, Self::Normal) => Some("dependencies"),
            (Ecosystem::Npm, Self::Peer) => Some("peerDependencies"),
            (Ecosystem::Npm, Self::Optional) => Some("optionalDependencies"),
            (Ecosystem::Npm, Self::Development) => Some("devDependencies"),
            (Ecosystem::Cargo, Self::Build) => Some("build-dependencies"),
            (Ecosystem::Cargo, Self::Development) => Some("dev-dependencies"),
            (Ecosystem::Npm, Self::Build) | (Ecosystem::Cargo, Self::Peer | Self::Optional) => None,
        }
    }

    /// Whether releasing the dependency can change what the dependent ships.
    ///
    /// `Build` is the three-way line in Cargo that no surveyed tool draws: a
    /// build script cannot see `[dependencies]` and needs `[build-dependencies]`
    /// to do its work, so a change there can change the compiled output.
    /// release-please merges all three kinds; knope does not handle
    /// `build-dependencies` at all (ADR-0008).
    ///
    /// False does not mean absent. Development edges stay in the graph and their
    /// ranges are rewritten whenever the dependent is released for some other
    /// reason — dropping them would leave stale ranges in published manifests.
    /// The graph is larger than the cascade set, and this is the only thing that
    /// separates them.
    #[must_use]
    pub const fn is_runtime(self) -> bool {
        match self {
            Self::Normal | Self::Peer | Self::Optional | Self::Build => true,
            Self::Development => false,
        }
    }
}

impl fmt::Display for DependencyKind {
    /// For the messages about a kind whose ecosystem has no section to name.
    /// Callers phrase around these rather than prefixing an article: `optional`
    /// is the one reachable kind that would need "an".
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Normal => "normal",
            Self::Peer => "peer",
            Self::Optional => "optional",
            Self::Build => "build",
            Self::Development => "development",
        })
    }
}

/// How far a `workspace:` protocol range follows the package it points at.
///
/// pnpm's publish-time rewrites are what make this a declaration rather than a
/// convention: for a dependency at `1.5.0`, `workspace:*` publishes as an exact
/// `1.5.0`, `workspace:~` as `~1.5.0`, and `workspace:^` as `^1.5.0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tracking {
    /// `workspace:*`, and a bare `workspace:`, which pnpm treats as `workspace:*`.
    Exact,
    Tilde,
    Caret,
}

impl fmt::Display for Tracking {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Exact => "*",
            Self::Tilde => "~",
            Self::Caret => "^",
        })
    }
}

/// A prefix that replaces a range's bounds with a reference resolved at publish
/// time. Both are pnpm, yarn, and bun spellings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeProtocol {
    Workspace,
    Catalog,
}

impl fmt::Display for RangeProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Workspace => "workspace:",
            Self::Catalog => "catalog:",
        })
    }
}

/// The range a dependent declared for an intra-workspace dependency.
///
/// Two things are needed downstream and neither is recoverable from the other:
/// the bounds the published manifest imposes, which ADR-0010 gates the cascade
/// on, and the protocol prefix `version` has to write back.
///
/// Plain bounds are [`Bounds`]: Cargo text via [`Bounds::from_cargo_text`], npm
/// text via [`Bounds::from_npm_text`] (ADR-0026). Never parse npm strings with
/// `VersionReq` — bare `1.5.0` is a caret in Cargo and an exact pin in npm
/// (ADR-0018). Equality is ecosystem-aware for the same reason.
///
/// `workspace:` / `catalog:` arms also carry [`Bounds`]: after oakum peels the
/// protocol, the published range is still npm grammar on npm packages (including
/// `||`). Tracking forms expand at the tag version instead.
///
/// [`DeclaredRange::PathLinked`] is a Cargo path edge with no declared version:
/// there is nothing for ADR-0010's gate to compare, so the planner always
/// cascades (ADR-0026). [`Workspace::new`] refuses ecosystem/range pairings that
/// cannot appear in a real manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeclaredRange {
    /// A plain range: Cargo `^1.5.0` / `=1.5.0`, or npm bare / `||` / hyphen.
    /// npm rejects the workspace protocol outright with `EUNSUPPORTEDPROTOCOL`.
    Plain(Bounds),
    /// `workspace:` carrying its own bounds, as in `workspace:^1.5.0`. Publishes
    /// as those bounds alone, but the prefix has to survive a rewrite: without it
    /// the range resolves against the registry instead of the workspace member.
    Workspace(Bounds),
    /// A `workspace:` form whose bounds come from the dependency's version at
    /// publish time.
    WorkspaceTracking(Tracking),
    /// `catalog:` or `catalog:<name>`, carrying the bounds the catalog names.
    ///
    /// Discovery resolves it, since that is where `pnpm-workspace.yaml` can be
    /// read, and an entry it cannot resolve is an error naming the file rather
    /// than a range that passes: bumpy short-circuits its range check to `true`
    /// for every `catalog:` range while shipping the resolver it declines to
    /// call, which silently under-releases every catalog consumer (ADR-0010).
    /// `bounds` is mandatory to force that resolution, and a `*` here has to come
    /// from the catalog saying `*` rather than from a fallback.
    Catalog {
        /// `None` for the default catalog, which is written as a bare `catalog:`.
        name: Option<String>,
        bounds: Bounds,
    },
    /// Cargo path dependency with no `version` key (ADR-0026). Always cascades.
    PathLinked,
}

impl fmt::Display for DeclaredRange {
    /// Manifest protocol spelling. Catalog bounds are not part of the token
    /// (`catalog:` / `catalog:<name>`); print bounds separately when the
    /// ADR-0010 gate range is needed.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plain(bounds) => write!(f, "{bounds}"),
            Self::Workspace(bounds) => write!(f, "workspace:{bounds}"),
            Self::WorkspaceTracking(tracking) => write!(f, "workspace:{tracking}"),
            Self::Catalog { name: None, .. } => f.write_str("catalog:"),
            Self::Catalog {
                name: Some(name), ..
            } => write!(f, "catalog:{name}"),
            Self::PathLinked => f.write_str("path-linked"),
        }
    }
}

impl DeclaredRange {
    /// The protocol prefix this range is written with, or `None` for a plain one
    /// or a path-linked edge.
    ///
    /// `version` rewrites through the same answer, so the mapping lives here
    /// rather than at its call site. Use [`DeclaredRange::is_path_linked`] before
    /// treating a no-protocol range as a plain `version` line.
    #[must_use]
    pub const fn protocol(&self) -> Option<RangeProtocol> {
        match self {
            Self::Workspace(_) | Self::WorkspaceTracking(_) => Some(RangeProtocol::Workspace),
            Self::Catalog { .. } => Some(RangeProtocol::Catalog),
            Self::Plain(_) | Self::PathLinked => None,
        }
    }

    #[must_use]
    pub const fn is_path_linked(&self) -> bool {
        matches!(self, Self::PathLinked)
    }

    fn expand_tracking(tracking: Tracking, version_at_last_tag: &Version) -> VersionReq {
        VersionReq {
            comparators: Vec::from([Comparator {
                op: match tracking {
                    Tracking::Exact => Op::Exact,
                    Tracking::Tilde => Op::Tilde,
                    Tracking::Caret => Op::Caret,
                },
                major: version_at_last_tag.major,
                minor: Some(version_at_last_tag.minor),
                patch: Some(version_at_last_tag.patch),
                // A prerelease dropped here publishes a pin on a version that was
                // never released.
                pre: version_at_last_tag.pre.clone(),
            }]),
        }
    }

    /// The Cargo-shaped bounds the published manifest imposes, when they exist.
    ///
    /// Returns `None` when there is no `VersionReq` to hand back:
    /// - [`DeclaredRange::PathLinked`] — no published range (always cascade)
    /// - any arm carrying [`Bounds::Npm`] — use [`DeclaredRange::admits`] for
    ///   the gate; do not treat this `None` as path-linked
    ///
    /// `version_at_last_tag` is the dependency's version *before* the bump under
    /// consideration — ADR-0014's tag-derived version, not the one the plan is
    /// proposing. Passing the new version makes every tracking form expand to a
    /// range that trivially contains it, so the gate reads "satisfied" and
    /// nothing cascades.
    ///
    /// The expansion lives here rather than at each call site because the arm
    /// that gets it wrong is the cheap one: a caller matching the variants that
    /// carry Cargo bounds is left with a bounds-free
    /// [`DeclaredRange::WorkspaceTracking`] and nothing to write but `true`.
    /// That is bumpy's recorded bug — it treats `workspace:*` as always
    /// satisfied, which is backwards, since pnpm publishes that form as an exact
    /// version and it is the tightest pin of the three.
    #[must_use]
    pub fn published_req(&self, version_at_last_tag: &Version) -> Option<VersionReq> {
        match self {
            Self::Plain(Bounds::Cargo(req))
            | Self::Workspace(Bounds::Cargo(req))
            | Self::Catalog {
                bounds: Bounds::Cargo(req),
                ..
            } => Some(req.clone()),
            Self::Plain(Bounds::Npm(_))
            | Self::Workspace(Bounds::Npm(_))
            | Self::Catalog {
                bounds: Bounds::Npm(_),
                ..
            }
            | Self::PathLinked => None,
            Self::WorkspaceTracking(tracking) => {
                Some(Self::expand_tracking(*tracking, version_at_last_tag))
            }
        }
    }

    /// [`DeclaredRange::PathLinked`] never admits; ADR-0026 always cascades.
    #[must_use]
    pub fn admits(&self, version_at_last_tag: &Version, candidate: &Version) -> bool {
        match self {
            Self::PathLinked => false,
            Self::Plain(bounds) | Self::Workspace(bounds) | Self::Catalog { bounds, .. } => {
                bounds.matches(candidate)
            }
            Self::WorkspaceTracking(tracking) => {
                Self::expand_tracking(*tracking, version_at_last_tag).matches(candidate)
            }
        }
    }

    const fn fits_ecosystem(&self, ecosystem: Ecosystem) -> bool {
        matches!(
            (ecosystem, self),
            (
                Ecosystem::Cargo,
                Self::PathLinked | Self::Plain(Bounds::Cargo(_))
            ) | (
                Ecosystem::Npm,
                Self::Plain(Bounds::Npm(_))
                    | Self::Workspace(Bounds::Npm(_))
                    | Self::Catalog {
                        bounds: Bounds::Npm(_),
                        ..
                    }
                    | Self::WorkspaceTracking(_)
            )
        )
    }
}

/// One intra-workspace edge.
///
/// Only intra-workspace edges belong in the graph. A dependency on a registry
/// package cannot be released by oakum, and carrying those edges would make every
/// traversal walk the whole transitive dependency set to reach the handful of
/// nodes that can move.
///
/// A manifest line is identified by the section this edge's kind maps to in the
/// dependent's ecosystem, the key it is written under, and the target table
/// holding it. `version` rewrites that line, and `Workspace::new` refuses a
/// repeat of those three, since one line cannot hold two ranges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dependency {
    /// A package in the same ecosystem as the dependent, guaranteed for any
    /// `Dependency` reached through a [`Workspace`], which is where the check
    /// lives — [`Package`] can be built without it.
    pub on: PackageId,
    pub kind: DependencyKind,
    /// The manifest key this edge is written under, which equals `on.name` unless
    /// the dependent renamed it — Cargo's `alias = { package = "real" }`, npm's
    /// `"alias": "npm:real@^1.0.0"`. A renamed edge rewritten by name lands on
    /// the wrong entry, or on none.
    pub declared_as: String,
    /// Cargo's target predicate, as `cargo metadata` reports it — `cfg(windows)`
    /// for a `[target.'cfg(windows)'.dependencies]` entry, `None` for an
    /// unconditional one. Always `None` for npm, which has no equivalent.
    ///
    /// One package can be declared under one key in several target tables with a
    /// different range in each — see `docs/research/cargo-metadata-edge-shapes.md`.
    /// Without this they are indistinguishable, and a rewrite edits whichever
    /// table it finds first.
    pub target: Option<String>,
    pub range: DeclaredRange,
}

/// When a package's dependency versions stop being re-resolvable, which is what
/// decides whether the range gate applies to it (ADR-0009).
///
/// Named for the mechanism rather than the effect, matching the config key.
/// "Delivery artifact" describes the consequence; a package resolving its
/// dependencies at build time is the fact that produces it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolvesDependenciesAt {
    /// A consumer re-resolves at install time, so a dependency fix reaches it
    /// without a release. This is the case the range gate is correct for.
    Install,
    /// Dependencies are resolved once, at build time, and shipped inside the
    /// artifact. Nothing re-resolves afterward, so the range gate must not apply:
    /// a satisfied range means no bump, no bump means no tag, and the artifact
    /// users download keeps the version it was built against.
    Build(BuildResolution),
}

impl ResolvesDependenciesAt {
    /// Exhaustive, so a third variant is a compile error rather than a silent
    /// "not a delivery artifact" — the ADR-0009 under-release this type prevents.
    #[must_use]
    pub const fn is_build(self) -> bool {
        match self {
            Self::Build(_) => true,
            Self::Install => false,
        }
    }
}

/// Why a package was taken to resolve its dependencies at build time. ADR-0009
/// allows exactly these two routes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildResolution {
    /// The package manager reported a binary target: a `bin` target in
    /// `cargo metadata`, a `bin` field in npm. Derived from resolved targets and
    /// never from a `src/main.rs` on disk, which is what makes `autobins = false`
    /// and explicit `[[bin]]` entries come out right.
    BinaryTarget,
    /// `resolves-dependencies-at = "build"` in config. The one case that needs a
    /// declaration: a library that bundles its dependencies into its published
    /// output, and so ships them baked in without producing a binary.
    Declared,
}

/// A package as the planner sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    id: PackageId,
    version: Version,
    resolves_dependencies_at: ResolvesDependenciesAt,
    /// Whether a registry publish is allowed somewhere (ADR-0004 / ADR-0027).
    /// Derived from Cargo `publish` / npm `private`, never configured. Cargo
    /// allow-lists collapse to `true` in v0; restricted registries are not
    /// retained on the plan model until publish lands.
    publishable: bool,
    dependencies: Vec<Dependency>,
    /// Repository-relative directory (no trailing slash). Empty string is the
    /// repository root. Used for commit path fallback; plan math does not read it.
    manifest_dir: String,
}

impl Package {
    #[must_use]
    pub fn new(
        id: PackageId,
        version: Version,
        resolves_dependencies_at: ResolvesDependenciesAt,
        publishable: bool,
        dependencies: Vec<Dependency>,
    ) -> Self {
        Self {
            id,
            version,
            resolves_dependencies_at,
            publishable,
            dependencies,
            manifest_dir: String::new(),
        }
    }

    /// Repository-relative package directory. Empty string is the repository root.
    #[must_use]
    pub fn with_manifest_dir(mut self, dir: impl Into<String>) -> Self {
        self.manifest_dir = dir.into();
        self
    }

    /// Repository-relative directory. Empty string is the repository root.
    #[must_use]
    pub fn manifest_dir(&self) -> &str {
        &self.manifest_dir
    }

    #[must_use]
    pub const fn id(&self) -> &PackageId {
        &self.id
    }

    /// The version the tags say this package is at, not the one in its manifest.
    /// The two differ inside an open version pull request, and the tag is the
    /// source of truth (ADR-0014).
    #[must_use]
    pub const fn version(&self) -> &Version {
        &self.version
    }

    #[must_use]
    pub const fn resolves_dependencies_at(&self) -> ResolvesDependenciesAt {
        self.resolves_dependencies_at
    }

    /// Registry publish allowed. Unrelated to whether oakum versions the package
    /// (ADR-0027: versioning unpublishable packages is opt-in preference).
    #[must_use]
    pub const fn publishable(&self) -> bool {
        self.publishable
    }

    /// Every declared intra-workspace edge, development edges included.
    #[must_use]
    pub fn dependencies(&self) -> &[Dependency] {
        &self.dependencies
    }

    /// The subset of [`Package::dependencies`] a release can cascade along.
    pub fn runtime_dependencies(&self) -> impl Iterator<Item = &Dependency> {
        self.dependencies.iter().filter(|d| d.kind.is_runtime())
    }
}

/// The packages in a repository and the edges between them.
///
/// [`Workspace::new`] is the only place the set is checked, so a [`Package`] or
/// [`Dependency`] reached through a `Workspace` has been through those checks and
/// one built directly has not. Construction refuses any cycle that names two or
/// more packages (every edge kind — ADR-0008). A package depending on itself is
/// allowed: Cargo accepts a path self-dev-dependency, and a self-edge cannot
/// cascade into another package — but it still appears in
/// [`Workspace::runtime_dependents`], so a naive walk can loop; callers need a
/// visited set (or must treat dependent == dependency as already seen).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    packages: BTreeMap<PackageId, Package>,
}

impl Workspace {
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] for a duplicate package, an empty set, or an
    /// edge that is any of the following:
    ///
    /// - onto a package in another ecosystem
    /// - declared under a section its ecosystem does not have
    /// - carrying a target predicate its ecosystem does not have
    /// - written with a range protocol its ecosystem does not have
    /// - carrying bounds (or a path-linked shape) its ecosystem cannot express
    /// - onto a package the workspace does not contain
    /// - a repeat of an existing `(section, key, target)`
    /// - part of a dependency cycle that names two or more packages
    ///
    /// Each is a discovery fault or a manifest mistake that under-cascades
    /// silently if let through: a dependent behind an unresolvable edge is
    /// reachable from no traversal at all, and an empty set makes "discovery
    /// found nothing" indistinguishable from "nothing to release" — never
    /// collapse "we didn't look" into "it's fine".
    ///
    /// The order of the edge checks is the order above, most fundamental first,
    /// so an edge breaking two rules reports the one that explains the other, and
    /// an edge that is itself unusable is reported before its interaction with
    /// any edge beside it. Cycle detection runs after every edge has passed those
    /// checks, so a broken line is never reported as a cycle.
    pub fn new(packages: impl IntoIterator<Item = Package>) -> Result<Self, WorkspaceError> {
        let mut map = BTreeMap::new();
        for package in packages {
            if let Some(existing) = map.insert(package.id.clone(), package) {
                return Err(WorkspaceError::DuplicatePackage { id: existing.id });
            }
        }

        if map.is_empty() {
            return Err(WorkspaceError::Empty);
        }

        for package in map.values() {
            let id = &package.id;
            let mut seen = BTreeSet::new();
            for dependency in &package.dependencies {
                if dependency.on.ecosystem != id.ecosystem {
                    return Err(WorkspaceError::CrossEcosystemDependency {
                        dependent: id.clone(),
                        dependency: dependency.on.clone(),
                    });
                }
                let Some(section) = dependency.kind.section(id.ecosystem) else {
                    return Err(WorkspaceError::KindNotInEcosystem {
                        dependent: id.clone(),
                        dependency: dependency.on.clone(),
                        kind: dependency.kind,
                    });
                };
                if let Some(target) = &dependency.target {
                    if !id.ecosystem.has_target_tables() {
                        return Err(WorkspaceError::TargetNotInEcosystem {
                            dependent: id.clone(),
                            dependency: dependency.on.clone(),
                            target: target.clone(),
                        });
                    }
                }
                if let Some(protocol) = dependency.range.protocol() {
                    if !id.ecosystem.has_range_protocols() {
                        return Err(WorkspaceError::RangeNotInEcosystem {
                            dependent: id.clone(),
                            dependency: dependency.on.clone(),
                            protocol,
                        });
                    }
                }
                if !dependency.range.fits_ecosystem(id.ecosystem) {
                    return Err(WorkspaceError::BoundsNotInEcosystem {
                        dependent: id.clone(),
                        dependency: dependency.on.clone(),
                    });
                }
                if !map.contains_key(&dependency.on) {
                    return Err(WorkspaceError::UnknownDependency {
                        dependent: id.clone(),
                        dependency: dependency.on.clone(),
                    });
                }
                // The manifest line, which the package it points at is no part
                // of: a key is unique within a section and target table whatever
                // it resolves to, so keying on the package would let two of them
                // share one line.
                if !seen.insert((section, &dependency.declared_as, &dependency.target)) {
                    return Err(WorkspaceError::DuplicateEdge {
                        dependent: id.clone(),
                        declared_as: dependency.declared_as.clone(),
                        section,
                        // The only way to reach this with a target set is a
                        // duplicate inside one target table, which is the case
                        // the bare section name describes wrongly.
                        target: dependency.target.clone(),
                    });
                }
            }
        }

        if let Some(path) = multi_package_cycle(&map) {
            return Err(WorkspaceError::Cycle { path });
        }

        Ok(Self { packages: map })
    }

    #[must_use]
    pub fn get(&self, id: &PackageId) -> Option<&Package> {
        self.packages.get(id)
    }

    /// Ordered by [`PackageId`], so a plan built from this workspace is
    /// reproducible.
    pub fn packages(&self) -> impl Iterator<Item = &Package> {
        self.packages.values()
    }

    /// Every package declaring an edge on `id`, paired with the edge itself.
    ///
    /// One item per edge, so a package appears more than once when it declares
    /// the same dependency under more than one section, key, or target table —
    /// `peerDependencies` plus `devDependencies` is the ordinary way to write an
    /// npm plugin. Callers that bump have to reach a package once however many
    /// edges led them there.
    pub fn dependents<'a, 'b>(
        &'a self,
        id: &'b PackageId,
    ) -> impl Iterator<Item = (&'a Package, &'a Dependency)> + use<'a, 'b> {
        self.packages.values().flat_map(move |package| {
            package
                .dependencies
                .iter()
                .filter(move |d| &d.on == id)
                .map(move |d| (package, d))
        })
    }

    /// The subset of [`Workspace::dependents`] a release can cascade along.
    pub fn runtime_dependents<'a, 'b>(
        &'a self,
        id: &'b PackageId,
    ) -> impl Iterator<Item = (&'a Package, &'a Dependency)> + use<'a, 'b> {
        self.dependents(id)
            .filter(|(_, dependency)| dependency.kind.is_runtime())
    }
}

/// A workspace that cannot be planned over, reported before any planning starts.
///
/// Messages name packages and manifest keys rather than files. Discovery holds
/// the paths and is where a location belongs in a message; by the time a
/// [`Package`] exists the path is deliberately gone, so nothing in the planner
/// can reach for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceError {
    Empty,
    DuplicatePackage {
        id: PackageId,
    },
    CrossEcosystemDependency {
        dependent: PackageId,
        dependency: PackageId,
    },
    KindNotInEcosystem {
        dependent: PackageId,
        dependency: PackageId,
        kind: DependencyKind,
    },
    TargetNotInEcosystem {
        dependent: PackageId,
        dependency: PackageId,
        target: String,
    },
    RangeNotInEcosystem {
        dependent: PackageId,
        dependency: PackageId,
        protocol: RangeProtocol,
    },
    /// Bounds or path-linked shape that cannot appear on this ecosystem
    /// (ADR-0026): e.g. [`DeclaredRange::PathLinked`] on npm, or
    /// [`Bounds::Npm`] on a Cargo plain.
    BoundsNotInEcosystem {
        dependent: PackageId,
        dependency: PackageId,
    },
    UnknownDependency {
        dependent: PackageId,
        dependency: PackageId,
    },
    DuplicateEdge {
        dependent: PackageId,
        declared_as: String,
        section: &'static str,
        target: Option<String>,
    },
    /// A dependency cycle naming two or more packages. `path` is the cycle
    /// walk with the first id repeated at the end (`a → b → a`).
    Cycle {
        path: Vec<PackageId>,
    },
}

impl fmt::Display for WorkspaceError {
    // Each variant needs its own wording; folding them into helpers obscures the
    // message that discovery surfaces.
    #[expect(clippy::too_many_lines)]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str(
                "discovery reported no packages; a workspace file above the repository root can \
                 reparent the workspace, so check which root the package manager resolved",
            ),
            Self::DuplicatePackage { id } => {
                write!(f, "the workspace reports {id} more than once")
            }
            Self::CrossEcosystemDependency {
                dependent,
                dependency,
            } => write!(
                f,
                "{dependent} declares an edge on {dependency}, which no manifest can express"
            ),
            Self::KindNotInEcosystem {
                dependent,
                dependency,
                kind,
            } => {
                write!(
                    f,
                    "{dependent} declares {dependency} with kind {kind}, which {} has no section for",
                    dependent.ecosystem
                )?;
                // Cargo does have optional dependencies, so the bare message
                // reads as "unsupported" when the fault is a mis-mapped kind.
                if (dependent.ecosystem, kind) == (Ecosystem::Cargo, &DependencyKind::Optional) {
                    f.write_str(
                        "; Cargo declares optional dependencies as `optional = true` entries in \
                         [dependencies], which is the normal kind",
                    )?;
                }
                Ok(())
            }
            Self::TargetNotInEcosystem {
                dependent,
                dependency,
                target,
            } => write!(
                f,
                "{dependent} declares {dependency} under target `{target}`, which {} has no tables for",
                dependent.ecosystem
            ),
            Self::RangeNotInEcosystem {
                dependent,
                dependency,
                protocol,
            } => {
                write!(
                    f,
                    "{dependent} declares {dependency} with a `{protocol}` range, which {} has no \
                     protocol for",
                    dependent.ecosystem
                )?;
                // Cargo is the only ecosystem this variant can name today, so the
                // ecosystem half is pre-armed rather than live: a third one
                // without range protocols must not be handed Cargo's advice.
                if (dependent.ecosystem, protocol) == (Ecosystem::Cargo, &RangeProtocol::Workspace)
                {
                    f.write_str(
                        "; Cargo's `workspace = true` inheritance is a different mechanism and \
                         reaches the planner as a plain range",
                    )?;
                }
                Ok(())
            }
            Self::BoundsNotInEcosystem {
                dependent,
                dependency,
            } => write!(
                f,
                "{dependent} declares {dependency} with a range shape {} cannot express",
                dependent.ecosystem
            ),
            Self::UnknownDependency {
                dependent,
                dependency,
            } => write!(
                f,
                "{dependent} depends on {dependency}, which the workspace does not contain"
            ),
            Self::DuplicateEdge {
                dependent,
                declared_as,
                section,
                target,
            } => match target {
                Some(target) => write!(
                    f,
                    "{dependent} declares `{declared_as}` more than once in \
                     [target.'{target}'.{section}]"
                ),
                None => write!(
                    f,
                    "{dependent} declares `{declared_as}` more than once in {section}"
                ),
            },
            Self::Cycle { path } => {
                debug_assert!(
                    path.len() >= 3 && path.first() == path.last(),
                    "Cycle.path must be a closed multi-package walk"
                );
                f.write_str("found cycle in dependency graph: ")?;
                for (i, id) in path.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" -> ")?;
                    }
                    write!(f, "{id}")?;
                }
                Ok(())
            }
        }
    }
}

/// Walks every edge (runtime and development). Returns a cycle path that names
/// two or more packages, with the start id repeated at the end. Self-edges alone
/// are ignored — Cargo accepts a path self-dev-dependency (ADR-0008).
fn multi_package_cycle(packages: &BTreeMap<PackageId, Package>) -> Option<Vec<PackageId>> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Color {
        White,
        Gray,
        Black,
    }

    fn visit(
        id: &PackageId,
        packages: &BTreeMap<PackageId, Package>,
        color: &mut BTreeMap<PackageId, Color>,
        path: &mut Vec<PackageId>,
    ) -> Option<Vec<PackageId>> {
        color.insert(id.clone(), Color::Gray);
        path.push(id.clone());

        if let Some(package) = packages.get(id) {
            for dependency in &package.dependencies {
                let next = &dependency.on;
                match color.get(next).copied().unwrap_or(Color::White) {
                    Color::Gray => {
                        let start = path
                            .iter()
                            .position(|step| step == next)
                            .expect("gray node must be on the DFS path");
                        // Self-edge: path[start..] is one node before the close.
                        if path.len() - start < 2 {
                            continue;
                        }
                        let mut cycle: Vec<PackageId> = path[start..].to_vec();
                        cycle.push(next.clone());
                        return Some(cycle);
                    }
                    Color::White => {
                        if let Some(cycle) = visit(next, packages, color, path) {
                            return Some(cycle);
                        }
                    }
                    Color::Black => {}
                }
            }
        }

        path.pop();
        color.insert(id.clone(), Color::Black);
        None
    }

    let mut color = BTreeMap::new();
    for id in packages.keys() {
        color.insert(id.clone(), Color::White);
    }

    let mut path = Vec::new();
    for id in packages.keys() {
        if color.get(id).copied() != Some(Color::White) {
            continue;
        }
        if let Some(cycle) = visit(id, packages, &mut color, &mut path) {
            return Some(cycle);
        }
    }
    None
}

impl core::error::Error for WorkspaceError {}

// Held to `no_std` along with the rest of the module: `--all-targets` builds
// this through `crates/plan-no-std`, so a fixture reaching for `HashMap` or
// `Box` fails there rather than here.
#[cfg(test)]
mod tests {
    use alloc::format;
    use alloc::string::ToString;
    use alloc::vec;

    use super::*;

    fn cargo(name: &str) -> PackageId {
        PackageId::new(Ecosystem::Cargo, name)
    }

    fn npm(name: &str) -> PackageId {
        PackageId::new(Ecosystem::Npm, name)
    }

    fn req(text: &str) -> VersionReq {
        VersionReq::parse(text).expect("test range should parse")
    }

    fn plain_cargo(text: &str) -> DeclaredRange {
        DeclaredRange::Plain(Bounds::from_cargo_text(text).expect("test cargo range"))
    }

    fn plain_npm(text: &str) -> DeclaredRange {
        DeclaredRange::Plain(Bounds::from_npm_text(text).expect("test npm range"))
    }

    fn workspace_npm(text: &str) -> DeclaredRange {
        DeclaredRange::Workspace(Bounds::from_npm_text(text).expect("test npm range"))
    }

    fn catalog_npm(name: Option<&str>, text: &str) -> DeclaredRange {
        DeclaredRange::Catalog {
            name: name.map(str::to_string),
            bounds: Bounds::from_npm_text(text).expect("test npm range"),
        }
    }

    fn edge(on: PackageId, kind: DependencyKind) -> Dependency {
        let declared_as = on.name.clone();
        let range = match on.ecosystem {
            Ecosystem::Cargo => plain_cargo("^1.0.0"),
            Ecosystem::Npm => plain_npm("^1.0.0"),
        };
        Dependency {
            on,
            kind,
            declared_as,
            target: None,
            range,
        }
    }

    fn package(id: PackageId, dependencies: Vec<Dependency>) -> Package {
        Package::new(
            id,
            Version::new(1, 0, 0),
            ResolvesDependenciesAt::Install,
            true,
            dependencies,
        )
    }

    #[test]
    fn a_duplicate_package_is_refused() {
        let error = Workspace::new([
            package(cargo("core"), vec![]),
            package(cargo("core"), vec![]),
        ])
        .expect_err("two packages under one id should not build a workspace");

        assert_eq!(
            error,
            WorkspaceError::DuplicatePackage { id: cargo("core") }
        );
        assert_eq!(
            error.to_string(),
            "the workspace reports core (cargo) more than once"
        );
    }

    /// Discovery returning nothing and a repository with nothing to release are
    /// different facts. pnpm reports the first as exit 0 with an empty list when
    /// a stray ancestor reparents the workspace, so the message has to name that.
    #[test]
    fn an_empty_workspace_is_refused_and_names_the_likely_cause() {
        let error = Workspace::new([]).expect_err("an empty workspace should be refused");

        assert_eq!(error, WorkspaceError::Empty);
        assert!(
            error.to_string().contains("reparent the workspace"),
            "message gives the reader nothing to check: {error}"
        );
    }

    /// An edge onto a package the workspace does not contain reaches nothing, so
    /// accepting it means a dependent that no traversal can find.
    #[test]
    fn an_edge_onto_a_package_outside_the_workspace_is_refused_by_name() {
        let error = Workspace::new([package(
            cargo("cli"),
            vec![edge(cargo("core"), DependencyKind::Normal)],
        )])
        .expect_err("an unresolvable edge should not build a workspace");

        // The whole rendered string, not that both names appear somewhere: the
        // message reads the same either way round, and one naming the two ends
        // backwards sends the reader to the wrong manifest.
        assert_eq!(
            error.to_string(),
            "cli (cargo) depends on core (cargo), which the workspace does not contain"
        );
        assert_eq!(
            error,
            WorkspaceError::UnknownDependency {
                dependent: cargo("cli"),
                dependency: cargo("core"),
            }
        );
    }

    /// See [`PackageId`]: keyed by name alone the two collapse into one node.
    #[test]
    fn one_name_in_two_ecosystems_is_two_packages() {
        let workspace = Workspace::new([
            package(cargo("oakum"), vec![]),
            package(npm("oakum"), vec![]),
        ])
        .expect("distinct ecosystems should not collide");

        assert_eq!(workspace.packages().count(), 2);
        // `is_some()` would hold whichever of the two came back.
        assert_eq!(
            workspace.get(&cargo("oakum")).map(Package::id),
            Some(&cargo("oakum"))
        );

        let cargo_only = Workspace::new([package(cargo("oakum"), vec![])]).expect("should build");
        assert!(cargo_only.get(&npm("oakum")).is_none());
    }

    /// The other half of that separation. Distinct ids stop the packages merging;
    /// only this stops an edge crossing between them.
    #[test]
    fn an_edge_across_ecosystems_is_refused() {
        let error = Workspace::new([
            package(cargo("core"), vec![]),
            package(
                npm("cli"),
                vec![edge(cargo("core"), DependencyKind::Normal)],
            ),
        ])
        .expect_err("a cross-ecosystem edge should be refused");

        assert_eq!(
            error.to_string(),
            "cli (npm) declares an edge on core (cargo), which no manifest can express"
        );
        assert_eq!(
            error,
            WorkspaceError::CrossEcosystemDependency {
                dependent: npm("cli"),
                dependency: cargo("core"),
            }
        );
    }

    /// Lookups have to match the whole id, not the name.
    #[test]
    fn dependents_does_not_match_across_ecosystems() {
        let workspace = Workspace::new([
            package(npm("shared"), vec![]),
            package(cargo("shared"), vec![]),
            package(
                cargo("cli"),
                vec![edge(cargo("shared"), DependencyKind::Normal)],
            ),
        ])
        .expect("workspace should build");

        let found: Vec<&PackageId> = workspace
            .dependents(&cargo("shared"))
            .map(|(package, _)| package.id())
            .collect();
        assert_eq!(found, vec![&cargo("cli")]);
        assert_eq!(workspace.dependents(&npm("shared")).count(), 0);
    }

    /// ADR-0008's separation, in both directions: the edge stays in the graph so
    /// its range is still rewritten, and out of the cascade set so it never bumps.
    #[test]
    fn a_development_edge_stays_in_the_graph_and_out_of_the_cascade_set() {
        let workspace = Workspace::new([
            package(cargo("config"), vec![]),
            package(
                cargo("cli"),
                vec![edge(cargo("config"), DependencyKind::Development)],
            ),
        ])
        .expect("a development edge should build a workspace");

        let cli = workspace.get(&cargo("cli")).expect("cli should be present");
        assert_eq!(cli.dependencies().len(), 1);
        assert_eq!(cli.runtime_dependencies().count(), 0);

        let dependents: Vec<&PackageId> = workspace
            .dependents(&cargo("config"))
            .map(|(package, _)| package.id())
            .collect();
        assert_eq!(dependents, vec![&cargo("cli")]);
        assert_eq!(workspace.runtime_dependents(&cargo("config")).count(), 0);
    }

    /// See [`DependencyKind::is_runtime`] for why `Build` is on the cascade side.
    #[test]
    fn every_kind_lands_on_the_side_adr_0008_puts_it() {
        for kind in [
            DependencyKind::Normal,
            DependencyKind::Peer,
            DependencyKind::Optional,
            DependencyKind::Build,
        ] {
            assert!(kind.is_runtime(), "{kind:?} should cascade");
        }
        assert!(!DependencyKind::Development.is_runtime());
    }

    /// The table `version` writes through. Three of the ten pairings are nothing;
    /// see [`DependencyKind::section`] for why Cargo has no optional section.
    #[test]
    fn every_kind_maps_to_the_section_its_ecosystem_writes() {
        use DependencyKind::{Build, Development, Normal, Optional, Peer};

        for (ecosystem, kind, section) in [
            (Ecosystem::Npm, Normal, Some("dependencies")),
            (Ecosystem::Npm, Peer, Some("peerDependencies")),
            (Ecosystem::Npm, Optional, Some("optionalDependencies")),
            (Ecosystem::Npm, Development, Some("devDependencies")),
            (Ecosystem::Npm, Build, None),
            (Ecosystem::Cargo, Normal, Some("dependencies")),
            (Ecosystem::Cargo, Build, Some("build-dependencies")),
            (Ecosystem::Cargo, Development, Some("dev-dependencies")),
            (Ecosystem::Cargo, Peer, None),
            (Ecosystem::Cargo, Optional, None),
        ] {
            assert_eq!(kind.section(ecosystem), section, "{ecosystem} {kind:?}");
        }
    }

    /// A kind its ecosystem has no section for would be written back nowhere.
    #[test]
    fn a_kind_the_ecosystem_has_no_section_for_is_refused() {
        let error = Workspace::new([
            package(cargo("core"), vec![]),
            package(
                cargo("cli"),
                vec![edge(cargo("core"), DependencyKind::Peer)],
            ),
        ])
        .expect_err("a Cargo peer dependency should be refused");

        assert_eq!(
            error.to_string(),
            "cli (cargo) declares core (cargo) with kind peer, which cargo has no section for"
        );
        assert_eq!(
            error,
            WorkspaceError::KindNotInEcosystem {
                dependent: cargo("cli"),
                dependency: cargo("core"),
                kind: DependencyKind::Peer,
            }
        );
    }

    /// Declaring a package as both a peer and a development dependency is the
    /// ordinary way to write an npm plugin, so a dependent arrives twice.
    #[test]
    fn a_package_declared_in_two_sections_yields_one_edge_each() {
        let workspace = Workspace::new([
            package(npm("host"), vec![]),
            package(
                npm("plugin"),
                vec![
                    edge(npm("host"), DependencyKind::Peer),
                    edge(npm("host"), DependencyKind::Development),
                ],
            ),
        ])
        .expect("two sections naming one dependency should build a workspace");

        let kinds: Vec<DependencyKind> = workspace
            .dependents(&npm("host"))
            .map(|(_, dependency)| dependency.kind)
            .collect();
        assert_eq!(
            kinds,
            vec![DependencyKind::Peer, DependencyKind::Development]
        );
        assert_eq!(workspace.runtime_dependents(&npm("host")).count(), 1);
    }

    /// Two entries under one key, one section, and one target table are two
    /// ranges for one manifest line, and whichever the gate reads decides whether
    /// the cascade fires.
    #[test]
    fn one_dependency_declared_twice_under_one_key_is_refused() {
        let mut tighter = edge(cargo("core"), DependencyKind::Normal);
        tighter.range = plain_cargo("=1.0.0");

        let error = Workspace::new([
            package(cargo("core"), vec![]),
            package(
                cargo("cli"),
                vec![edge(cargo("core"), DependencyKind::Normal), tighter],
            ),
        ])
        .expect_err("two ranges for one entry should be refused");

        assert_eq!(
            error.to_string(),
            "cli (cargo) declares `core` more than once in dependencies"
        );
        assert_eq!(
            error,
            WorkspaceError::DuplicateEdge {
                dependent: cargo("cli"),
                declared_as: "core".to_string(),
                section: "dependencies",
                target: None,
            }
        );
    }

    /// Two packages behind one key is a state no manifest can hold — the shape a
    /// broken alias resolver produces.
    #[test]
    fn two_packages_under_one_key_is_refused() {
        let mut collides = edge(cargo("utils"), DependencyKind::Normal);
        collides.declared_as = "shared".to_string();
        let mut first = edge(cargo("core"), DependencyKind::Normal);
        first.declared_as = "shared".to_string();

        let error = Workspace::new([
            package(cargo("core"), vec![]),
            package(cargo("utils"), vec![]),
            package(cargo("cli"), vec![first, collides]),
        ])
        .expect_err("one key naming two packages should be refused");

        assert_eq!(
            error,
            WorkspaceError::DuplicateEdge {
                dependent: cargo("cli"),
                declared_as: "shared".to_string(),
                section: "dependencies",
                target: None,
            }
        );
    }

    /// Two entries onto one package under different keys. Cargo builds this only
    /// while the aliased entry is an unenabled optional dependency, and npm
    /// aliases are ordinary separate keys — either way the uniqueness rule must
    /// not refuse it. See `docs/research/cargo-metadata-edge-shapes.md`.
    #[test]
    fn a_renamed_edge_alongside_the_plain_one_is_not_a_duplicate() {
        let mut renamed = edge(cargo("core"), DependencyKind::Normal);
        renamed.declared_as = "core_alias".to_string();

        let workspace = Workspace::new([
            package(cargo("core"), vec![]),
            package(
                cargo("cli"),
                vec![edge(cargo("core"), DependencyKind::Normal), renamed],
            ),
        ])
        .expect("a rename beside the plain entry should build a workspace");

        assert_eq!(workspace.dependents(&cargo("core")).count(), 2);
    }

    /// Verified against `cargo metadata`: one key under `cfg(unix)` and
    /// `cfg(windows)` reports two entries with different ranges, identical in
    /// every other field. Keying without the target refuses that legal manifest.
    #[test]
    fn one_key_in_two_target_tables_is_not_a_duplicate() {
        let mut unix = edge(cargo("core"), DependencyKind::Normal);
        unix.target = Some("cfg(unix)".to_string());
        let mut windows = edge(cargo("core"), DependencyKind::Normal);
        windows.target = Some("cfg(windows)".to_string());
        windows.range = plain_cargo("=1.0.0");

        let workspace = Workspace::new([
            package(cargo("core"), vec![]),
            package(cargo("cli"), vec![unix, windows]),
        ])
        .expect("two target tables should build a workspace");

        assert_eq!(workspace.runtime_dependents(&cargo("core")).count(), 2);
    }

    /// `workspace:` and `catalog:` are pnpm, yarn, and bun protocols. Accepting one
    /// on a Cargo package would have `version` write `workspace:^1.5.0` into a
    /// `Cargo.toml`, and `published_req` expand it as though pnpm had rewritten it.
    #[test]
    fn a_range_protocol_the_ecosystem_does_not_have_is_refused() {
        // Cargo has workspace dependencies, so the `workspace:` message carries a
        // clause saying which mechanism is being refused.
        let inheritance = "; Cargo's `workspace = true` inheritance is a different \
                           mechanism and reaches the planner as a plain range";

        for (range, protocol, tail) in [
            (
                DeclaredRange::WorkspaceTracking(Tracking::Caret),
                RangeProtocol::Workspace,
                inheritance,
            ),
            (
                workspace_npm("^1.0.0"),
                RangeProtocol::Workspace,
                inheritance,
            ),
            (catalog_npm(None, "^1.0.0"), RangeProtocol::Catalog, ""),
        ] {
            let mut dependency = edge(cargo("core"), DependencyKind::Normal);
            dependency.range = range;

            let error = Workspace::new([
                package(cargo("core"), vec![]),
                package(cargo("cli"), vec![dependency]),
            ])
            .expect_err("a protocol range on a Cargo package should be refused");

            assert_eq!(
                error,
                WorkspaceError::RangeNotInEcosystem {
                    dependent: cargo("cli"),
                    dependency: cargo("core"),
                    protocol,
                }
            );
            // Named ends, not both names alone: swapped, the message sends
            // the reader to core's manifest when the bad range is in cli's.
            assert_eq!(
                error.to_string(),
                format!(
                    "cli (cargo) declares core (cargo) with a `{protocol}` range, \
                     which cargo has no protocol for{tail}"
                )
            );
        }

        // The pre-armed half of that guard, which `Workspace::new` cannot reach
        // while Cargo is the only ecosystem without range protocols.
        assert_eq!(
            WorkspaceError::RangeNotInEcosystem {
                dependent: npm("cli"),
                dependency: npm("core"),
                protocol: RangeProtocol::Workspace,
            }
            .to_string(),
            "cli (npm) declares core (npm) with a `workspace:` range, which npm has no protocol for"
        );

        // The same range on an npm package is the whole point of the variant.
        for range in [
            DeclaredRange::WorkspaceTracking(Tracking::Caret),
            workspace_npm("^1.0.0"),
            catalog_npm(None, "^1.0.0"),
            catalog_npm(Some("react18"), "^1.0.0 || ^2.0.0"),
        ] {
            Workspace::new([
                package(npm("core"), vec![]),
                package(npm("cli"), {
                    let mut d = edge(npm("core"), DependencyKind::Normal);
                    d.range = range;
                    vec![d]
                }),
            ])
            .expect("npm protocol / tracking ranges should build");
        }
    }

    /// Multi-package cycles refuse at construction (any edge kind). A self-edge
    /// alone is allowed: Cargo accepts `path = "."` under `[dev-dependencies]`.
    #[test]
    fn a_multi_package_cycle_is_refused_and_a_self_edge_is_not() {
        for kind in [DependencyKind::Normal, DependencyKind::Development] {
            Workspace::new([package(cargo("core"), vec![edge(cargo("core"), kind)])])
                .unwrap_or_else(|e| panic!("a {kind} self-edge should build: {e}"));
        }

        for kind in [DependencyKind::Normal, DependencyKind::Development] {
            let error = Workspace::new([
                package(cargo("a"), vec![edge(cargo("b"), kind)]),
                package(cargo("b"), vec![edge(cargo("a"), kind)]),
            ])
            .expect_err("a two-node cycle should refuse");
            match &error {
                WorkspaceError::Cycle { path } => {
                    assert!(path.len() >= 3, "{path:?}");
                    assert_eq!(path.first(), path.last());
                    let distinct: BTreeSet<_> = path.iter().collect();
                    assert_eq!(distinct.len(), 2);
                }
                other => panic!("expected Cycle, got {other:?}"),
            }
            assert!(
                error
                    .to_string()
                    .starts_with("found cycle in dependency graph: "),
                "{error}"
            );
        }

        // Self-edge still appears in runtime_dependents; callers must not loop.
        let workspace = Workspace::new([package(
            cargo("core"),
            vec![edge(cargo("core"), DependencyKind::Normal)],
        )])
        .expect("workspace should build");
        let reached: Vec<&PackageId> = workspace
            .runtime_dependents(&cargo("core"))
            .map(|(package, _)| package.id())
            .collect();
        assert_eq!(reached, vec![&cargo("core")]);
    }

    #[test]
    fn a_development_cycle_is_refused_like_a_runtime_one() {
        // release-please #2452 shape: two packages that only devDepend on each other.
        let error = Workspace::new([
            package(
                npm("eslint-plugin-treekeeper"),
                vec![edge(
                    npm("eslint-plugin-node-specifier"),
                    DependencyKind::Development,
                )],
            ),
            package(
                npm("eslint-plugin-node-specifier"),
                vec![edge(
                    npm("eslint-plugin-treekeeper"),
                    DependencyKind::Development,
                )],
            ),
        ])
        .expect_err("a development-only cycle should refuse");

        assert!(matches!(error, WorkspaceError::Cycle { .. }));
        let message = error.to_string();
        assert!(
            message.contains("eslint-plugin-treekeeper")
                && message.contains("eslint-plugin-node-specifier"),
            "{message}"
        );
    }

    #[test]
    fn a_three_node_cycle_names_every_package() {
        let error = Workspace::new([
            package(cargo("a"), vec![edge(cargo("b"), DependencyKind::Normal)]),
            package(cargo("b"), vec![edge(cargo("c"), DependencyKind::Build)]),
            package(
                cargo("c"),
                vec![edge(cargo("a"), DependencyKind::Development)],
            ),
        ])
        .expect_err("a three-node mixed-kind cycle should refuse");

        let WorkspaceError::Cycle { path } = error else {
            panic!("expected Cycle");
        };
        assert_eq!(path.len(), 4);
        assert_eq!(path.first(), path.last());
        let distinct: BTreeSet<_> = path.iter().cloned().collect();
        assert_eq!(
            distinct,
            BTreeSet::from([cargo("a"), cargo("b"), cargo("c")])
        );
    }

    #[test]
    fn an_unknown_dependency_is_reported_before_a_cycle() {
        let error = Workspace::new([
            package(
                cargo("a"),
                vec![
                    edge(cargo("b"), DependencyKind::Normal),
                    edge(cargo("absent"), DependencyKind::Normal),
                ],
            ),
            package(cargo("b"), vec![edge(cargo("a"), DependencyKind::Normal)]),
        ])
        .expect_err("unknown dependency should win over the a↔b cycle");

        assert_eq!(
            error,
            WorkspaceError::UnknownDependency {
                dependent: cargo("a"),
                dependency: cargo("absent"),
            }
        );
    }

    #[test]
    fn a_mixed_kind_two_node_cycle_is_refused() {
        let error = Workspace::new([
            package(cargo("a"), vec![edge(cargo("b"), DependencyKind::Normal)]),
            package(
                cargo("b"),
                vec![edge(cargo("a"), DependencyKind::Development)],
            ),
        ])
        .expect_err("runtime↔development between the same pair is still a cycle");

        assert!(matches!(error, WorkspaceError::Cycle { .. }));
    }

    #[test]
    fn a_self_edge_does_not_hide_a_multi_package_cycle() {
        let error = Workspace::new([
            package(
                cargo("a"),
                vec![
                    edge(cargo("a"), DependencyKind::Development),
                    edge(cargo("b"), DependencyKind::Normal),
                ],
            ),
            package(cargo("b"), vec![edge(cargo("a"), DependencyKind::Normal)]),
        ])
        .expect_err("self-edge plus a↔b must still refuse");

        let WorkspaceError::Cycle { path } = error else {
            panic!("expected Cycle");
        };
        let distinct: BTreeSet<_> = path.iter().collect();
        assert!(distinct.contains(&&cargo("a")) && distinct.contains(&&cargo("b")));
    }

    #[test]
    fn a_diamond_is_not_a_cycle() {
        Workspace::new([
            package(
                cargo("a"),
                vec![
                    edge(cargo("b"), DependencyKind::Normal),
                    edge(cargo("c"), DependencyKind::Normal),
                ],
            ),
            package(cargo("b"), vec![edge(cargo("d"), DependencyKind::Normal)]),
            package(cargo("c"), vec![edge(cargo("d"), DependencyKind::Normal)]),
            package(cargo("d"), vec![]),
        ])
        .expect("a diamond DAG should build");
    }

    /// An edge can break more than one rule at once, and the report has to be the
    /// fault that explains the others — otherwise the reader is sent to add a
    /// package that would still be rejected, or to a section that is not the
    /// problem.
    ///
    /// The duplicate pairing needs a *pair* of edges, not one edge with two
    /// faults: the duplicate check only runs on an edge whose key-mate already
    /// passed everything, so a fixture repeating one broken edge trips the other
    /// fault first and pins nothing.
    #[test]
    fn a_two_fault_edge_reports_the_fault_that_explains_the_other() {
        let keyed = |on: PackageId| {
            let mut dependency = edge(on, DependencyKind::Normal);
            dependency.declared_as = "shared".to_string();
            dependency
        };

        // Cross-ecosystem outranks no-such-section: an edge into another
        // ecosystem is not one a manifest could hold whatever section it names.
        assert!(matches!(
            Workspace::new([package(
                npm("cli"),
                vec![edge(cargo("core"), DependencyKind::Build)]
            )]),
            Err(WorkspaceError::CrossEcosystemDependency { .. })
        ));

        // No-such-section outranks unresolvable.
        assert!(matches!(
            Workspace::new([package(
                cargo("cli"),
                vec![edge(cargo("absent"), DependencyKind::Peer)]
            )]),
            Err(WorkspaceError::KindNotInEcosystem { .. })
        ));

        // Unresolvable outranks duplicate: reporting the duplicate would hide the
        // missing package behind a complaint about how often it is named.
        assert!(matches!(
            Workspace::new([
                package(cargo("core"), vec![]),
                package(
                    cargo("cli"),
                    vec![keyed(cargo("core")), keyed(cargo("absent"))]
                ),
            ]),
            Err(WorkspaceError::UnknownDependency { .. })
        ));

        // Cross-ecosystem outranks the missing target table, which is the same
        // ordering the section check gets and for the same reason.
        let mut targeted = edge(cargo("core"), DependencyKind::Normal);
        targeted.target = Some("cfg(windows)".to_string());
        assert!(matches!(
            Workspace::new([package(npm("cli"), vec![targeted])]),
            Err(WorkspaceError::CrossEcosystemDependency { .. })
        ));

        // No-such-section outranks the range protocol: an edge with no section to
        // live in is not yet a question about how its range is spelled.
        let mut sectionless = edge(cargo("core"), DependencyKind::Peer);
        sectionless.range = DeclaredRange::WorkspaceTracking(Tracking::Caret);
        assert!(matches!(
            Workspace::new([
                package(cargo("core"), vec![]),
                package(cargo("cli"), vec![sectionless]),
            ]),
            Err(WorkspaceError::KindNotInEcosystem { .. })
        ));

        // Cross-ecosystem outranks it too — and this ordering also keeps the
        // message honest, since it renders the dependent's ecosystem.
        let mut crossed = edge(npm("core"), DependencyKind::Normal);
        crossed.range = DeclaredRange::WorkspaceTracking(Tracking::Caret);
        assert!(matches!(
            Workspace::new([
                package(npm("core"), vec![]),
                package(cargo("cli"), vec![crossed]),
            ]),
            Err(WorkspaceError::CrossEcosystemDependency { .. })
        ));

        // No-such-section outranks the missing target table: an edge with no
        // section to live in is not yet a question about which table holds it.
        let mut sectionless = edge(npm("core"), DependencyKind::Build);
        sectionless.target = Some("cfg(windows)".to_string());
        assert!(matches!(
            Workspace::new([
                package(npm("core"), vec![]),
                package(npm("cli"), vec![sectionless]),
            ]),
            Err(WorkspaceError::KindNotInEcosystem { .. })
        ));

        // An unusable range protocol outranks unresolvable, on the same reasoning.
        let mut protocol_on_absent = edge(cargo("absent"), DependencyKind::Normal);
        protocol_on_absent.range = DeclaredRange::WorkspaceTracking(Tracking::Caret);
        assert!(matches!(
            Workspace::new([package(cargo("cli"), vec![protocol_on_absent])]),
            Err(WorkspaceError::RangeNotInEcosystem { .. })
        ));

        // And the missing target table outranks unresolvable: an npm edge cannot
        // carry one whether or not the package it names exists.
        let mut absent = edge(npm("absent"), DependencyKind::Normal);
        absent.target = Some("cfg(windows)".to_string());
        assert!(matches!(
            Workspace::new([package(npm("cli"), vec![absent])]),
            Err(WorkspaceError::TargetNotInEcosystem { .. })
        ));
    }

    /// npm has no target tables, so an edge carrying one is a discovery fault —
    /// and it widens the uniqueness key, letting two ranges through for one
    /// `package.json` entry.
    #[test]
    fn an_npm_edge_carrying_a_target_is_refused() {
        let mut targeted = edge(npm("core"), DependencyKind::Normal);
        targeted.target = Some("cfg(windows)".to_string());

        let error = Workspace::new([
            package(npm("core"), vec![]),
            package(npm("cli"), vec![targeted]),
        ])
        .expect_err("an npm target should be refused");

        assert_eq!(
            error.to_string(),
            "cli (npm) declares core (npm) under target `cfg(windows)`, which npm has no tables for"
        );
        assert_eq!(
            error,
            WorkspaceError::TargetNotInEcosystem {
                dependent: npm("cli"),
                dependency: npm("core"),
                target: "cfg(windows)".to_string(),
            }
        );
    }

    /// The section is derived, so a duplicate outside the default one has to name
    /// the table `version` would actually edit.
    #[test]
    fn a_duplicate_names_the_section_and_target_table_it_fired_in() {
        let npm_dev = Workspace::new([
            package(npm("core"), vec![]),
            package(
                npm("cli"),
                vec![
                    edge(npm("core"), DependencyKind::Development),
                    edge(npm("core"), DependencyKind::Development),
                ],
            ),
        ])
        .expect_err("a repeated npm development edge should be refused");
        assert_eq!(
            npm_dev.to_string(),
            "cli (npm) declares `core` more than once in devDependencies"
        );

        let mut windows = edge(cargo("core"), DependencyKind::Normal);
        windows.target = Some("cfg(windows)".to_string());
        let in_target = Workspace::new([
            package(cargo("core"), vec![]),
            package(cargo("cli"), vec![windows.clone(), windows]),
        ])
        .expect_err("a repeated edge inside one target table should be refused");
        assert_eq!(
            in_target.to_string(),
            "cli (cargo) declares `core` more than once in [target.'cfg(windows)'.dependencies]"
        );
    }

    /// `Display` is reachable only through `KindNotInEcosystem`, so the words are
    /// pinned here directly. The Cargo-optional render has to name the fix rather
    /// than read as "unsupported": Cargo does have optional dependencies, and a
    /// reader told otherwise may delete the flag instead of the wrong kind.
    #[test]
    fn every_kind_renders_its_own_word() {
        for (kind, word) in [
            (DependencyKind::Normal, "normal"),
            (DependencyKind::Peer, "peer"),
            (DependencyKind::Optional, "optional"),
            (DependencyKind::Build, "build"),
            (DependencyKind::Development, "development"),
        ] {
            assert_eq!(kind.to_string(), word);
        }

        let cargo_optional = WorkspaceError::KindNotInEcosystem {
            dependent: cargo("cli"),
            dependency: cargo("core"),
            kind: DependencyKind::Optional,
        }
        .to_string();
        assert!(
            cargo_optional.contains("`optional = true` entries in [dependencies]"),
            "the message does not name the fix: {cargo_optional}"
        );

        let npm_optional = WorkspaceError::KindNotInEcosystem {
            dependent: npm("cli"),
            dependency: npm("core"),
            kind: DependencyKind::Optional,
        }
        .to_string();
        assert!(
            !npm_optional.contains("optional = true"),
            "npm carries Cargo's advice: {npm_optional}"
        );

        let npm_build = WorkspaceError::KindNotInEcosystem {
            dependent: npm("cli"),
            dependency: npm("core"),
            kind: DependencyKind::Build,
        }
        .to_string();
        assert_eq!(
            npm_build,
            "cli (npm) declares core (npm) with kind build, which npm has no section for"
        );
    }

    /// Plans are compared whole, so order has to come from the ids rather than
    /// from insertion or a hash seed.
    #[test]
    fn packages_iterate_in_id_order_whatever_order_they_arrived_in() {
        let workspace = Workspace::new([
            package(cargo("zephyr"), vec![]),
            package(npm("alpha"), vec![]),
            package(cargo("alpha"), vec![]),
        ])
        .expect("workspace should build");

        let order: Vec<&PackageId> = workspace.packages().map(Package::id).collect();
        assert_eq!(
            order,
            vec![&npm("alpha"), &cargo("alpha"), &cargo("zephyr")]
        );
    }

    /// The pair above ties on name and breaks on ecosystem, which sorts the same
    /// way whichever field leads. This pair does not: name-major would reverse it.
    #[test]
    fn ids_sort_by_ecosystem_before_name() {
        let workspace = Workspace::new([
            package(cargo("alpha"), vec![]),
            package(npm("zephyr"), vec![]),
        ])
        .expect("workspace should build");

        let order: Vec<&PackageId> = workspace.packages().map(Package::id).collect();
        assert_eq!(order, vec![&npm("zephyr"), &cargo("alpha")]);
    }

    /// The publish-time rewrites ADR-0010 records, plus a prerelease case it does
    /// not cover — see [`DeclaredRange::published_req`] for why the pre survives.
    #[test]
    fn a_tracking_range_publishes_as_adr_0010_says() {
        for (version, exact, tilde, caret) in [
            ("1.5.0", "=1.5.0", "~1.5.0", "^1.5.0"),
            ("1.5.0-rc.1", "=1.5.0-rc.1", "~1.5.0-rc.1", "^1.5.0-rc.1"),
        ] {
            let version = Version::parse(version).expect("test version should parse");
            for (tracking, published) in [
                (Tracking::Exact, exact),
                (Tracking::Tilde, tilde),
                (Tracking::Caret, caret),
            ] {
                assert_eq!(
                    DeclaredRange::WorkspaceTracking(tracking).published_req(&version),
                    Some(req(published)),
                    "{version} {tracking:?}"
                );
            }
        }
    }

    /// The gate has to be able to fire. Expanding against the version the
    /// dependency is moving *to* would produce a range that contains it.
    #[test]
    fn an_exact_tracking_range_excludes_the_version_it_is_gating() {
        let at_last_tag = Version::new(1, 5, 0);
        let bumped_to = Version::new(1, 6, 0);

        let published =
            DeclaredRange::WorkspaceTracking(Tracking::Exact).published_req(&at_last_tag);
        let published = published.expect("tracking expands");
        assert!(!published.matches(&bumped_to));
        assert!(published.matches(&at_last_tag));
    }

    #[test]
    fn workspace_tracking_admits_via_tag_expansion() {
        let at_tag = Version::new(1, 5, 0);
        let same = Version::new(1, 5, 0);
        let minor = Version::new(1, 6, 0);
        let major = Version::new(2, 0, 0);

        let exact = DeclaredRange::WorkspaceTracking(Tracking::Exact);
        assert!(exact.admits(&at_tag, &same));
        assert!(!exact.admits(&at_tag, &minor));

        let tilde = DeclaredRange::WorkspaceTracking(Tracking::Tilde);
        assert!(tilde.admits(&at_tag, &same));
        assert!(!tilde.admits(&at_tag, &minor));

        let caret = DeclaredRange::WorkspaceTracking(Tracking::Caret);
        assert!(caret.admits(&at_tag, &same));
        assert!(caret.admits(&at_tag, &minor));
        assert!(!caret.admits(&at_tag, &major));
    }

    /// A range that carries its own bounds publishes those bounds for the gate.
    /// Cargo plains still expose a `VersionReq`; npm protocol arms do not.
    #[test]
    fn a_declared_range_publishes_the_bounds_it_carries() {
        let moved_on = Version::new(9, 9, 9);
        assert_eq!(
            plain_cargo("^1.5.0").published_req(&moved_on),
            Some(req("^1.5.0"))
        );

        for range in [
            workspace_npm("^1.5.0"),
            catalog_npm(Some("react18"), "^1.5.0"),
        ] {
            assert_eq!(range.published_req(&moved_on), None);
            assert!(range.admits(&moved_on, &Version::new(1, 5, 0)));
            assert!(!range.admits(&moved_on, &Version::new(2, 0, 0)));
        }
    }

    #[test]
    fn workspace_and_catalog_admit_through_their_carried_req() {
        let at_tag = Version::new(9, 9, 9);

        for range in [workspace_npm("^1.5.0"), catalog_npm(None, "^1.5.0")] {
            assert!(range.admits(&at_tag, &Version::new(1, 5, 0)));
            assert!(!range.admits(&at_tag, &Version::new(2, 0, 0)));
        }

        // Forms VersionReq cannot parse — proves protocol arms use npm Bounds.
        for range in [
            workspace_npm("^1.0.0 || ^2.0.0"),
            catalog_npm(None, "^1.0.0 || ^2.0.0"),
        ] {
            assert!(range.admits(&at_tag, &Version::new(1, 5, 0)));
            assert!(range.admits(&at_tag, &Version::new(2, 1, 0)));
            assert!(!range.admits(&at_tag, &Version::new(3, 0, 0)));
        }

        for range in [
            workspace_npm("1.5.0"),
            catalog_npm(Some("react18"), "1.5.0"),
        ] {
            assert!(range.admits(&at_tag, &Version::new(1, 5, 0)));
            assert!(!range.admits(&at_tag, &Version::new(1, 6, 0)));
        }
    }

    #[test]
    fn path_linked_on_cargo_builds_a_workspace() {
        let mut path = edge(cargo("core"), DependencyKind::Normal);
        path.range = DeclaredRange::PathLinked;
        let workspace = Workspace::new([
            package(cargo("core"), vec![]),
            package(cargo("cli"), vec![path]),
        ])
        .expect("path-linked Cargo edges should build");

        let edge = &workspace.get(&cargo("cli")).expect("cli").dependencies()[0];
        assert!(edge.range.is_path_linked());
        assert_eq!(workspace.dependents(&cargo("core")).count(), 1);
    }

    #[test]
    fn path_linked_never_admits_and_has_no_published_req() {
        let at_tag = Version::new(1, 0, 0);
        let next = Version::new(1, 0, 1);
        assert!(!DeclaredRange::PathLinked.admits(&at_tag, &at_tag));
        assert!(!DeclaredRange::PathLinked.admits(&at_tag, &next));
        assert_eq!(DeclaredRange::PathLinked.published_req(&at_tag), None);
        assert_eq!(DeclaredRange::PathLinked.protocol(), None);
        assert!(DeclaredRange::PathLinked.is_path_linked());
    }

    #[test]
    fn npm_plain_admits_via_js_semver_not_version_req() {
        let at_tag = Version::new(1, 5, 0);
        let range = plain_npm("1.5.0");
        assert!(range.admits(&at_tag, &Version::new(1, 5, 0)));
        assert!(!range.admits(&at_tag, &Version::new(1, 6, 0)));
        assert_eq!(range.published_req(&at_tag), None);
    }

    #[test]
    fn bare_version_at_declared_range_keeps_ecosystem_meaning() {
        let at_tag = Version::new(1, 5, 0);
        let cargo = plain_cargo("1.5.0");
        let npm = plain_npm("1.5.0");
        assert!(cargo.admits(&at_tag, &Version::new(1, 6, 0)));
        assert!(!npm.admits(&at_tag, &Version::new(1, 6, 0)));
    }

    #[test]
    fn a_range_shape_the_ecosystem_cannot_express_is_refused() {
        let mut path_on_npm = edge(npm("core"), DependencyKind::Normal);
        path_on_npm.range = DeclaredRange::PathLinked;
        assert_eq!(
            Workspace::new([
                package(npm("core"), vec![]),
                package(npm("cli"), vec![path_on_npm]),
            ])
            .expect_err("path-linked is Cargo-only"),
            WorkspaceError::BoundsNotInEcosystem {
                dependent: npm("cli"),
                dependency: npm("core"),
            }
        );

        let mut npm_bounds_on_cargo = edge(cargo("core"), DependencyKind::Normal);
        npm_bounds_on_cargo.range = plain_npm("^1.0.0");
        assert_eq!(
            Workspace::new([
                package(cargo("core"), vec![]),
                package(cargo("cli"), vec![npm_bounds_on_cargo]),
            ])
            .expect_err("npm Bounds on Cargo is refused"),
            WorkspaceError::BoundsNotInEcosystem {
                dependent: cargo("cli"),
                dependency: cargo("core"),
            }
        );

        let mut cargo_bounds_on_npm = edge(npm("core"), DependencyKind::Normal);
        cargo_bounds_on_npm.range = plain_cargo("^1.0.0");
        assert_eq!(
            Workspace::new([
                package(npm("core"), vec![]),
                package(npm("cli"), vec![cargo_bounds_on_npm]),
            ])
            .expect_err("Cargo Bounds on npm is refused"),
            WorkspaceError::BoundsNotInEcosystem {
                dependent: npm("cli"),
                dependency: npm("core"),
            }
        );

        let cargo_bounds = Bounds::from_cargo_text("^1.0.0").expect("cargo");
        for range in [
            DeclaredRange::Workspace(cargo_bounds.clone()),
            DeclaredRange::Catalog {
                name: None,
                bounds: cargo_bounds,
            },
        ] {
            let mut edge = edge(npm("core"), DependencyKind::Normal);
            edge.range = range;
            assert_eq!(
                Workspace::new([
                    package(npm("core"), vec![]),
                    package(npm("cli"), vec![edge]),
                ])
                .expect_err("Cargo Bounds on npm protocol arms is refused"),
                WorkspaceError::BoundsNotInEcosystem {
                    dependent: npm("cli"),
                    dependency: npm("core"),
                }
            );
        }
    }

    /// The row that broke release-plz: a caret on a `0.x` version does not span a
    /// minor bump. A claim about the semver crate, and it decides every cascade
    /// in a pre-1.0 workspace.
    #[test]
    fn a_caret_on_a_zero_major_excludes_the_next_minor() {
        assert!(!req("^0.1.3").matches(&Version::new(0, 2, 0)));
        assert!(req("^1.1.3").matches(&Version::new(1, 2, 0)));
    }

    /// See [`DeclaredRange`]: `1.5.0` and `=1.5.0` are what pnpm and yarn write
    /// for the same `workspace:*` declaration, and Cargo's parser reads the first
    /// as a caret.
    #[test]
    fn cargos_grammar_reads_a_bare_version_as_a_caret() {
        assert_ne!(req("1.5.0"), req("=1.5.0"));
        assert!(req("1.5.0").matches(&Version::new(1, 6, 0)));
        assert!(!req("=1.5.0").matches(&Version::new(1, 6, 0)));
    }

    #[test]
    fn either_route_to_build_time_resolution_reads_as_one() {
        for resolution in [BuildResolution::BinaryTarget, BuildResolution::Declared] {
            assert!(ResolvesDependenciesAt::Build(resolution).is_build());
        }
        assert!(!ResolvesDependenciesAt::Install.is_build());
    }

    /// ADR-0009 turns on this field surviving construction, and ADR-0014 on the
    /// version doing the same. A constructor that dropped either would be the
    /// under-release both exist to prevent.
    #[test]
    fn a_package_keeps_the_version_and_resolution_it_was_built_with() {
        let workspace = Workspace::new([Package::new(
            cargo("cli"),
            Version::new(2, 3, 4),
            ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget),
            true,
            vec![],
        )])
        .expect("workspace should build");

        let cli = workspace.get(&cargo("cli")).expect("cli should be present");
        assert_eq!(cli.version(), &Version::new(2, 3, 4));
        assert_eq!(
            cli.resolves_dependencies_at(),
            ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget)
        );
    }

    #[test]
    fn both_ecosystems_render_their_own_name() {
        assert_eq!(npm("x").to_string(), "x (npm)");
        assert_eq!(cargo("x").to_string(), "x (cargo)");
    }
}
