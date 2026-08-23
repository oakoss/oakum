//! Ecosystem-aware published bounds (ADR-0026).
//!
//! Cargo text is parsed with [`semver::VersionReq`]. npm text is parsed with
//! `js-semver` so bare pins, partials, and `||` keep npm meaning. Discovery
//! must call the constructor that matches the manifest's ecosystem; never
//! `VersionReq::parse` on npm strings.

use alloc::string::{String, ToString};
use core::fmt;

use semver::{Version, VersionReq};

/// Bounds ADR-0010's gate compares a candidate version against.
#[derive(Clone, Debug)]
pub enum Bounds {
    /// Cargo / crates.io grammar.
    Cargo(VersionReq),
    /// npm / node-semver grammar (`js-semver`, ADR-0026).
    ///
    /// `authored` is the manifest spelling. js-semver Display expands `^`/`~`
    /// into `>= … < …`, so rewrite must not round-trip through Display.
    Npm {
        parsed: js_semver::Range,
        authored: String,
    },
}

impl PartialEq for Bounds {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Cargo(left), Self::Cargo(right)) => left == right,
            // `js_semver::Range` has no PartialEq; compare Display strings.
            // Same-parse / same Display only: `||` clause order is not
            // normalized (`^1||^2` ≠ `^2||^1`), so this Eq is not semantic
            // equivalence for rewrite or dedup.
            (
                Self::Npm {
                    parsed: left,
                    authored: _,
                },
                Self::Npm {
                    parsed: right,
                    authored: _,
                },
            ) => left.to_string() == right.to_string(),
            _ => false,
        }
    }
}

impl Eq for Bounds {}

impl fmt::Display for Bounds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cargo(req) => write!(f, "{req}"),
            Self::Npm { authored, .. } => f.write_str(authored),
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

fn retarget_cargo(req: &VersionReq, new: &Version) -> Option<String> {
    if req.comparators.is_empty() {
        return Some(String::from("*"));
    }
    if let [only] = req.comparators.as_slice() {
        let rewritten = VersionReq {
            comparators: alloc::vec![semver::Comparator {
                op: only.op,
                major: new.major,
                minor: Some(new.minor),
                patch: Some(new.patch),
                pre: new.pre.clone(),
            }],
        };
        return Some(rewritten.to_string());
    }
    None
}

fn retarget_npm(text: &str, new: &Version) -> Option<String> {
    let n = new.to_string();
    if matches!(text.trim(), "*" | "x" | "X") {
        return Some(text.trim().to_string());
    }
    for prefix in ["^", "~=", "~>", "~", ">=", "<=", "=", ">", "<"] {
        if let Some(rest) = text.strip_prefix(prefix) {
            if looks_like_version(rest) {
                return Some(alloc::format!("{prefix}{n}"));
            }
            if let Some(wild) = retarget_wildcard(rest, new) {
                return Some(alloc::format!("{prefix}{wild}"));
            }
        }
    }
    if looks_like_version(text) {
        if text.trim().starts_with('v') {
            return Some(alloc::format!("v{n}"));
        }
        return Some(n);
    }
    retarget_wildcard(text, new)
}

fn retarget_wildcard(text: &str, new: &Version) -> Option<String> {
    let trimmed = text.trim();
    let (v_prefix, body) = match trimmed.strip_prefix('v') {
        Some(rest) => ("v", rest),
        None => ("", trimmed),
    };
    let news = [
        new.major.to_string(),
        new.minor.to_string(),
        new.patch.to_string(),
    ];
    let mut any_wild = false;
    let mut all_wild = true;
    let mut out = String::new();
    let mut i = 0;
    for part in body.split('.') {
        if i >= 3 || part.is_empty() {
            return None;
        }
        if i > 0 {
            out.push('.');
        }
        if matches!(part, "*" | "x" | "X") {
            any_wild = true;
            out.push_str(part);
        } else if part.bytes().all(|b| b.is_ascii_digit()) {
            all_wild = false;
            out.push_str(&news[i]);
        } else {
            return None;
        }
        i += 1;
    }
    if i == 0 || !any_wild {
        return None;
    }
    if all_wild {
        return Some(trimmed.to_string());
    }
    Some(alloc::format!("{v_prefix}{out}"))
}

