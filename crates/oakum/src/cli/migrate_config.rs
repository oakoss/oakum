//! The source tool's config as `migrate` reads it: the settings oakum carries
//! into `.changeset/_config.toml`, and the key names it leaves behind.
//!
//! Both source tools name the one carried setting `privatePackages`, with the
//! independent `version` and `tag` axes oakum spells `private-packages`
//! (ADR-0027), so one reader serves both files. Their schemas differ in one
//! place: changesets also accepts a bare boolean, which its own normalizer
//! expands to both axes, while bumpy's takes an object only. This reader
//! accepts the boolean from either file rather than tracking whose schema
//! permits it — reading a setting the writer plainly meant costs nothing, and
//! refusing it would drop the opt-in this reader exists to carry. Every other
//! key is reported, never guessed at.
//!
//! Both files are read when present, and an axis either opts in. A repository
//! migrating from one tool while a stale config from the other is still on
//! disk therefore carries both, which is the conservative direction: an opt-in
//! that should have lapsed costs a config line, where a dropped one costs a
//! release.
//!
//! One class of fault still refuses, before this module runs: a path that
//! escapes the checkout is rejected by the containment check in
//! [`super::detect_tools`], which inspects these same paths first. That guard
//! is about where a path leads rather than what a file holds, and loosening it
//! for a stale config would loosen it for everything.
//!
//! A file whose `privatePackages` carries one ill-typed axis is skipped whole,
//! so a valid sibling axis goes with it. That is the expensive side of the
//! trade above; the skip line says the file carried nothing, so the loss is
//! visible rather than silent.

use cap_std::fs::Dir;
use oakum::plan::Versioning;

use super::fs::read_text;
use super::owned_files::{ConfigSettings, PrivatePackages};
use super::CliError;

/// Where a source tool keeps its config, in the order the report names them.
const SOURCE_FILES: [&str; 2] = [".changeset/config.json", ".bumpy/_config.json"];

/// The only source key that maps onto an oakum config key.
const PRIVATE_PACKAGES: &str = "privatePackages";

/// One source config file, read once: what oakum carries out of it and what
/// it does not.
#[derive(Debug)]
pub(super) struct SourceConfig {
    /// Repository-relative, and still on disk after migrate: the report says so.
    pub(super) file: &'static str,
    /// Top-level key names with no oakum counterpart, sorted.
    pub(super) dropped: Vec<String>,
    /// `None` when the file did not set `privatePackages`.
    private_packages: Option<PrivatePackages>,
}

impl SourceConfig {
    /// The carried setting, only when it differs from oakum's default. Both
    /// axes off is what an absent `[private-packages]` already means, so
    /// there is nothing for the report to announce.
    pub(super) fn carried_private_packages(&self) -> Option<PrivatePackages> {
        self.private_packages.filter(|private| private.any())
    }
}

/// Every source config present in the repository, and a line for each one that
/// could not be used. Both tools can be detected at once; each file speaks for
/// itself.
///
/// A source config this function cannot open, read, or parse is reported and
/// skipped rather than refused. Both source tools load their config through
/// JSON5-tolerant readers, so a `//` note or a trailing comma is a file they
/// accept and `serde_json` does not. A path that escapes the checkout never
/// reaches here; see the module docs.
///
/// The report says what was skipped and what the skip may have cost, so a
/// dropped opt-in is visible rather than assumed absent, and `check` refuses
/// afterwards if the result manages nothing — the two halves cover each other.
pub(super) fn read_source_configs(dir: &Dir) -> (Vec<SourceConfig>, Vec<String>) {
    let mut configs = Vec::new();
    let mut unreadable = Vec::new();
    for file in SOURCE_FILES {
        // A directory or an unreadable mode at this path is no more the
        // migration's business than a trailing comma inside it.
        let outcome = match read_text(dir, file) {
            Ok(None) => continue,
            Ok(Some(body)) => parse_source_config(file, &body),
            Err(err) => Err(err),
        };
        match outcome {
            Ok(config) => configs.push(config),
            Err(err) => unreadable.push(format!(
                "could not use `{file}` ({err}); no settings carried from it, \
                 including `{PRIVATE_PACKAGES}` if it set one"
            )),
        }
    }
    (configs, unreadable)
}

