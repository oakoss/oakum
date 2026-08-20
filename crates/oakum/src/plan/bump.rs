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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BumpError {
    /// Component already `u64::MAX`; wrapping would yield a lower version.
    Overflow,
    /// `current` would remap to a different effective level than this value stores
    /// (for example a zero-major remap from `0.x` applied to a `1.x` package).
    StaleMapping,
}

impl fmt::Display for BumpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Overflow => f.write_str("version component overflow"),
            Self::StaleMapping => f.write_str("applied bump does not match this version"),
        }
    }
}

impl core::error::Error for BumpError {}

/// The level that actually moves the version number, after zero-major mapping.
///
/// Equal to `requested` except when [`Versioning::ZeroMajor`] remaps a major on a
/// `0.x` package to a minor. `--explain` names that remap (ADR-0022).
///
/// Construct only via [`effective_bump`] / [`apply_bump`] so `requested` and
/// `effective` cannot disagree with the versioning policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppliedBump {
    requested: BumpLevel,
    effective: BumpLevel,
    versioning: Versioning,
}

impl AppliedBump {
    #[must_use]
    pub const fn requested(self) -> BumpLevel {
        self.requested
    }

    #[must_use]
    pub const fn effective(self) -> BumpLevel {
        self.effective
    }

    #[must_use]
    pub const fn versioning(self) -> Versioning {
        self.versioning
    }

    /// Whether zero-major changed what the change file asked for.
    #[must_use]
    pub fn was_remapped(self) -> bool {
        self.requested != self.effective
    }

