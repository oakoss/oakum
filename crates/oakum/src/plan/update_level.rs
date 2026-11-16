//! The release level a dependency update implies for a package, and the note
//! it carries (ADR-0040).
//!
//! A move is compatible when the new requirement's lowest version falls within
//! the caret range of the old one's, or the new range accepts every version
//! the old one did; for `peerDependencies`, only the second. Requirements are
//! read as unions of half-open version intervals. One that cannot be read so
//! (a dist-tag, a URL, an operator this does not know) has no lowest version
//! and is never compatible, since the level errs upward. A prerelease
//! comparator is not read either: cargo and npm match a prerelease only under
//! a comparator naming the same release, which no interval holds.

use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use semver::{Op, Version, VersionReq};

use super::bump::BumpLevel;
use super::dependency_update::Moved;
use super::workspace::{
    BuildResolution, Declaration, DependencyKind, Ecosystem, Package, ResolvesDependenciesAt,
};

/// `[low, high)`; no `high` is unbounded.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Interval {
    low: Version,
    high: Option<Version>,
}

impl Interval {
    fn everything() -> Self {
        Self {
            low: Version::new(0, 0, 0),
            high: None,
        }
    }

    fn holds(&self, version: &Version) -> bool {
        *version >= self.low && self.high.as_ref().is_none_or(|high| version < high)
    }

    fn covers(&self, other: &Self) -> bool {
        self.low <= other.low
            && match (&self.high, &other.high) {
                (None, _) => true,
                (Some(_), None) => false,
                (Some(mine), Some(theirs)) => theirs <= mine,
            }
    }

    fn intersect(self, other: Self) -> Option<Self> {
        let low = self.low.max(other.low);
        let high = match (self.high, other.high) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (Some(high), None) | (None, Some(high)) => Some(high),
            (None, None) => None,
        };
        high.as_ref()
            .is_none_or(|high| low < *high)
            .then_some(Self { low, high })
    }
}

/// A version written with its minor or patch left out (`1`, `1.2`, `1.x`).
#[derive(Clone, Debug)]
struct Partial {
    major: u64,
    minor: Option<u64>,
    patch: Option<u64>,
}

impl Partial {
    fn floor(&self) -> Version {
        Version::new(self.major, self.minor.unwrap_or(0), self.patch.unwrap_or(0))
    }

    /// The first version past every version this names.
    fn past(&self) -> Version {
        match (self.minor, self.patch) {
            (Some(minor), Some(patch)) => Version::new(self.major, minor, patch + 1),
            (Some(minor), None) => Version::new(self.major, minor + 1, 0),
            (None, _) => Version::new(self.major + 1, 0, 0),
        }
    }

    fn caret_high(&self) -> Version {
        match (self.major, self.minor, self.patch) {
            (0, Some(0), Some(patch)) => Version::new(0, 0, patch + 1),
            (0, Some(0), None) => Version::new(0, 1, 0),
            (0, Some(minor), _) => Version::new(0, minor + 1, 0),
            (0, None, _) => Version::new(1, 0, 0),
            (major, _, _) => Version::new(major + 1, 0, 0),
        }
    }

    fn tilde_high(&self) -> Version {
        match self.minor {
            Some(minor) => Version::new(self.major, minor + 1, 0),
            None => Version::new(self.major + 1, 0, 0),
        }
    }