fn looks_like_version(text: &str) -> bool {
    let text = text.trim().strip_prefix('v').unwrap_or(text.trim());
    if Version::parse(text).is_ok() {
        return true;
    }
    // npm allows `1` and `1.2`; semver::Version does not.
    let mut n = 0;
    for part in text.split('.') {
        n += 1;
        if n > 3 || part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
    }
    n == 1 || n == 2
}

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
            .map(|parsed| Self::Npm {
                parsed,
                authored: String::from(text),
            })
            .map_err(BoundsError::Npm)
    }

    /// Manifest range text with the same operator, pointed at `new`.
    ///
    /// A single comparator keeps its `Op` (`^`, `~`, `=`). `None` when the
    /// authored form is not a single operator or wildcard (`||`, AND-ranges).
    #[must_use]
    pub fn retargeted(&self, new: &Version) -> Option<String> {
        match self {
            Self::Cargo(req) => retarget_cargo(req, new),
            Self::Npm { authored, .. } => retarget_npm(authored, new),
        }
    }

    #[must_use]
    pub fn matches(&self, version: &Version) -> bool {
        match self {
            Self::Cargo(req) => req.matches(version),
            Self::Npm { parsed, .. } => {
                let range = parsed;
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

    #[test]
    fn cargo_caret_keeps_the_operator() {
        let bounds = Bounds::from_cargo_text("^0.1.3").expect("parse");
        assert_eq!(bounds.retargeted(&v("0.2.0")).as_deref(), Some("^0.2.0"));
    }

    #[test]
    fn cargo_exact_keeps_the_operator() {
        let bounds = Bounds::from_cargo_text("=0.1.3").expect("parse");
        assert_eq!(bounds.retargeted(&v("0.2.0")).as_deref(), Some("=0.2.0"));
    }

    #[test]
    fn npm_caret_keeps_the_operator() {
        let bounds = Bounds::from_npm_text("^0.1.3").expect("parse");
        assert_eq!(bounds.retargeted(&v("0.2.0")).as_deref(), Some("^0.2.0"));
    }

    #[test]
    fn npm_bare_stays_exact() {
        let bounds = Bounds::from_npm_text("0.1.3").expect("parse");
        assert_eq!(bounds.retargeted(&v("0.2.0")).as_deref(), Some("0.2.0"));
    }

    #[test]
    fn npm_tilde_on_one_x_keeps_the_operator() {
        let bounds = Bounds::from_npm_text("~1.2.3").expect("parse");
        assert_eq!(bounds.retargeted(&v("1.5.0")).as_deref(), Some("~1.5.0"));
    }

    #[test]
    fn cargo_tilde_keeps_the_operator() {
        let bounds = Bounds::from_cargo_text("~1.2.3").expect("parse");
        assert_eq!(bounds.retargeted(&v("1.5.0")).as_deref(), Some("~1.5.0"));
    }

    #[test]
    fn npm_partial_tilde_keeps_the_operator() {
        let bounds = Bounds::from_npm_text("~1.2").expect("parse");
        assert_eq!(bounds.retargeted(&v("1.5.0")).as_deref(), Some("~1.5.0"));
    }

    #[test]
    fn npm_star_stays_a_star() {
        let bounds = Bounds::from_npm_text("*").expect("parse");
        assert_eq!(bounds.retargeted(&v("2.0.0")).as_deref(), Some("*"));
    }

    #[test]
    fn cargo_star_stays_a_star() {
        let bounds = Bounds::from_cargo_text("*").expect("parse");
        assert_eq!(bounds.retargeted(&v("2.0.0")).as_deref(), Some("*"));
    }

    #[test]
    fn npm_wildcard_keeps_its_shape() {
        let patch = Bounds::from_npm_text("1.2.*").expect("parse");
        assert_eq!(patch.retargeted(&v("2.0.0")).as_deref(), Some("2.0.*"));
        let minor = Bounds::from_npm_text("1.x").expect("parse");
        assert_eq!(minor.retargeted(&v("2.0.0")).as_deref(), Some("2.x"));
        let all = Bounds::from_npm_text("*.*").expect("parse");
        assert_eq!(all.retargeted(&v("2.0.0")).as_deref(), Some("*.*"));
    }

    #[test]
    fn npm_tilde_eq_and_v_prefix_keep_spelling() {
        let tilde_eq = Bounds::from_npm_text("~=1.2.3").expect("parse");
        assert_eq!(tilde_eq.retargeted(&v("2.0.0")).as_deref(), Some("~=2.0.0"));
        let tilde_gt = Bounds::from_npm_text("~>1.2.3").expect("parse");
        assert_eq!(tilde_gt.retargeted(&v("2.0.0")).as_deref(), Some("~>2.0.0"));
        let v_pin = Bounds::from_npm_text("v1.2.3").expect("parse");
        assert_eq!(v_pin.retargeted(&v("2.0.0")).as_deref(), Some("v2.0.0"));
    }

    #[test]
    fn npm_union_is_not_collapsed_to_a_caret() {
        let bounds = Bounds::from_npm_text("^1.0.0 || ^2.0.0").expect("parse");
        assert_eq!(bounds.retargeted(&v("3.0.0")), None);
    }

    #[test]
    fn cargo_and_range_is_not_collapsed_to_a_caret() {
        let bounds = Bounds::from_cargo_text(">=0.1.0, <0.2.0").expect("parse");
        assert_eq!(bounds.retargeted(&v("0.2.0")), None);
    }
}
