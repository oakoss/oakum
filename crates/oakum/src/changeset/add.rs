//! Pure helpers for `oakum add`: package-list parsing and filename stems.
//!
//! Disk writes, discovery, and prompts live in the binary (ADR-0002).

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::plan::{BumpLevel, BumpLevelParseError};

/// One `name:level` pair from a packages list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageSpec {
    name: String,
    level: BumpLevel,
}

impl PackageSpec {
    #[must_use]
    pub fn new(name: String, level: BumpLevel) -> Self {
        Self { name, level }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn level(&self) -> BumpLevel {
        self.level
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackagesError {
    Empty,
    EmptyEntry,
    MissingLevel { entry: String },
    EmptyName { entry: String },
    EmptyLevel { entry: String },
    InvalidPackageName { name: String },
    DuplicatePackage { name: String },
    UnknownLevel(BumpLevelParseError),
}

impl fmt::Display for PackagesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("package list must include at least one `name:level` pair"),
            Self::EmptyEntry => f.write_str("package list contains an empty entry"),
            Self::MissingLevel { entry } => write!(
                f,
                "package list entry `{entry}` is missing a `:level` (expected `name:level`)"
            ),
            Self::EmptyName { entry } => {
                write!(f, "package list entry `{entry}` has an empty package name")
            }
            Self::EmptyLevel { entry } => {
                write!(f, "package list entry `{entry}` has an empty bump level")
            }
            Self::InvalidPackageName { name } => write!(
                f,
                "package name `{name}` is not writable in the changeset-format intersection"
            ),
            Self::DuplicatePackage { name } => {
                write!(
                    f,
                    "package `{name}` appears more than once in the package list"
                )
            }
            Self::UnknownLevel(err) => write!(f, "{err}"),
        }
    }
}

impl core::error::Error for PackagesError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::UnknownLevel(err) => Some(err),
            _ => None,
        }
    }
}

/// Parse comma-separated `name:level` pairs (`core:minor,@scope/pkg:patch`).
///
/// The level is the segment after the last `:`, so scoped npm names keep their
/// single slash and never need quoting in the flag value.
///
/// # Errors
///
/// [`PackagesError`] when the list is empty or any entry is malformed.
pub fn parse_packages_list(text: &str) -> Result<Vec<PackageSpec>, PackagesError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(PackagesError::Empty);
    }

    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for raw in trimmed.split(',') {
        let entry = raw.trim();
        if entry.is_empty() {
            return Err(PackagesError::EmptyEntry);
        }
        let Some((name, level_text)) = entry.rsplit_once(':') else {
            return Err(PackagesError::MissingLevel {
                entry: String::from(entry),
            });
        };
        let name = name.trim();
        let level_text = level_text.trim();
        if name.is_empty() {
            return Err(PackagesError::EmptyName {
                entry: String::from(entry),
            });
        }
        if level_text.is_empty() {
            return Err(PackagesError::EmptyLevel {
                entry: String::from(entry),
            });
        }
        validate_list_name(name)?;
        if !seen.insert(String::from(name)) {
            return Err(PackagesError::DuplicatePackage {
                name: String::from(name),
            });
        }
        let level = level_text.parse().map_err(PackagesError::UnknownLevel)?;
        out.push(PackageSpec {
            name: String::from(name),
            level,
        });
    }
    Ok(out)
}

fn validate_list_name(name: &str) -> Result<(), PackagesError> {
    if name != name.trim() || name.contains(['"', '\'', '\n', '\r', ':']) {
        return Err(PackagesError::InvalidPackageName {
            name: String::from(name),
        });
    }
    Ok(())
}

/// Slugify a user-supplied stem for `.changeset/` filenames.
///
/// An empty result becomes `change`.
#[must_use]
pub fn slugify(stem: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for ch in stem.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(ch.to_ascii_lowercase());
        } else {
            pending_dash = !out.is_empty();
        }
    }
    if out.is_empty() {
        String::from("change")
    } else {
        out
    }
}

/// Default filename stem when `--name` is omitted (`oakum-` + hex seed).
#[must_use]
pub fn default_stem(seed: u64) -> String {
    alloc::format!("oakum-{seed:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::BumpLevel;

    #[test]
    fn parse_packages_accepts_scoped_and_plain() {
        let specs = parse_packages_list("core:minor, @oakum/cli:patch").expect("parse");
        assert_eq!(specs[0].name(), "core");
        assert_eq!(specs[0].level(), BumpLevel::Minor);
        assert_eq!(specs[1].name(), "@oakum/cli");
        assert_eq!(specs[1].level(), BumpLevel::Patch);
    }

    #[test]
    fn parse_packages_rejects_empty_and_bad_level() {
        assert_eq!(parse_packages_list("").unwrap_err(), PackagesError::Empty);
        assert!(matches!(
            parse_packages_list("core").unwrap_err(),
            PackagesError::MissingLevel { .. }
        ));
        assert!(matches!(
            parse_packages_list("core:weird").unwrap_err(),
            PackagesError::UnknownLevel(_)
        ));
        assert!(matches!(
            parse_packages_list("core:patch,").unwrap_err(),
            PackagesError::EmptyEntry
        ));
        assert!(matches!(
            parse_packages_list(":patch").unwrap_err(),
            PackagesError::EmptyName { .. }
        ));
        assert!(matches!(
            parse_packages_list("core:").unwrap_err(),
            PackagesError::EmptyLevel { .. }
        ));
        assert!(matches!(
            parse_packages_list("a:patch,a:minor").unwrap_err(),
            PackagesError::DuplicatePackage { .. }
        ));
    }

    #[test]
    fn slugify_collapses_and_lowercases() {
        assert_eq!(slugify("Hello World!!"), "hello-world");
        assert_eq!(slugify("  "), "change");
        assert_eq!(slugify("Already-ok"), "already-ok");
    }

    #[test]
    fn default_stem_is_hex() {
        assert_eq!(default_stem(0x_dead_beef), "oakum-00000000deadbeef");
    }
}