    fn of(version: &Version) -> Self {
        Self {
            major: version.major,
            minor: Some(version.minor),
            patch: Some(version.patch),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Operator {
    Exact,
    Greater,
    GreaterEq,
    Less,
    LessEq,
    Tilde,
    Caret,
}

fn interval(op: Operator, partial: &Partial) -> Option<Interval> {
    let zero = || Version::new(0, 0, 0);
    let (low, high) = match op {
        Operator::Exact => (partial.floor(), Some(partial.past())),
        Operator::Greater => (partial.past(), None),
        Operator::GreaterEq => (partial.floor(), None),
        Operator::Less => (zero(), Some(partial.floor())),
        Operator::LessEq => (zero(), Some(partial.past())),
        Operator::Tilde => (partial.floor(), Some(partial.tilde_high())),
        Operator::Caret => (partial.floor(), Some(partial.caret_high())),
    };
    Interval::everything().intersect(Interval { low, high })
}

/// A Cargo requirement as one interval: its comparators all hold.
fn cargo_intervals(req: &VersionReq) -> Option<Vec<Interval>> {
    let mut range = Interval::everything();
    for comparator in &req.comparators {
        let op = match comparator.op {
            Op::Exact | Op::Wildcard => Operator::Exact,
            Op::Greater => Operator::Greater,
            Op::GreaterEq => Operator::GreaterEq,
            Op::Less => Operator::Less,
            Op::LessEq => Operator::LessEq,
            Op::Tilde => Operator::Tilde,
            Op::Caret => Operator::Caret,
            _ => return None,
        };
        if !comparator.pre.is_empty() {
            return None;
        }
        let partial = Partial {
            major: comparator.major,
            minor: comparator.minor,
            patch: comparator.patch,
        };
        range = range.intersect(interval(op, &partial)?)?;
    }
    Some(alloc::vec![range])
}

fn npm_partial(text: &str) -> Option<Partial> {
    let text = text.strip_prefix(['v', '=']).unwrap_or(text);
    let text = text.split('+').next().unwrap_or(text);
    // A prerelease's `-` leaves a part that does not parse as a number.
    let mut parts = text.split('.');
    let number = |part: Option<&str>| -> Result<Option<u64>, ()> {
        match part {
            None | Some("x" | "X" | "*") => Ok(None),
            Some(digits) => digits.parse().map(Some).map_err(|_| ()),
        }
    };
    let major = number(parts.next()).ok()??;
    let minor = number(parts.next()).ok()?;
    let patch = number(parts.next()).ok()?;
    if parts.next().is_some() || (minor.is_none() && patch.is_some()) {
        return None;
    }
    Some(Partial {
        major,
        minor,
        patch,
    })
}

fn npm_clause(clause: &str) -> Option<Interval> {
    let clause = clause.trim();
    if clause.is_empty() || matches!(clause, "*" | "x" | "X") {
        return Some(Interval::everything());
    }
    if let Some((from, to)) = clause.split_once(" - ") {
        let (from, to) = (npm_partial(from.trim())?, npm_partial(to.trim())?);
        return Interval {
            low: from.floor(),
            high: Some(to.past()),
        }
        .intersect(Interval::everything());
    }
    let mut range = Interval::everything();
    let mut tokens = clause.split_whitespace().peekable();
    while let Some(token) = tokens.next() {
        let split = token
            .find(|ch: char| ch.is_ascii_alphanumeric() || ch == '*')
            .unwrap_or(token.len());
        let (op, rest) = token.split_at(split);
        let rest = if rest.is_empty() {
            tokens.next()?
        } else {
            rest
        };
        let op = match op {
            "" | "=" => Operator::Exact,
            ">" => Operator::Greater,
            ">=" => Operator::GreaterEq,
            "<" => Operator::Less,
            "<=" => Operator::LessEq,
            "~" | "~>" => Operator::Tilde,
            "^" => Operator::Caret,
            _ => return None,
        };
        if matches!(rest, "*" | "x" | "X") {
            if matches!(op, Operator::Exact) {
                continue;
            }
            return None;
        }
        range = range.intersect(interval(op, &npm_partial(rest)?)?)?;
    }
    Some(range)
}

/// An npm range as the union of its `||` clauses.
fn npm_intervals(text: &str) -> Option<Vec<Interval>> {
    text.split("||")
        .map(npm_clause)
        .collect::<Option<Vec<_>>>()
        .map(merged)
}

/// Overlapping or touching intervals joined, so coverage reads the union.
fn merged(mut intervals: Vec<Interval>) -> Vec<Interval> {
    intervals.sort_by(|left, right| left.low.cmp(&right.low));
    let mut out: Vec<Interval> = Vec::new();
    for next in intervals {
        match out.last_mut() {
            Some(last) if last.high.as_ref().is_none_or(|high| next.low <= *high) => {
                last.high = match (last.high.take(), next.high) {
                    (Some(left), Some(right)) => Some(left.max(right)),
                    _ => None,
                };
            }
            _ => out.push(next),
        }
    }
    out
}

/// Whether moving from `before` to `after` is compatible: for a peer range,
/// the new range accepts every version the old one did; otherwise that, or
/// the new lowest version falls within the caret range of the old one's.
fn compatible(kind: DependencyKind, before: &[Interval], after: &[Interval]) -> bool {
    let accepts_all = before
        .iter()
        .all(|old| after.iter().any(|new| new.covers(old)));
    if accepts_all || kind == DependencyKind::Peer {
        return accepts_all;
    }
    let lowest = |side: &[Interval]| side.iter().map(|interval| interval.low.clone()).min();
    let (Some(old), Some(new)) = (lowest(before), lowest(after)) else {
        return false;
    };
    let partial = Partial::of(&old);
    Interval {
        low: old,
        high: Some(partial.caret_high()),
    }
    .holds(&new)
}

/// Whether a Cargo requirement moving from `before` to `after` is compatible.
#[must_use]
pub fn cargo_requirement_compatible(before: &VersionReq, after: &VersionReq) -> bool {
    match (cargo_intervals(before), cargo_intervals(after)) {
        (Some(before), Some(after)) => compatible(DependencyKind::Normal, &before, &after),
        _ => false,
    }
}

/// Whether an npm range of `kind` moving from `before` to `after` is
/// compatible. A range with no lowest version, such as a dist-tag or a URL,
/// is not.
#[must_use]
pub fn npm_range_compatible(kind: DependencyKind, before: &str, after: &str) -> bool {
    match (npm_intervals(before), npm_intervals(after)) {
        (Some(before), Some(after)) => compatible(kind, &before, &after),
        _ => false,
    }
}

/// No feature removed and the defaults not turned off; adding one is
/// compatible.
#[must_use]
pub fn features_compatible(before: &Declaration, after: &Declaration) -> bool {
    before.features.is_subset(&after.features)
        && (after.default_features || !before.default_features)
}

/// The level a moved Cargo entry adds to a library: `patch` when the move is
/// compatible, `major` when its requirement, features or source is not. A
/// source or path that changed has no lowest version to compare.
#[must_use]
pub fn cargo_move_level(moved: &Moved) -> BumpLevel {
    let (before, after) = (&moved.before, &moved.after);
    let compatible = before.source == after.source
        && before.declaration.path == after.declaration.path
        && cargo_requirement_compatible(&before.req, &after.req)
        && features_compatible(&before.declaration, &after.declaration);
    if compatible {
        BumpLevel::Patch
    } else {
        BumpLevel::Major
    }
}

/// The level a moved npm entry of `kind` adds to a library.
#[must_use]
pub fn npm_move_level(kind: DependencyKind, before: &str, after: &str) -> BumpLevel {
    if npm_range_compatible(kind, before, after) {
        BumpLevel::Patch
    } else {
        BumpLevel::Major
    }
}

/// What dependency updates in a range did to one package.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UpdateReach {
    /// Some dependency update reached it, through a lockfile or a declaration.
    pub reached: bool,
    /// The highest level among its own moved declarations, those of an
    /// unmanaged package it ships, and the shared entries it takes.
    pub declared: Option<BumpLevel>,
}

/// The level dependency updates release `package` at, if any. A package that
/// resolves its dependencies at build takes `patch` for any update; a library
/// takes its declared level, and nothing for a lockfile alone; a `bin` that
/// also ships a library takes both, the higher winning. A Cargo crate with
/// neither, built only as `cdylib` or `staticlib`, ships its dependencies
/// compiled in, so it takes the build rule.
#[must_use]
pub fn update_level(package: &Package, reach: UpdateReach) -> Option<BumpLevel> {
    let at = package.resolves_dependencies_at();
    let artifact_only = artifact_only(package);
    let build = (takes_build_rule(package) && reach.reached).then_some(BumpLevel::Patch);
    let library_rule = match at {
        ResolvesDependenciesAt::Install => !artifact_only,
        ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget) => package.ships_library(),
        ResolvesDependenciesAt::Build(BuildResolution::Declared) => false,
    };
    let library = if library_rule { reach.declared } else { None };
    build.max(library).filter(|level| level.is_release())
}

/// A Cargo crate resolving at install that ships no library: built only as
/// `cdylib` or `staticlib`.
fn artifact_only(package: &Package) -> bool {
    package.resolves_dependencies_at() == ResolvesDependenciesAt::Install
        && package.id().ecosystem == Ecosystem::Cargo
        && !package.ships_library()
}

/// Whether a dependency update reaches `package` as a build does: what it
/// ships has its dependencies resolved in, so a lockfile update below it
/// changes it ([`update_level`]).
#[must_use]
pub fn takes_build_rule(package: &Package) -> bool {
    package.resolves_dependencies_at().is_build() || artifact_only(package)
}

/// What one line of a dependency update's note says.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NoteChange {
    /// The declared requirement when it moved, otherwise the locked version.
    Moved {
        from: String,
        to: String,
    },
    /// Its own version did not move; crates below it did.
    DependenciesUpdated,
    FeatureRemoved(String),
    FeatureAdded(String),
    DefaultFeaturesOff,
    DefaultFeaturesOn,
}

