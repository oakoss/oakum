//! A pnpm workspace's root files compared as data on each side of a change
//! (ADR-0039): which catalog entries moved, and which members' runtime
//! dependencies name them.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use super::catalog_file::CatalogFile;
use super::npm_spec::split_name_version;

/// pnpm reads a bare `catalog:` from the default catalog.
const DEFAULT_CATALOG: &str = "default";

/// A catalog entry: the catalog's name and the package key in it.
pub type CatalogEntry = (String, String);

/// A changed entry that installs one package on both sides, moving only its
/// range: the shape of the pin `version` rewrites.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repinned {
    pub package: String,
    /// The earlier side's range.
    pub from: String,
    /// The later side's range.
    pub to: String,
}

/// Catalog entries added, removed, or given another value between `base` and
/// `head`, each the text of `pnpm-workspace.yaml` there, `None` where absent.
/// An entry that installs the same package on both sides (its key, or the
/// package an `npm:` alias names) carries it as [`Repinned`].
///
/// # Errors
///
/// A side that is not a catalog file pnpm would read, named by `base` or
/// `head`.
pub fn changed_catalog_entries(
    base: Option<&str>,
    head: Option<&str>,
) -> Result<BTreeMap<CatalogEntry, Option<Repinned>>, (&'static str, String)> {
    let base = entries(base).map_err(|err| ("base", err))?;
    let head = entries(head).map_err(|err| ("head", err))?;
    Ok(base
        .keys()
        .chain(head.keys())
        .filter(|entry| base.get(*entry) != head.get(*entry))
        .map(|entry| {
            let key = entry.1.as_str();
            let before = base.get(entry).and_then(Option::as_deref);
            let after = head.get(entry).and_then(Option::as_deref);
            let repinned = match (
                before.map(|value| installed(key, value)),
                after.map(|value| installed(key, value)),
            ) {
                (Some((before, from)), Some((after, to))) if before == after => Some(Repinned {
                    package: after.to_owned(),
                    from: from.trim().to_owned(),
                    to: to.trim().to_owned(),
                }),
                _ => None,
            };
            (entry.clone(), repinned)
        })
        .collect())
}

/// The package an entry installs and its range: an `npm:<name>@<range>` alias
/// names its own, anything else installs the key at the whole value.
fn installed<'a>(key: &'a str, value: &'a str) -> (&'a str, &'a str) {
    value
        .trim()
        .strip_prefix("npm:")
        .and_then(split_name_version)
        .unwrap_or((key, value))
}

fn entries(text: Option<&str>) -> Result<BTreeMap<CatalogEntry, Option<String>>, String> {
    let Some(text) = text else {
        return Ok(BTreeMap::new());
    };
    let file = CatalogFile::parse(text).map_err(|err| err.to_string())?;
    if file.has_duplicate_default() {
        return Err(String::from(
            "both `catalog` and `catalogs.default` define the default catalog",
        ));
    }
    if file.has_null_named_table() {
        return Err(String::from("a table under `catalogs` is null"));
    }
    let mut out = BTreeMap::new();
    for (package, pin) in file.catalog.unwrap_or_default() {
        out.insert((DEFAULT_CATALOG.to_owned(), package), pin);
    }
    for (name, pins) in file.catalogs.unwrap_or_default() {
        for (package, pin) in pins.unwrap_or_default() {
            out.insert((name.clone(), package), pin);
        }
    }
    Ok(out)
}

type Section = Option<BTreeMap<String, serde_json::Value>>;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeSections {
    #[serde(default)]
    dependencies: Section,
    #[serde(default)]
    optional_dependencies: Section,
    #[serde(default)]
    peer_dependencies: Section,
}

