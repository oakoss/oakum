//! Shared pnpm/yarn catalog file schema.
//!
//! Discover and the YAML writer share this `serde_saphyr` mapping so XOR and
//! "is this a string pin" do not drift. `Option<String>` keeps a null pin from
//! failing the whole file: the table is still present, the pin is not a string.

use std::collections::BTreeMap;

use serde::Deserialize;

type CatalogPins = BTreeMap<String, Option<String>>;
type NamedCatalogs = BTreeMap<String, Option<CatalogPins>>;

/// Absent vs present matters: empty `catalog: {}` still owns the default
/// slot, and empty `catalogs: {}` still owns the named-table slot.
/// `None` is a missing key or YAML null.
#[derive(Debug, Deserialize)]
pub(crate) struct CatalogFile {
    pub catalog: Option<CatalogPins>,
    pub catalogs: Option<NamedCatalogs>,
}

impl CatalogFile {
    pub(crate) fn parse(text: &str) -> Result<Self, serde_saphyr::Error> {
        serde_saphyr::from_str(text)
    }

    pub(crate) fn string_at<'a>(&'a self, path: &[&str]) -> Option<&'a str> {
        match path {
            ["catalog", package] => self
                .catalog
                .as_ref()?
                .get(*package)
                .and_then(Option::as_deref),
            ["catalogs", name, package] => self
                .catalogs
                .as_ref()?
                .get(*name)?
                .as_ref()?
                .get(*package)
                .and_then(Option::as_deref),
            _ => None,
        }
    }

    pub(crate) fn string_pins(map: CatalogPins) -> BTreeMap<String, String> {
        map.into_iter().filter_map(|(k, v)| Some((k, v?))).collect()
    }

    pub(crate) fn string_tables(
        tables: NamedCatalogs,
    ) -> BTreeMap<String, BTreeMap<String, String>> {
        tables
            .into_iter()
            .filter_map(|(name, pins)| Some((name, Self::string_pins(pins?))))
            .collect()
    }

    pub(crate) fn has_null_named_table(&self) -> bool {
        self.catalogs
            .as_ref()
            .is_some_and(|tables| tables.values().any(Option::is_none))
    }

    pub(crate) fn has_default_table(&self) -> bool {
        self.catalogs
            .as_ref()
            .and_then(|tables| tables.get("default"))
            .is_some_and(Option::is_some)
    }

    /// True when `catalog` or `catalogs` deserialized as a mapping, including
    /// `{}`. Missing or null is false.
    pub(crate) fn has_catalog_table(&self) -> bool {
        self.catalog.is_some() || self.catalogs.is_some()
    }
}

pub(crate) enum CatalogTarget<'a> {
    Duplicate,
    At {
        path: Vec<&'a str>,
        missing_path: String,
    },
}