    /// Pre-release and build metadata are dropped: a bump is a release line move,
    /// and carrying either forward would stamp a version that was never cut as a
    /// tag.
    ///
    /// # Errors
    ///
    /// Returns [`BumpError::StaleMapping`] when `current` would not produce this
    /// same effective level under the stored request and versioning.
    /// Returns [`BumpError::Overflow`] when the component that would increment is
    /// already `u64::MAX`.
    pub fn next_version(self, current: &Version) -> Result<Version, BumpError> {
        let expected = effective_bump(current, self.requested, self.versioning);
        if expected.effective != self.effective {
            return Err(BumpError::StaleMapping);
        }
        let (major, minor, patch) = match self.effective {
            BumpLevel::Patch => (
                current.major,
                current.minor,
                current.patch.checked_add(1).ok_or(BumpError::Overflow)?,
            ),
            BumpLevel::Minor => (
                current.major,
                current.minor.checked_add(1).ok_or(BumpError::Overflow)?,
                0,
            ),
            BumpLevel::Major => (
                current.major.checked_add(1).ok_or(BumpError::Overflow)?,
                0,
                0,
            ),
        };
        Ok(Version::new(major, minor, patch))
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

/// Returns the next version and the [`AppliedBump`] that produced it, so
/// `--explain` does not recompute the mapping.
///
/// # Errors
///
/// Returns [`BumpError::Overflow`] when the bumped component would overflow
/// `u64::MAX`. Mapping is computed for `current`, so [`BumpError::StaleMapping`]
/// does not arise on this path.
pub fn apply_bump(
    current: &Version,
    requested: BumpLevel,
    versioning: Versioning,
) -> Result<(Version, AppliedBump), BumpError> {
    let applied = effective_bump(current, requested, versioning);
    let next = applied.next_version(current)?;
    Ok((next, applied))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    fn v(major: u64, minor: u64, patch: u64) -> Version {
        Version::new(major, minor, patch)
    }

    fn next(current: &Version, requested: BumpLevel, versioning: Versioning) -> Version {
        apply_bump(current, requested, versioning).expect("bump").0
    }

    #[test]
    fn patch_and_minor_ignore_versioning_and_major_line() {
        for versioning in [Versioning::ZeroMajor, Versioning::Semver] {
            assert_eq!(next(&v(0, 1, 3), BumpLevel::Patch, versioning), v(0, 1, 4));
            assert_eq!(next(&v(0, 1, 3), BumpLevel::Minor, versioning), v(0, 2, 0));
            assert_eq!(next(&v(1, 4, 2), BumpLevel::Patch, versioning), v(1, 4, 3));
            assert_eq!(next(&v(1, 4, 2), BumpLevel::Minor, versioning), v(1, 5, 0));
        }
    }

    /// ADR-0022: below 1.0.0 under zero-major, major → next minor, not 1.0.0.
    #[test]
    fn zero_major_maps_breaking_to_minor_below_one() {
        assert_eq!(
            next(&v(0, 1, 3), BumpLevel::Major, Versioning::ZeroMajor),
            v(0, 2, 0)
        );
        let applied = effective_bump(&v(0, 1, 3), BumpLevel::Major, Versioning::ZeroMajor);
        assert_eq!(applied.effective(), BumpLevel::Minor);
        assert!(applied.was_remapped());
    }

    /// Graduation is setting semver; the next major then yields 1.0.0.
    #[test]
    fn semver_major_on_zero_line_is_one_zero_zero() {
        assert_eq!(
            next(&v(0, 4, 1), BumpLevel::Major, Versioning::Semver),
            v(1, 0, 0)
        );
        assert!(!effective_bump(&v(0, 4, 1), BumpLevel::Major, Versioning::Semver).was_remapped());
    }

    /// At or above 1.0.0 the setting is inert.
    #[test]
    fn zero_major_is_inert_at_or_above_one() {
        assert_eq!(
            next(&v(1, 0, 0), BumpLevel::Major, Versioning::ZeroMajor),
            v(2, 0, 0)
        );
        assert_eq!(
            next(&v(1, 4, 3), BumpLevel::Major, Versioning::ZeroMajor),
            v(2, 0, 0)
        );
        assert_eq!(
            next(&v(1, 4, 3), BumpLevel::Major, Versioning::Semver),
            v(2, 0, 0)
        );
        assert!(
            !effective_bump(&v(1, 0, 0), BumpLevel::Major, Versioning::ZeroMajor).was_remapped()
        );
    }

    /// A feature does not become a patch below 1.0.0 — the knope divergence.
    #[test]
    fn feature_stays_minor_below_one() {
        assert_eq!(
            next(&v(0, 1, 3), BumpLevel::Minor, Versioning::ZeroMajor),
            v(0, 2, 0)
        );
        assert_ne!(
            next(&v(0, 1, 3), BumpLevel::Minor, Versioning::ZeroMajor),
            v(0, 1, 4)
        );
    }

    /// Mixed workspace: each package's own version decides, not a repo-wide line.
    #[test]
    fn mixed_workspace_evaluates_per_package_version() {
        let policy = Versioning::ZeroMajor;
        assert_eq!(next(&v(0, 1, 0), BumpLevel::Major, policy), v(0, 2, 0));
        assert_eq!(next(&v(1, 4, 1), BumpLevel::Major, policy), v(2, 0, 0));
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
        let bumped = next(&current, BumpLevel::Patch, Versioning::ZeroMajor);
        assert_eq!(bumped, v(0, 1, 4));
        assert!(bumped.pre.is_empty());
        assert!(bumped.build.is_empty());
    }

    #[test]
    fn remapped_major_also_strips_pre_and_build() {
        let mut current = v(0, 1, 3);
        current.pre = semver::Prerelease::new("rc.1").expect("pre");
        current.build = semver::BuildMetadata::new("git").expect("build");
        let bumped = next(&current, BumpLevel::Major, Versioning::ZeroMajor);
        assert_eq!(bumped, v(0, 2, 0));
        assert!(bumped.pre.is_empty());
        assert!(bumped.build.is_empty());
    }

    #[test]
    fn apply_bump_returns_mapping_with_version() {
        let (version, applied) =
            apply_bump(&v(0, 1, 3), BumpLevel::Major, Versioning::ZeroMajor).expect("bump");
        assert_eq!(version, v(0, 2, 0));
        assert_eq!(applied.requested(), BumpLevel::Major);
        assert_eq!(applied.effective(), BumpLevel::Minor);
        assert_eq!(applied.versioning(), Versioning::ZeroMajor);
        assert!(applied.was_remapped());
    }

    #[test]
    fn overflow_is_an_error() {
        assert_eq!(
            apply_bump(&v(0, 0, u64::MAX), BumpLevel::Patch, Versioning::ZeroMajor),
            Err(BumpError::Overflow)
        );
        assert_eq!(
            apply_bump(&v(0, u64::MAX, 0), BumpLevel::Minor, Versioning::ZeroMajor),
            Err(BumpError::Overflow)
        );
        assert_eq!(
            apply_bump(&v(u64::MAX, 0, 0), BumpLevel::Major, Versioning::Semver),
            Err(BumpError::Overflow)
        );
        // Zero-major remaps major→minor; overflow must key off the effective component.
        assert_eq!(
            apply_bump(&v(0, u64::MAX, 0), BumpLevel::Major, Versioning::ZeroMajor),
            Err(BumpError::Overflow)
        );
    }

    #[test]
    fn remapped_overflow_ignores_max_on_other_components() {
        assert_eq!(
            next(&v(0, 5, u64::MAX), BumpLevel::Major, Versioning::ZeroMajor),
            v(0, 6, 0)
        );
    }

    #[test]
    fn bump_to_u64_max_succeeds() {
        assert_eq!(
            next(&v(0, 0, u64::MAX - 1), BumpLevel::Patch, Versioning::Semver),
            v(0, 0, u64::MAX)
        );
    }

    #[test]
    fn next_version_refuses_stale_mapping() {
        let applied = effective_bump(&v(0, 1, 0), BumpLevel::Major, Versioning::ZeroMajor);
        assert_eq!(applied.effective(), BumpLevel::Minor);
        assert_eq!(
            applied.next_version(&v(1, 0, 0)),
            Err(BumpError::StaleMapping)
        );
    }

    #[test]
    fn bump_error_display() {
        assert_eq!(
            BumpError::Overflow.to_string(),
            "version component overflow"
        );
        assert_eq!(
            BumpError::StaleMapping.to_string(),
            "applied bump does not match this version"
        );
    }

    #[test]
    fn display_matches_config_spelling() {
        assert_eq!(BumpLevel::Patch.to_string(), "patch");
        assert_eq!(BumpLevel::Minor.to_string(), "minor");
        assert_eq!(BumpLevel::Major.to_string(), "major");
        assert_eq!(Versioning::ZeroMajor.to_string(), "zero-major");
        assert_eq!(Versioning::Semver.to_string(), "semver");
    }

    #[test]
    fn default_versioning_is_zero_major() {
        assert_eq!(Versioning::default(), Versioning::ZeroMajor);
    }
}