/// One direct dependency an update reached a package through, qualified by
/// its owner or target where a name is declared more than once.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct NoteLine {
    pub name: String,
    pub qualifier: Option<String>,
    pub change: NoteChange,
}

/// The note lines a features or default-features change reads as.
#[must_use]
pub fn feature_changes(before: &Declaration, after: &Declaration) -> Vec<NoteChange> {
    let mut changes: Vec<NoteChange> = before
        .features
        .difference(&after.features)
        .map(|feature| NoteChange::FeatureRemoved(feature.clone()))
        .chain(
            after
                .features
                .difference(&before.features)
                .map(|feature| NoteChange::FeatureAdded(feature.clone())),
        )
        .collect();
    match (before.default_features, after.default_features) {
        (true, false) => changes.push(NoteChange::DefaultFeaturesOff),
        (false, true) => changes.push(NoteChange::DefaultFeaturesOn),
        _ => {}
    }
    changes
}

/// The note under `### Changed`, one bullet per line sorted by name, each
/// line once; empty when there are none.
#[must_use]
pub fn render_update_note(lines: &[NoteLine]) -> String {
    let rendered: BTreeSet<(String, String)> = lines
        .iter()
        .map(|line| {
            let name = match &line.qualifier {
                Some(qualifier) => format!("{} ({qualifier})", line.name),
                None => line.name.clone(),
            };
            let text = match &line.change {
                NoteChange::Moved { from, to } => format!("{name} {from} → {to}"),
                NoteChange::DependenciesUpdated => format!("{name} (its dependencies updated)"),
                NoteChange::FeatureRemoved(feature) => format!("{name}: feature {feature} removed"),
                NoteChange::FeatureAdded(feature) => format!("{name}: feature {feature} added"),
                NoteChange::DefaultFeaturesOff => format!("{name}: default features off"),
                NoteChange::DefaultFeaturesOn => format!("{name}: default features on"),
            };
            (line.name.clone(), text)
        })
        .collect();
    if rendered.is_empty() {
        return String::new();
    }
    let mut note = "### Changed\n\n".to_string();
    for (_, text) in rendered {
        note.push_str("- ");
        note.push_str(&text);
        note.push('\n');
    }
    note
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cargo(before: &str, after: &str) -> bool {
        cargo_requirement_compatible(
            &VersionReq::parse(before).expect("before"),
            &VersionReq::parse(after).expect("after"),
        )
    }

    #[test]
    fn a_cargo_move_is_compatible_within_the_caret_or_when_it_widens() {
        for (before, after) in [
            ("^1.2", "^1.5"),
            ("^1.5", "^1.2"),
            ("1.0.100", "1.0.200"),
            ("~1.2.3", "~1.2.5"),
            (">=1.2, <2", "^1.4"),
            ("^0.40.1", "^0.40.3"),
            ("=1.2.3", "^1.2"),
            ("~1.2", "~1.3"),
        ] {
            assert!(cargo(before, after), "{before} → {after}");
        }
        for (before, after) in [
            ("^0.40", "^0.41"),
            ("^1", "^2"),
            ("^0.0.3", "^0.0.4"),
            ("<1.5", ">=2"),
            ("=1.2.3", "=1.2.3-rc.1"),
            ("^1.2.3-alpha", "^1.2"),
            ("=1.2.3", "<=1.2.3-alpha"),
        ] {
            assert!(!cargo(before, after), "{before} → {after}");
        }
    }

    #[test]
    fn an_npm_range_is_read_as_npm_writes_it() {
        let normal = DependencyKind::Normal;
        for (before, after) in [
            ("^1.2.0", "^1.5.0"),
            ("^1.5.0", "^1.2.0"),
            ("1.x", "^1.3.0"),
            ("~3.0.0", "~3.0.4"),
            (">= 1.2.0 < 2", "^1.4.0"),
            ("1.2.0 - 1.4", "^1.3.0"),
            ("^1", "^1 || ^2"),
        ] {
            assert!(
                npm_range_compatible(normal, before, after),
                "{before} → {after}"
            );
        }
        for (before, after) in [
            ("^0.40.0", "^0.41.0"),
            ("^1.0.0", "^2.0.0"),
            ("latest", "^1.0.0"),
            ("^1.0.0", "next"),
            ("^1.0.0", "https://example.invalid/x.tgz"),
            ("^1.0.0", "github:a/b#v2"),
            ("1.2.3", "1.2.3-rc.1"),
            ("^1.2.3-alpha", "^1.2.0"),
            ("^1.0.0", "^2.0.0-alpha"),
            ("*", ">*"),
            ("*", "<*"),
        ] {
            assert!(
                !npm_range_compatible(normal, before, after),
                "{before} → {after}"
            );
        }
    }

    #[test]
    fn a_peer_range_is_compatible_only_when_it_accepts_every_old_version() {
        let peer = DependencyKind::Peer;
        assert!(npm_range_compatible(peer, "^1", "^1 || ^2"));
        assert!(npm_range_compatible(peer, "^1.5.0", "^1.2.0"));
        assert!(npm_range_compatible(peer, ">=1 <3", "^1 || ^2"));
        assert!(npm_range_compatible(peer, ">=1", "1.x || >=2"));
        for (before, after) in [("^1 || ^2", "^1"), ("^1 || ^2", "^2"), ("^1.2.0", "^1.5.0")] {
            assert!(
                !npm_range_compatible(peer, before, after),
                "{before} → {after}"
            );
        }
    }

    fn declaration(features: &[&str], default_features: bool) -> Declaration {
        Declaration {
            features: features
                .iter()
                .map(|feature| (*feature).to_string())
                .collect(),
            default_features,
            ..Declaration::default()
        }
    }

    #[test]
    fn a_features_move_is_compatible_unless_it_takes_one_away() {
        let plain = declaration(&["derive"], true);
        assert!(features_compatible(
            &plain,
            &declaration(&["derive", "rc"], true)
        ));
        assert!(features_compatible(
            &declaration(&[], false),
            &declaration(&[], true)
        ));
        assert!(!features_compatible(&plain, &declaration(&[], true)));
        assert!(!features_compatible(
            &plain,
            &declaration(&["derive"], false)
        ));
        assert_eq!(
            feature_changes(&plain, &declaration(&["rc"], false)),
            alloc::vec![
                NoteChange::FeatureRemoved("derive".into()),
                NoteChange::FeatureAdded("rc".into()),
                NoteChange::DefaultFeaturesOff,
            ]
        );
    }

    fn moved(before: &str, after: &str, source: Option<&str>) -> Moved {
        let entry = |req: &str, source: Option<&str>| super::super::workspace::ExternalDependency {
            name: "serde".into(),
            declared_as: "serde".into(),
            kind: DependencyKind::Normal,
            source: source.map(Into::into),
            req: VersionReq::parse(req).expect("req"),
            inherited: false,
            declaration: Declaration::default(),
        };
        Moved {
            before: entry(before, Some("registry+crates")),
            after: entry(after, source),
        }
    }

    #[test]
    fn a_cargo_entry_moving_source_is_never_compatible() {
        assert_eq!(
            cargo_move_level(&moved("1.0.100", "1.0.200", Some("registry+crates"))),
            BumpLevel::Patch
        );
        assert_eq!(
            cargo_move_level(&moved("1", "2", Some("registry+crates"))),
            BumpLevel::Major
        );
        assert_eq!(
            cargo_move_level(&moved("*", "*", Some("git+https://example.invalid/serde"))),
            BumpLevel::Major
        );
    }

    fn package(at: ResolvesDependenciesAt, ships_library: bool) -> Package {
        Package::new(
            super::super::workspace::PackageId::new(
                super::super::workspace::Ecosystem::Cargo,
                "app",
            ),
            Version::new(1, 0, 0),
            at,
            true,
            Vec::new(),
        )
        .with_ships_library(ships_library)
    }

    #[test]
    fn a_build_takes_patch_and_a_library_its_declared_level() {
        let bin = ResolvesDependenciesAt::Build(BuildResolution::BinaryTarget);
        let bundled = ResolvesDependenciesAt::Build(BuildResolution::Declared);
        let library = ResolvesDependenciesAt::Install;
        let locked = UpdateReach {
            reached: true,
            declared: None,
        };
        let major = UpdateReach {
            reached: true,
            declared: Some(BumpLevel::Major),
        };
        assert_eq!(
            update_level(&package(bin, false), locked),
            Some(BumpLevel::Patch)
        );
        assert_eq!(update_level(&package(library, true), locked), None);
        assert_eq!(
            update_level(&package(library, true), major),
            Some(BumpLevel::Major)
        );
        assert_eq!(
            update_level(&package(bin, false), major),
            Some(BumpLevel::Patch)
        );
        assert_eq!(
            update_level(&package(bin, true), major),
            Some(BumpLevel::Major)
        );
        assert_eq!(
            update_level(&package(bundled, true), major),
            Some(BumpLevel::Patch)
        );
        assert_eq!(
            update_level(&package(bin, true), UpdateReach::default()),
            None
        );
        let cdylib = package(library, false);
        assert_eq!(update_level(&cdylib, locked), Some(BumpLevel::Patch));
        assert_eq!(update_level(&cdylib, major), Some(BumpLevel::Patch));
        let npm_no_entry = Package::new(
            super::super::workspace::PackageId::new(Ecosystem::Npm, "web"),
            Version::new(1, 0, 0),
            library,
            true,
            Vec::new(),
        );
        assert_eq!(update_level(&npm_no_entry, locked), None);
        assert_eq!(update_level(&npm_no_entry, major), Some(BumpLevel::Major));
        let declared_bin = package(bin, true).with_resolves_dependencies_at(bundled);
        assert_eq!(update_level(&declared_bin, major), Some(BumpLevel::Major));
        let installed_bin = package(bin, true).with_resolves_dependencies_at(library);
        assert_eq!(installed_bin.resolves_dependencies_at(), library);
    }

    #[test]
    fn the_note_lists_each_dependency_once_by_name() {
        let line = |name: &str, qualifier: Option<&str>, change: NoteChange| NoteLine {
            name: name.into(),
            qualifier: qualifier.map(Into::into),
            change,
        };
        let moved = |from: &str, to: &str| NoteChange::Moved {
            from: from.into(),
            to: to.into(),
        };
        let note = render_update_note(&[
            line("rusqlite", None, moved("0.40.0", "0.41.0")),
            line("host", None, moved("^1 || ^2", "^2")),
            line(
                "gitdep",
                None,
                moved("^1 (git+https://a/b)", "^1 (git+https://a/c)"),
            ),
            line("rusqlite", None, moved("0.40.0", "0.41.0")),
            line(
                "windows-sys",
                Some("cfg(windows)"),
                moved("0.52.0", "0.59.0"),
            ),
            line("tm-core", None, NoteChange::DependenciesUpdated),
            line("serde", None, NoteChange::FeatureRemoved("derive".into())),
        ]);
        assert_eq!(
            note,
            "### Changed\n\n\
             - gitdep ^1 (git+https://a/b) → ^1 (git+https://a/c)\n\
             - host ^1 || ^2 → ^2\n\
             - rusqlite 0.40.0 → 0.41.0\n\
             - serde: feature derive removed\n\
             - tm-core (its dependencies updated)\n\
             - windows-sys (cfg(windows)) 0.52.0 → 0.59.0\n"
        );
        assert_eq!(render_update_note(&[]), "");
    }
}