/// The catalog entries a member's `package.json` takes a runtime dependency
/// from: `dependencies`, `optionalDependencies` and `peerDependencies`, the
/// edges a release cascades along (ADR-0008). pnpm looks an entry up by the
/// manifest key, and an `optionalDependencies` entry overrides a
/// `dependencies` one of the same key, as discovery reads it.
///
/// # Errors
///
/// A manifest that is not JSON with those sections as objects.
pub fn runtime_catalog_uses(manifest: &str) -> Result<BTreeSet<CatalogEntry>, String> {
    let sections: RuntimeSections =
        serde_json::from_str(manifest).map_err(|err| err.to_string())?;
    let optional = sections.optional_dependencies.unwrap_or_default();
    let dependencies = sections
        .dependencies
        .unwrap_or_default()
        .into_iter()
        .filter(|(key, _)| !optional.contains_key(key));
    Ok(dependencies
        .chain(optional.clone())
        .chain(sections.peer_dependencies.unwrap_or_default())
        .filter_map(|(key, value)| {
            let catalog = value.as_str()?.trim().strip_prefix("catalog:")?.trim();
            let catalog = if catalog.is_empty() {
                DEFAULT_CATALOG
            } else {
                catalog
            };
            Some((catalog.to_owned(), key))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(catalog: &str, package: &str) -> CatalogEntry {
        (catalog.to_owned(), package.to_owned())
    }

    fn changed(base: Option<&str>, head: Option<&str>) -> BTreeSet<CatalogEntry> {
        changed_catalog_entries(base, head)
            .expect("parses")
            .into_keys()
            .collect()
    }

    #[test]
    fn a_moved_pin_is_changed_and_an_untouched_one_is_not() {
        let base = "catalog:\n  a: ^1.0.0\n  b: ^2.0.0\ncatalogs:\n  old:\n    c: 1.0.0\n";
        let head =
            "catalog:\n  a: ^1.3.0\n  b: ^2.0.0\n  d: ^4.0.0\ncatalogs:\n  old:\n    c: 1.0.0\n";
        assert_eq!(
            changed(Some(base), Some(head)),
            BTreeSet::from([entry("default", "a"), entry("default", "d")])
        );
    }

    #[test]
    fn the_default_catalog_reads_alike_from_either_table() {
        let base = "catalog:\n  a: ^1.0.0\n";
        let head = "catalogs:\n  default:\n    a: ^1.0.0\n  named:\n    a: ^1.0.0\n";
        assert_eq!(
            changed(Some(base), Some(head)),
            BTreeSet::from([entry("named", "a")])
        );
    }

    #[test]
    fn an_added_file_changes_every_entry_and_a_garbled_side_is_named() {
        assert_eq!(
            changed(None, Some("catalog:\n  a: ^1.0.0\n")),
            BTreeSet::from([entry("default", "a")])
        );
        let (side, _) = changed_catalog_entries(Some("catalog: [\n"), None).expect_err("not yaml");
        assert_eq!(side, "base");
        let (side, _) = changed_catalog_entries(None, Some("catalogs:\n  unused: null\n"))
            .expect_err("pnpm refuses a null table");
        assert_eq!(side, "head");
    }

    #[test]
    fn only_an_entry_installing_one_package_on_both_sides_is_repinned() {
        let found = changed_catalog_entries(
            Some("catalog:\n  pad: npm:lib@^1.0.0\n  keep: npm:lib@^1.0.0\n  plain: ^1.0.0\n"),
            Some("catalog:\n  pad: npm:other@^1.0.0\n  keep: npm:lib@^1.1.0\n  plain: ^1.1.0\n  new: ^1.0.0\n"),
        )
        .expect("parses");
        let repinned = |package: &str, from: &str, to: &str| {
            Some(Repinned {
                package: package.to_owned(),
                from: from.to_owned(),
                to: to.to_owned(),
            })
        };
        assert_eq!(
            found,
            BTreeMap::from([
                (
                    entry("default", "keep"),
                    repinned("lib", "^1.0.0", "^1.1.0")
                ),
                (entry("default", "new"), None),
                (entry("default", "pad"), None),
                (
                    entry("default", "plain"),
                    repinned("plain", "^1.0.0", "^1.1.0")
                ),
            ])
        );
    }

    #[test]
    fn an_optional_entry_overrides_a_dependency_of_the_same_key() {
        let manifest = r#"{
            "dependencies": { "a": "catalog:", "b": "^1.0.0" },
            "optionalDependencies": { "a": "^2.0.0", "b": "catalog:" },
            "peerDependencies": null
        }"#;
        assert_eq!(
            runtime_catalog_uses(manifest).expect("json"),
            BTreeSet::from([entry("default", "b")])
        );
    }

    #[test]
    fn runtime_sections_name_their_catalog_and_dev_ones_do_not() {
        let manifest = r#"{
            "dependencies": { "a": "catalog:", "x": "^1.0.0" },
            "optionalDependencies": { "b": " catalog:named " },
            "peerDependencies": { "c": "catalog:" },
            "devDependencies": { "d": "catalog:" }
        }"#;
        assert_eq!(
            runtime_catalog_uses(manifest).expect("json"),
            BTreeSet::from([
                entry("default", "a"),
                entry("named", "b"),
                entry("default", "c"),
            ])
        );
    }
}