pub(crate) fn catalog_target<'a>(
    catalog: Option<&'a str>,
    package: &'a str,
    has_catalog: bool,
    has_default: bool,
) -> CatalogTarget<'a> {
    match catalog {
        Some(name) => CatalogTarget::At {
            path: vec!["catalogs", name, package],
            missing_path: format!("catalogs/{name}/{package}"),
        },
        None if has_catalog && has_default => CatalogTarget::Duplicate,
        None if has_catalog => CatalogTarget::At {
            path: vec!["catalog", package],
            missing_path: format!("catalog/{package}"),
        },
        None if has_default => CatalogTarget::At {
            path: vec!["catalogs", "default", package],
            missing_path: format!("catalog/{package}"),
        },
        None => CatalogTarget::At {
            path: vec!["catalog", package],
            missing_path: format!("catalog/{package}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_object_catalog_owns_the_default_slot() {
        let file = CatalogFile::parse("catalog: {}\n").expect("parse");
        assert!(file.catalog.as_ref().is_some_and(BTreeMap::is_empty));
        assert!(file.catalogs.is_none());
        assert!(file.has_catalog_table());
    }

    #[test]
    fn empty_object_catalogs_owns_the_named_slot() {
        let file = CatalogFile::parse("catalogs: {}\n").expect("parse");
        assert!(file.catalog.is_none());
        assert!(file.catalogs.as_ref().is_some_and(BTreeMap::is_empty));
        assert!(file.has_catalog_table());
    }

    #[test]
    fn implicit_nested_catalogs_default_is_a_deserialize_miss() {
        assert!(CatalogFile::parse("catalogs: default:\n  core: '^1.0.0'\n").is_err());
    }

    #[test]
    fn sequence_catalog_is_a_deserialize_miss() {
        let text = "catalog: []\ncatalogs:\n  default:\n    core: '^1.0.0'\n";
        assert!(CatalogFile::parse(text).is_err());
    }

    #[test]
    fn both_default_tables_are_present() {
        let file = CatalogFile::parse(
            "catalog:\n  other: '^1.0.0'\ncatalogs:\n  default:\n    core: '^1.0.0'\n",
        )
        .expect("parse");
        assert!(file.catalog.is_some());
        assert!(file.has_default_table());
    }

    #[test]
    fn null_pin_keeps_the_catalog_table() {
        let file = CatalogFile::parse(
            "catalog:\n  core: null\ncatalogs:\n  default:\n    core: '^9.0.0'\n",
        )
        .expect("parse");
        assert!(file.catalog.is_some());
        assert!(file.string_at(&["catalog", "core"]).is_none());
        assert_eq!(
            file.string_at(&["catalogs", "default", "core"]),
            Some("^9.0.0")
        );
    }

    #[test]
    fn leftover_tokens_match_serde_saphyr_string_map() {
        let accepted = [
            ("1", "1"),
            ("true", "true"),
            ("yes", "yes"),
            ("^0.1.0", "^0.1.0"),
            ("0B10", "0B10"),
            ("2024-01-01", "2024-01-01"),
        ];
        for (value, expect) in accepted {
            let file = CatalogFile::parse(&format!("catalog:\n  core: {value}\n")).expect(value);
            assert_eq!(
                file.string_at(&["catalog", "core"]),
                Some(expect),
                "{value}"
            );
        }
        for value in ["null", "Null", "~"] {
            let file = CatalogFile::parse(&format!("catalog:\n  core: {value}\n")).expect(value);
            assert!(file.catalog.is_some(), "{value}");
            assert!(file.string_at(&["catalog", "core"]).is_none(), "{value}");
        }
        for value in ["!!int 1", "*p"] {
            assert!(
                CatalogFile::parse(&format!("catalog:\n  core: {value}\n")).is_err(),
                "{value}"
            );
        }
    }

    #[test]
    fn leftover_tokens_match_named_catalog_string_map() {
        let file = CatalogFile::parse("catalogs:\n  default:\n    core: 1\n").expect("parse");
        assert_eq!(file.string_at(&["catalogs", "default", "core"]), Some("1"));
        for value in ["null", "Null", "~"] {
            let file = CatalogFile::parse(&format!("catalogs:\n  default:\n    core: {value}\n"))
                .expect(value);
            assert!(file.has_default_table(), "{value}");
            assert!(
                file.string_at(&["catalogs", "default", "core"]).is_none(),
                "{value}"
            );
        }
        for value in ["!!int 1", "*p"] {
            assert!(
                CatalogFile::parse(&format!("catalogs:\n  default:\n    core: {value}\n")).is_err(),
                "{value}"
            );
        }
    }

    #[test]
    fn string_pins_drops_null_and_keeps_neighbors() {
        let mut pins = BTreeMap::new();
        pins.insert("core".into(), None);
        pins.insert("other".into(), Some("^1.0.0".into()));
        let kept = CatalogFile::string_pins(pins);
        assert_eq!(kept.get("other").map(String::as_str), Some("^1.0.0"));
        assert!(!kept.contains_key("core"));
    }

    #[test]
    fn string_tables_drops_null_pins() {
        let mut pins = BTreeMap::new();
        pins.insert("core".into(), None);
        pins.insert("other".into(), Some("^1.0.0".into()));
        let mut tables = BTreeMap::new();
        tables.insert("pinned".into(), Some(pins));
        let named = CatalogFile::string_tables(tables);
        let pinned = named.get("pinned").expect("pinned");
        assert_eq!(pinned.get("other").map(String::as_str), Some("^1.0.0"));
        assert!(!pinned.contains_key("core"));
    }

    #[test]
    fn string_tables_keeps_an_all_null_named_table() {
        let mut pins = BTreeMap::new();
        pins.insert("core".into(), None);
        let mut tables = BTreeMap::new();
        tables.insert("pinned".into(), Some(pins));
        let named = CatalogFile::string_tables(tables);
        assert!(named.get("pinned").is_some_and(BTreeMap::is_empty));
    }

    #[test]
    fn parsed_null_pin_is_dropped_from_string_pins() {
        let file =
            CatalogFile::parse("catalog:\n  core: null\n  other: '^1.0.0'\n").expect("parse");
        let kept = CatalogFile::string_pins(file.catalog.expect("catalog"));
        assert_eq!(kept.get("other").map(String::as_str), Some("^1.0.0"));
        assert!(!kept.contains_key("core"));
    }

    #[test]
    fn null_named_catalog_table_is_flagged() {
        let file = CatalogFile::parse("catalogs:\n  default: null\n").expect("parse");
        assert!(file.has_null_named_table());
        assert!(!file.has_default_table());
    }

    #[test]
    fn null_non_default_named_catalog_table_is_flagged() {
        let file = CatalogFile::parse("catalog:\n  lodash: '^4.0.0'\ncatalogs:\n  pinned: null\n")
            .expect("parse");
        assert!(file.has_null_named_table());
        assert!(!file
            .catalogs
            .as_ref()
            .and_then(|tables| tables.get("pinned"))
            .is_some_and(Option::is_some));
    }

    #[test]
    fn null_pin_keeps_a_named_catalog_table() {
        let file = CatalogFile::parse("catalogs:\n  pinned:\n    core: null\n").expect("parse");
        assert!(file
            .catalogs
            .as_ref()
            .is_some_and(|tables| tables.contains_key("pinned")));
        assert!(file.string_at(&["catalogs", "pinned", "core"]).is_none());
    }

    #[test]
    fn catalog_target_named_skips_xor() {
        match catalog_target(Some("default"), "core", true, true) {
            CatalogTarget::At { path, missing_path } => {
                assert_eq!(path, ["catalogs", "default", "core"]);
                assert_eq!(missing_path, "catalogs/default/core");
            }
            CatalogTarget::Duplicate => panic!("named is not xor"),
        }
    }

    #[test]
    fn catalog_target_both_default_tables_is_duplicate() {
        assert!(matches!(
            catalog_target(None, "core", true, true),
            CatalogTarget::Duplicate
        ));
    }

    #[test]
    fn catalog_target_default_flag_grid() {
        match catalog_target(None, "core", true, false) {
            CatalogTarget::At { path, missing_path } => {
                assert_eq!(path, ["catalog", "core"]);
                assert_eq!(missing_path, "catalog/core");
            }
            CatalogTarget::Duplicate => panic!("catalog-only is not xor"),
        }
        match catalog_target(None, "core", false, true) {
            CatalogTarget::At { path, missing_path } => {
                assert_eq!(path, ["catalogs", "default", "core"]);
                assert_eq!(missing_path, "catalog/core");
            }
            CatalogTarget::Duplicate => panic!("default-only is not xor"),
        }
        match catalog_target(None, "core", false, false) {
            CatalogTarget::At { path, missing_path } => {
                assert_eq!(path, ["catalog", "core"]);
                assert_eq!(missing_path, "catalog/core");
            }
            CatalogTarget::Duplicate => panic!("neither table is not xor"),
        }
    }
}
