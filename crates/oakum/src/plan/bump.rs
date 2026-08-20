//! Apply a bump level to a version, including the zero-major mapping (ADR-0022).
//!
//! Pure: inputs are a version, a level, and a versioning policy. No package graph,
//! no tags, no config I/O — those resolve *which* policy applies to a package
//! before calling here.

use core::fmt;

use semver::Version;

/// How far a change file asks a package to move.
///
/// Order is patch < minor < major so aggregation can take the highest of several
/// files for one package (`okm-4eg`) with `Ord` rather than a hand-rolled rank.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BumpLevel {
    Patch,
    Minor,
    Major,
}

impl fmt::Display for BumpLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Patch => "patch",
            Self::Minor => "minor",
            Self::Major => "major",
        })
    }
}

/// How a [`BumpLevel::Major`] maps while a package is below `1.0.0`.
///
/// Repository-wide default with an optional per-package override; this type is
/// only the value after that resolution. Evaluation is always against the
/// package's own current version (ADR-0022).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Versioning {
    /// Below `1.0.0`, a major change file produces a minor bump. At or above
    /// `1.0.0` this is identical to [`Versioning::Semver`].
    #[default]
    ZeroMajor,
    /// Strict semver: a major change file always increments the major component.
    Semver,
}

impl fmt::Display for Versioning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ZeroMajor => "zero-major",
            Self::Semver => "semver",
        })
    }
}

/// The level that actually moves the version number, after zero-major mapping.
///
/// Equal to `requested` except when [`Versioning::ZeroMajor`] remaps a major on a
/// `0.x` package to a minor. `--explain` names that remap (ADR-0022).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppliedBump {
    pub requested: BumpLevel,
    pub effective: BumpLevel,
    pub versioning: Versioning,
}

impl AppliedBump {
    /// Whether zero-major changed what the change file asked for.
    #[must_use]
    pub const fn was_remapped(self) -> bool {
        !matches!(
            (self.requested, self.effective),
            (BumpLevel::Patch, BumpLevel::Patch)
                | (BumpLevel::Minor, BumpLevel::Minor)
                | (BumpLevel::Major, BumpLevel::Major)
        )
    }
}

/// Resolve the effective bump level for `current` under `versioning`.
///
/// A feature never becomes a patch. ADR-0022 declines knope's and release-please's
/// feature-to-patch mapping below 1.0.0; only `major` is remapped.
#[must_use]
pub const fn effective_bump(
    current: &Version,
    requested: BumpLevel,
    versioning: Versioning,
) -> AppliedBump {
    let effective = match (versioning, requested, current.major == 0) {
        (Versioning::ZeroMajor, BumpLevel::Major, true) => BumpLevel::Minor,
        _ => requested,
    };
    AppliedBump {
        requested,
        effective,
        versioning,
    }
}