/// What `migrate` writes into `_config.toml`: both intent mechanisms on, the
/// versioning it inferred, and the source tools' carried settings.
pub(super) fn migrated_settings(
    versioning: Versioning,
    configs: &[SourceConfig],
) -> ConfigSettings {
    ConfigSettings {
        change_files: true,
        conventional_commits: true,
        versioning,
        private_packages: carried_private_packages(configs),
    }
}

/// On if any source file turned that axis on. Two source tools that disagree
/// would each have opted their own packages in, and oakum has one config for
/// the repository.
fn carried_private_packages(configs: &[SourceConfig]) -> PrivatePackages {
    configs
        .iter()
        .filter_map(|config| config.private_packages)
        .fold(PrivatePackages::default(), PrivatePackages::union)
}

fn parse_source_config(
    file: &'static str,
    body: &str,
) -> Result<SourceConfig, Box<dyn std::error::Error>> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|err| CliError::new(format!("`{file}` is not valid JSON: {err}")))?;
    let Some(object) = value.as_object() else {
        return Err(Box::new(CliError::new(format!(
            "`{file}` is not a JSON object"
        ))));
    };
    let private_packages = object
        .get(PRIVATE_PACKAGES)
        .map(|value| parse_private_packages(file, value))
        .transpose()?;
    let mut dropped: Vec<String> = object
        .keys()
        .filter(|key| key.as_str() != PRIVATE_PACKAGES)
        .cloned()
        .collect();
    dropped.sort();
    Ok(SourceConfig {
        file,
        dropped,
        private_packages,
    })
}

fn parse_private_packages(
    file: &'static str,
    value: &serde_json::Value,
) -> Result<PrivatePackages, Box<dyn std::error::Error>> {
    // Changesets accepts a bare boolean here and expands it to both axes; its
    // schema is `object | boolean` and its normalizer reads a missing value as
    // false.
    if let serde_json::Value::Bool(on) = value {
        return Ok(PrivatePackages {
            version: *on,
            tag: *on,
        });
    }
    let Some(object) = value.as_object() else {
        return Err(Box::new(CliError::new(format!(
            "`{PRIVATE_PACKAGES}` in `{file}` is neither an object nor a boolean"
        ))));
    };
    Ok(PrivatePackages {
        version: axis(file, object, "version")?,
        tag: axis(file, object, "tag")?,
    })
}

fn axis(
    file: &'static str,
    object: &serde_json::Map<String, serde_json::Value>,
    name: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    // A missing key is the source tool's own default; an explicit `null` is a
    // value that is not a boolean, and migrating it as `false` would turn a
    // malformed setting into a silent opt-out.
    match object.get(name) {
        None => Ok(false),
        Some(serde_json::Value::Bool(on)) => Ok(*on),
        Some(other) => Err(Box::new(CliError::new(format!(
            "`{PRIVATE_PACKAGES}.{name}` in `{file}` is `{other}`, not a boolean"
        )))),
    }
}

#[cfg(test)]
mod reading {
    use super::{carried_private_packages, parse_source_config};
    use crate::cli::owned_files::PrivatePackages;

    fn parse(file: &'static str, body: &str) -> super::SourceConfig {
        parse_source_config(file, body).expect("source config")
    }

    #[test]
    fn private_packages_is_carried_and_never_reported_as_dropped() {
        let config = parse(
            ".bumpy/_config.json",
            r#"{"privatePackages": {"version": true, "tag": true}, "baseBranch": "main"}"#,
        );
        assert_eq!(
            config.carried_private_packages(),
            Some(PrivatePackages {
                version: true,
                tag: true
            })
        );
        assert_eq!(config.dropped, vec![String::from("baseBranch")]);
    }

