//! Ecosystem-aware published bounds (ADR-0026).
//!
//! Cargo text is parsed with [`semver::VersionReq`]. npm text is parsed with
//! `js-semver` so bare pins, partials, and `||` keep npm meaning. Discovery
//! must call the constructor that matches the manifest's ecosystem; never
//! `VersionReq::parse` on npm strings.

use alloc::string::ToString;
use core::fmt;

use semver::{Version, VersionReq};

/// Bounds ADR-0010's gate compares a candidate version against.
#[derive(Clone, Debug)]
pub enum Bounds {
    /// Cargo / crates.io grammar.
    Cargo(VersionReq),
    /// npm / node-semver grammar (`js-semver`, ADR-0026).
    Npm(js_semver::Range),
}

impl PartialEq for Bounds {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Cargo(left), Self::Cargo(right)) => left == right,
            // `js_semver::Range` has no PartialEq; compare Display strings.
            // Same-parse / same Display only: `||` clause order is not
            // normalized (`^1||^2` ≠ `^2||^1`), so this Eq is not semantic
            // equivalence for rewrite or dedup.
            (Self::Npm(left), Self::Npm(right)) => left.to_string() == right.to_string(),
            _ => false,
        }
    }
}

impl Eq for Bounds {}

impl fmt::Display for Bounds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cargo(req) => write!(f, "{req}"),
            Self::Npm(range) => write!(f, "{range}"),
        }
    }
}

/// Why [`Bounds::from_cargo_text`] or [`Bounds::from_npm_text`] failed.
#[derive(Debug)]
pub enum BoundsError {
    Cargo(semver::Error),
    Npm(js_semver::SemverError),
}

impl fmt::Display for BoundsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cargo(error) => write!(f, "cargo range: {error}"),
            Self::Npm(error) => write!(f, "npm range: {error}"),
        }
    }
}

impl core::error::Error for BoundsError {}

impl Bounds {
    /// Parse Cargo manifest range text (`^1.2.3`, `=1.5.0`, comma AND).
    ///
    /// # Errors
    ///
    /// Returns [`BoundsError::Cargo`] when `text` is not valid Cargo range grammar.
    pub fn from_cargo_text(text: &str) -> Result<Self, BoundsError> {
        VersionReq::parse(text)
            .map(Self::Cargo)
            .map_err(BoundsError::Cargo)
    }

    /// Parse npm / package.json range text (bare exact, `||`, space AND, hyphen).
    ///
    /// # Errors
    ///
    /// Returns [`BoundsError::Npm`] when `text` is not valid npm / node-semver
    /// range grammar.
    pub fn from_npm_text(text: &str) -> Result<Self, BoundsError> {
        text.parse::<js_semver::Range>()
            .map(Self::Npm)
            .map_err(BoundsError::Npm)
    }

    #[must_use]
    pub fn matches(&self, version: &Version) -> bool {
        match self {
            Self::Cargo(req) => req.matches(version),
            Self::Npm(range) => {
                if let Ok(js_version) = version.to_string().parse::<js_semver::Version>() {
                    range.satisfies(&js_version)
                } else {
                    // Package versions Display-roundtrip into js-semver; a
                    // failure here is a programming fault, not an unsatisfied
                    // range. Over-cascade (false) until the planner can abort.
                    debug_assert!(
                        false,
                        "semver::Version Display failed to parse as js_semver::Version: {version}"
                    );
                    false
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use semver::Version;

    use super::Bounds;

    fn v(text: &str) -> Version {
        Version::parse(text).expect("test version")
    }

    #[test]
    fn npm_bare_is_exact_not_caret() {
        let bounds = Bounds::from_npm_text("1.5.0").expect("parse");
        assert!(bounds.matches(&v("1.5.0")));
        assert!(!bounds.matches(&v("1.6.0")));
    }

    #[test]
    fn npm_partial_major_admits_any_minor() {
        let bounds = Bounds::from_npm_text("1").expect("parse");
        assert!(bounds.matches(&v("1.9.0")));
        assert!(!bounds.matches(&v("2.0.0")));
    }

    #[test]
    fn npm_partial_minor_stays_within_patch_line() {
        let bounds = Bounds::from_npm_text("1.2").expect("parse");
        assert!(bounds.matches(&v("1.2.9")));
        assert!(!bounds.matches(&v("1.3.0")));
    }

    #[test]
    fn npm_union_admits_either_side() {
        let bounds = Bounds::from_npm_text("^1.0.0 || ^2.0.0").expect("parse");
        assert!(bounds.matches(&v("1.5.0")));
        assert!(bounds.matches(&v("2.1.0")));
        assert!(!bounds.matches(&v("3.0.0")));
    }

    #[test]
    fn npm_space_and_hyphen_forms_parse() {
        let and = Bounds::from_npm_text(">=1.2.3 <2.0.0").expect("space AND");
        assert!(and.matches(&v("1.5.0")));
        assert!(!and.matches(&v("2.0.0")));

        let hyphen = Bounds::from_npm_text("1.2.3 - 1.5.0").expect("hyphen");
        assert!(hyphen.matches(&v("1.2.3")));
        assert!(hyphen.matches(&v("1.4.0")));
        assert!(hyphen.matches(&v("1.5.0")));
        assert!(!hyphen.matches(&v("1.6.0")));
    }

    #[test]
    fn cargo_bare_remains_a_caret() {
        let bounds = Bounds::from_cargo_text("1.5.0").expect("parse");
        assert!(bounds.matches(&v("1.6.0")));
        assert!(!bounds.matches(&v("2.0.0")));
    }

    #[test]
    fn invalid_text_is_rejected_per_ecosystem() {
        assert!(Bounds::from_npm_text("not a range").is_err());
        assert!(Bounds::from_cargo_text("not a range").is_err());
    }

    #[test]
    fn npm_and_cargo_bounds_are_not_equal_even_when_text_matches() {
        let npm = Bounds::from_npm_text("^1.0.0").expect("npm");
        let cargo = Bounds::from_cargo_text("^1.0.0").expect("cargo");
        assert_ne!(npm, cargo);
        assert_eq!(npm, Bounds::from_npm_text("^1.0.0").expect("npm again"));
    }
}