/// Apply `requested` to `current` under `versioning`, returning the next version.
///
/// Pre-release and build metadata are dropped: a bump is a release line move, and
/// carrying either forward would stamp a version that was never cut as a tag.
#[must_use]
pub fn apply_bump(current: &Version, requested: BumpLevel, versioning: Versioning) -> Version {
    let applied = effective_bump(current, requested, versioning);
    match applied.effective {
        BumpLevel::Patch => Version::new(current.major, current.minor, current.patch + 1),
        BumpLevel::Minor => Version::new(current.major, current.minor + 1, 0),
        BumpLevel::Major => Version::new(current.major + 1, 0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    fn v(major: u64, minor: u64, patch: u64) -> Version {
        Version::new(major, minor, patch)
    }

    #[test]
    fn patch_and_minor_ignore_versioning_and_major_line() {
        for versioning in [Versioning::ZeroMajor, Versioning::Semver] {
            assert_eq!(
                apply_bump(&v(0, 1, 3), BumpLevel::Patch, versioning),
                v(0, 1, 4)
            );
            assert_eq!(
                apply_bump(&v(0, 1, 3), BumpLevel::Minor, versioning),
                v(0, 2, 0)
            );
            assert_eq!(
                apply_bump(&v(1, 4, 2), BumpLevel::Patch, versioning),
                v(1, 4, 3)
            );
            assert_eq!(
                apply_bump(&v(1, 4, 2), BumpLevel::Minor, versioning),
                v(1, 5, 0)
            );
        }
    }

    /// ADR-0022: below 1.0.0 under zero-major, major → next minor, not 1.0.0.
    #[test]
    fn zero_major_maps_breaking_to_minor_below_one() {
        assert_eq!(
            apply_bump(&v(0, 1, 3), BumpLevel::Major, Versioning::ZeroMajor),
            v(0, 2, 0)
        );
        let applied = effective_bump(&v(0, 1, 3), BumpLevel::Major, Versioning::ZeroMajor);
        assert_eq!(applied.effective, BumpLevel::Minor);
        assert!(applied.was_remapped());
    }

    /// Graduation is setting semver; the next major then yields 1.0.0.
    #[test]
    fn semver_major_on_zero_line_is_one_zero_zero() {
        assert_eq!(
            apply_bump(&v(0, 4, 1), BumpLevel::Major, Versioning::Semver),
            v(1, 0, 0)
        );
        assert!(!effective_bump(&v(0, 4, 1), BumpLevel::Major, Versioning::Semver).was_remapped());
    }

    /// At or above 1.0.0 the setting is inert.
    #[test]
    fn zero_major_is_inert_at_or_above_one() {
        assert_eq!(
            apply_bump(&v(1, 4, 3), BumpLevel::Major, Versioning::ZeroMajor),
            v(2, 0, 0)
        );
        assert_eq!(
            apply_bump(&v(1, 4, 3), BumpLevel::Major, Versioning::Semver),
            v(2, 0, 0)
        );
        assert!(
            !effective_bump(&v(1, 4, 3), BumpLevel::Major, Versioning::ZeroMajor).was_remapped()
        );
    }

    /// A feature does not become a patch below 1.0.0 — the knope divergence.
    #[test]
    fn feature_stays_minor_below_one() {
        assert_eq!(
            apply_bump(&v(0, 1, 3), BumpLevel::Minor, Versioning::ZeroMajor),
            v(0, 2, 0)
        );
        assert_ne!(
            apply_bump(&v(0, 1, 3), BumpLevel::Minor, Versioning::ZeroMajor),
            v(0, 1, 4)
        );
    }

    /// Mixed workspace: each package's own version decides, not a repo-wide line.
    #[test]
    fn mixed_workspace_evaluates_per_package_version() {
        let policy = Versioning::ZeroMajor;
        assert_eq!(
            apply_bump(&v(0, 1, 0), BumpLevel::Major, policy),
            v(0, 2, 0)
        );
        assert_eq!(
            apply_bump(&v(1, 4, 1), BumpLevel::Major, policy),
            v(2, 0, 0)
        );
    }

    #[test]
    fn bump_levels_order_for_aggregation() {
        assert!(BumpLevel::Patch < BumpLevel::Minor);
        assert!(BumpLevel::Minor < BumpLevel::Major);
    }

    #[test]
    fn pre_release_and_build_are_stripped() {
        let mut current = v(0, 1, 3);
        current.pre = semver::Prerelease::new("rc.1").expect("pre");
        current.build = semver::BuildMetadata::new("git").expect("build");
        let next = apply_bump(&current, BumpLevel::Patch, Versioning::ZeroMajor);
        assert_eq!(next, v(0, 1, 4));
        assert!(next.pre.is_empty());
        assert!(next.build.is_empty());
    }

    #[test]
    fn display_matches_config_spelling() {
        assert_eq!(BumpLevel::Major.to_string(), "major");
        assert_eq!(Versioning::ZeroMajor.to_string(), "zero-major");
        assert_eq!(Versioning::Semver.to_string(), "semver");
    }

    #[test]
    fn default_versioning_is_zero_major() {
        assert_eq!(Versioning::default(), Versioning::ZeroMajor);
    }
}