    #[test]
    fn axes_are_independent_and_a_missing_axis_is_off() {
        let config = parse(
            ".changeset/config.json",
            r#"{"privatePackages": {"tag": true}}"#,
        );
        assert_eq!(
            config.carried_private_packages(),
            Some(PrivatePackages {
                version: false,
                tag: true
            })
        );
    }

    #[test]
    fn both_axes_off_matches_the_default_so_nothing_is_announced() {
        let config = parse(
            ".changeset/config.json",
            r#"{"privatePackages": {"version": false, "tag": false}}"#,
        );
        assert_eq!(config.carried_private_packages(), None);
        assert!(config.dropped.is_empty());
    }

    #[test]
    fn a_source_without_the_key_carries_nothing() {
        let config = parse(".changeset/config.json", r#"{"access": "public"}"#);
        assert_eq!(config.carried_private_packages(), None);
        assert_eq!(config.dropped, vec![String::from("access")]);
        assert_eq!(
            carried_private_packages(&[config]),
            PrivatePackages::default()
        );
    }

    #[test]
    fn two_sources_union_their_axes() {
        let carried = carried_private_packages(&[
            parse(
                ".changeset/config.json",
                r#"{"privatePackages": {"version": true}}"#,
            ),
            parse(
                ".bumpy/_config.json",
                r#"{"privatePackages": {"tag": true}}"#,
            ),
        ]);
        assert_eq!(
            carried,
            PrivatePackages {
                version: true,
                tag: true
            }
        );
    }

    #[test]
    fn a_non_boolean_axis_refuses_and_names_the_file() {
        let err = parse_source_config(
            ".bumpy/_config.json",
            r#"{"privatePackages": {"version": "yes"}}"#,
        )
        .expect_err("non-boolean axis");
        assert_eq!(
            err.to_string(),
            "`privatePackages.version` in `.bumpy/_config.json` is `\"yes\"`, not a boolean"
        );
    }

    #[test]
    fn a_private_packages_that_is_neither_object_nor_boolean_refuses() {
        let err = parse_source_config(".bumpy/_config.json", r#"{"privatePackages": "yes"}"#)
            .expect_err("a string is neither");
        assert_eq!(
            err.to_string(),
            "`privatePackages` in `.bumpy/_config.json` is neither an object nor a boolean"
        );
    }

    /// Changesets' own normalizer expands a bare boolean to both axes.
    #[test]
    fn a_changesets_boolean_private_packages_opts_both_axes_in() {
        let carried = carried_private_packages(&[parse(
            ".changeset/config.json",
            r#"{"privatePackages": true}"#,
        )]);
        assert_eq!(
            carried,
            PrivatePackages {
                version: true,
                tag: true
            }
        );
        let off = carried_private_packages(&[parse(
            ".changeset/config.json",
            r#"{"privatePackages": false}"#,
        )]);
        assert_eq!(off, PrivatePackages::default());
    }

    #[test]
    fn a_non_object_config_refuses_and_names_the_file() {
        let err = parse_source_config(".bumpy/_config.json", "[]").expect_err("array");
        assert_eq!(
            err.to_string(),
            "`.bumpy/_config.json` is not a JSON object"
        );
        let err = parse_source_config(".bumpy/_config.json", "{").expect_err("truncated");
        assert!(
            err.to_string()
                .starts_with("`.bumpy/_config.json` is not valid JSON: "),
            "{err}"
        );
    }
    #[test]
    fn an_explicit_null_axis_refuses_like_any_other_non_boolean() {
        let err = parse_source_config(
            ".bumpy/_config.json",
            r#"{"privatePackages": {"version": null}}"#,
        )
        .expect_err("null is not a boolean");
        assert!(err.to_string().contains("not a boolean"), "{err}");
    }
}
