//! Catalog pins live in `pnpm-workspace.yaml`, `.yarnrc.yml`, or
//! `package.json`, not on the member `catalog:` line
//! (`DeclaredRange::retargeted_text` is `None` there).

use std::collections::BTreeMap;

use super::json::{json_string, json_table_present, replace_json_string, JsonEditError};
use super::yaml_scalar::{apply_edits, locate_yaml_scalar, Span};
use crate::discover::{catalog_target, CatalogFile, CatalogTarget};

/// Rewrite a string catalog entry in `package.json` (`catalog` or `catalogs`).
///
/// `None` is the default catalog: `catalog.<package>`, else
/// `catalogs.default.<package>`. Both default tables in one file is an error
/// (pnpm XOR). `Some("default")` is only the named `catalogs.default` table.
///
/// # Errors
///
/// Returns [`JsonEditError`] when the path is missing, duplicated, or not a
/// string.
pub fn rewrite_catalog_json(
    text: &str,
    catalog: Option<&str>,
    package: &str,
    new_range: &str,
) -> Result<String, JsonEditError> {
    let (has_catalog, has_default) = json_default_tables(text, catalog)?;
    match catalog_target(catalog, package, has_catalog, has_default) {
        CatalogTarget::Duplicate => Err(JsonEditError::Duplicate {
            path: "catalog".into(),
        }),
        CatalogTarget::At { path, missing_path } => {
            if catalog.is_none() && !has_catalog && !has_default {
                return Err(JsonEditError::Missing { path: missing_path });
            }
            match rewrite_json_at(text, &path, new_range) {
                Ok(out) => Ok(out),
                Err(JsonEditError::Missing { .. }) => {
                    Err(JsonEditError::Missing { path: missing_path })
                }
                Err(err) => Err(err),
            }
        }
    }
}

fn json_default_tables(text: &str, catalog: Option<&str>) -> Result<(bool, bool), JsonEditError> {
    if catalog.is_some() {
        return Ok((false, false));
    }
    let has_catalog = json_table_present(text, &["catalog"])?;
    let has_default = match json_table_present(text, &["catalogs", "default"]) {
        Ok(present) => present,
        Err(JsonEditError::NotObject { .. }) if has_catalog => false,
        Err(err) => return Err(err),
    };
    Ok((has_catalog, has_default))
}

fn rewrite_json_at(text: &str, path: &[&str], new_range: &str) -> Result<String, JsonEditError> {
    let current = json_string(text, path)?;
    if already_at(&current, new_range) {
        return Ok(text.to_owned());
    }
    replace_json_string(text, path, &retarget_catalog_value(&current, new_range))
}

/// A blank pin retargets to blank (any version); discovery trims, so `' '`
/// arrives as `""`.
fn already_at(current: &str, new_range: &str) -> bool {
    current == new_range || (current.trim().is_empty() && new_range.trim().is_empty())
}

/// Rewrite a string catalog entry in `pnpm-workspace.yaml` or `.yarnrc.yml`.
///
/// `None` is the default catalog: `catalog.<package>`, else
/// `catalogs.default.<package>`. Both default tables in one file is an error
/// (pnpm XOR). `Some("default")` is only the named `catalogs.default` table.
///
/// Comments, indent, and newlines stay. A single-line string pin is
/// rewritten in place. Multiline block or quoted scalars, mappings,
/// sequences, merge keys, undefined aliases, and non-string scalars
/// are refused.
///
/// An anchor can be shared with another package's pin, so an alias pin is
/// written out as a literal, and an anchored pin's aliases are written out
/// with its old value. Returns the text and a note for each thing the
/// rewrite did past the pin.
///
/// # Errors
///
/// Returns [`CatalogYamlError`] when the file is not a catalog schema, or
/// the entry is missing, duplicated, or not a rewriteable string.
/// [`CatalogFile::parse`] failures become [`CatalogYamlError::Invalid`],
/// not the walker's `Duplicate` / `NotString`.
pub fn rewrite_catalog_yaml(
    text: &str,
    catalog: Option<&str>,
    package: &str,
    new_range: &str,
) -> Result<(String, Vec<AnchorNote>), CatalogYamlError> {
    let file = CatalogFile::parse(text).map_err(|_| CatalogYamlError::Invalid)?;
    if file.has_null_named_table() {
        return Err(CatalogYamlError::Invalid);
    }
    let bom = text.starts_with('\u{feff}');
    let body = text.strip_prefix('\u{feff}').unwrap_or(text);
    let (path, found) = match catalog_target(
        catalog,
        package,
        file.catalog.is_some(),
        file.has_default_table(),
    ) {
        CatalogTarget::Duplicate => return Err(CatalogYamlError::duplicate(&["catalog"])),
        CatalogTarget::At { path, missing_path } => {
            if let Some(current) = file.string_at(&path) {
                if already_at(current, new_range) {
                    return Ok((text.to_owned(), Vec::new()));
                }
                // Its span can cover a block indicator and a trailing comment.
                if current.trim().is_empty() {
                    return Err(CatalogYamlError::not_string(&path));
                }
            }
            match locate_yaml_scalar(body, &path, file.string_at(&path).is_some()) {
                Ok(found) => (path, found),
                Err(CatalogYamlError::Missing { .. }) => {
                    return Err(CatalogYamlError::Missing { path: missing_path })
                }
                Err(err) => return Err(err),
            }
        }
    };
    let decoded = without_final_newline(&found.decoded);
    let wanted = retarget_catalog_value(decoded, new_range);
    let mut edits = Vec::new();
    let mut notes = Vec::new();
    if let Some(alias) = found.alias {
        notes.push(AnchorNote::AliasWrittenOut {
            path: path.join("/"),
            anchor: anchor_name(body, &alias),
        });
        edits.push((alias, single_quoted(&wanted)));
    } else {
        let old = &body[found.span.start..found.span.end];
        edits.push((found.span, retarget_catalog_value(old, new_range)));
        if let Some(first) = found.uses.first() {
            notes.push(AnchorNote::AliasesWrittenOut {
                path: path.join("/"),
                anchor: anchor_name(body, first),
                uses: found.uses.len(),
            });
        }
        for alias in found.uses {
            edits.push((alias, single_quoted(decoded)));
        }
    }
    let mut out = apply_edits(body, edits);
    let after = CatalogFile::parse(&out).map_err(|_| CatalogYamlError::not_string(&path))?;
    match locate_yaml_scalar(&out, &path, true) {
        Ok(at) if without_final_newline(&at.decoded) == wanted => {}
        _ => return Err(CatalogYamlError::not_string(&path)),
    }
    // An anchor on a table holding the pin moves the pin's copies elsewhere.
    let moved = moved_entries(&file, &after, &path);
    if !moved.is_empty() {
        notes.push(AnchorNote::CopiesMoved {
            path: path.join("/"),
            moved,
        });
    }
    if bom {
        out.insert(0, '\u{feff}');
    }
    Ok((out, notes))
}

/// A block scalar decodes with its final newline; a written-out copy has none.
fn without_final_newline(decoded: &str) -> &str {
    decoded.strip_suffix('\n').unwrap_or(decoded)
}

/// Every entry other than `path` whose string differs between the two files.
fn moved_entries(before: &CatalogFile, after: &CatalogFile, path: &[&str]) -> Vec<String> {
    let entries = |file: &CatalogFile| {
        let pin = |pin: &Option<String>| {
            pin.as_deref()
                .map(|pin| without_final_newline(pin).to_owned())
        };
        let mut out = BTreeMap::new();
        for (package, value) in file.catalog.iter().flatten() {
            out.insert(format!("catalog/{package}"), pin(value));
        }
        for (name, table) in file.catalogs.iter().flatten() {
            for (package, value) in table.iter().flatten() {
                out.insert(format!("catalogs/{name}/{package}"), pin(value));
            }
        }
        out
    };
    let (before, after) = (entries(before), entries(after));
    let target = path.join("/");
    before
        .keys()
        .chain(after.keys())
        .filter(|key| **key != target && before.get(*key) != after.get(*key))
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// What a rewrite did beyond the pin, since a YAML anchor can be shared.
/// No value is quoted: a later pin in the same file can move it again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnchorNote {
    AliasWrittenOut {
        path: String,
        anchor: String,
    },
    AliasesWrittenOut {
        path: String,
        anchor: String,
        uses: usize,
    },
    /// Entries reaching the pin through an alias of a table holding it.
    CopiesMoved {
        path: String,
        moved: Vec<String>,
    },
}

impl core::fmt::Display for AnchorNote {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::AliasWrittenOut { path, anchor } => write!(
                f,
                "`{path}` was the alias `*{anchor}`; it is written out, so `&{anchor}` does not move with it"
            ),
            Self::AliasesWrittenOut { path, anchor, uses } => {
                let aliases = if *uses == 1 { "alias is" } else { "aliases are" };
                write!(
                    f,
                    "`{path}` carries `&{anchor}`; its {uses} {aliases} written out rather than moved with it"
                )
            }
            Self::CopiesMoved { path, moved } => write!(
                f,
                "`{path}` also moves `{}`, which reach it through an alias",
                moved.join("`, `")
            ),
        }
    }
}

/// Quoted, so a tag, a flow context, or an indicator character cannot change
/// what the written-out value reads as.
fn single_quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn anchor_name(text: &str, alias: &Span) -> String {
    let token = &text[alias.start..alias.end];
    token.strip_prefix('*').unwrap_or(token).to_owned()
}

/// Missing or `null` `catalog` / `catalogs` is `false`. A present table that
/// is not a map is [`CatalogYamlError::Invalid`], matching
/// [`rewrite_catalog_yaml`].
///
/// # Errors
///
/// Returns [`CatalogYamlError::Invalid`] when the text is not a catalog schema.
pub fn yaml_has_catalog_table(text: &str) -> Result<bool, CatalogYamlError> {
    let file = CatalogFile::parse(text).map_err(|_| CatalogYamlError::Invalid)?;
    Ok(file.has_catalog_table())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogYamlError {
    Missing { path: String },
    NotString { path: String },
    Duplicate { path: String },
    Invalid,
}

impl CatalogYamlError {
    pub(super) fn missing(path: &[&str]) -> Self {
        Self::Missing {
            path: path.join("/"),
        }
    }

    pub(super) fn not_string(path: &[&str]) -> Self {
        Self::NotString {
            path: path.join("/"),
        }
    }

    pub(super) fn duplicate(path: &[&str]) -> Self {
        Self::Duplicate {
            path: path.join("/"),
        }
    }
}

impl core::fmt::Display for CatalogYamlError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Missing { path } => write!(f, "catalog path `{path}` does not exist"),
            Self::NotString { path } => {
                write!(f, "catalog path `{path}` is not a rewriteable string")
            }
            Self::Duplicate { path } => write!(f, "catalog path `{path}` is duplicated"),
            Self::Invalid => f.write_str("catalog file is not a valid catalog schema"),
        }
    }
}

impl std::error::Error for CatalogYamlError {}

/// Keep `npm:<name>@` so a catalog alias is not retargeted to a different package.
fn retarget_catalog_value(current: &str, new_range: &str) -> String {
    let Some(rest) = current.strip_prefix("npm:") else {
        return new_range.to_owned();
    };
    match split_name_version(rest) {
        Some((name, version)) if !name.is_empty() && !version.is_empty() => {
            format!("npm:{name}@{new_range}")
        }
        _ => new_range.to_owned(),
    }
}

fn split_name_version(spec: &str) -> Option<(&str, &str)> {
    if let Some(without_at) = spec.strip_prefix('@') {
        if let Some(at) = without_at.find('@') {
            let name_end = at + 1;
            Some((&spec[..name_end], &without_at[at + 1..]))
        } else if without_at.is_empty() {
            None
        } else {
            Some((spec, ""))
        }
    } else if let Some(at) = spec.find('@') {
        Some((&spec[..at], &spec[at + 1..]))
    } else if spec.is_empty() {
        None
    } else {
        Some((spec, ""))
    }
}

#[cfg(test)]
mod tests {
    use super::{rewrite_catalog_json, yaml_has_catalog_table, CatalogYamlError};

    /// The rewritten text alone, for the tests that check no note.
    fn rewrite_catalog_yaml(
        text: &str,
        catalog: Option<&str>,
        package: &str,
        new_range: &str,
    ) -> Result<String, CatalogYamlError> {
        super::rewrite_catalog_yaml(text, catalog, package, new_range).map(|(out, _)| out)
    }

    fn yaml_missing(path: &str) -> CatalogYamlError {
        CatalogYamlError::Missing {
            path: path.to_owned(),
        }
    }

    fn yaml_not_string(path: &str) -> CatalogYamlError {
        CatalogYamlError::NotString {
            path: path.to_owned(),
        }
    }

    fn yaml_duplicate(path: &str) -> CatalogYamlError {
        CatalogYamlError::Duplicate {
            path: path.to_owned(),
        }
    }

    #[test]
    fn yaml_has_catalog_table_matches_catalog_file() {
        assert!(!yaml_has_catalog_table("packages:\n  - 'packages/*'\n").expect("packages"));
        assert!(!yaml_has_catalog_table("catalog: null\n").expect("null"));
        assert!(!yaml_has_catalog_table("catalogs: null\n").expect("null catalogs"));
        assert!(yaml_has_catalog_table("catalog: {}\n").expect("empty catalog"));
        assert!(yaml_has_catalog_table("catalogs: {}\n").expect("empty catalogs"));
        assert!(
            yaml_has_catalog_table("catalogs:\n  pinned:\n    core: '1.0.0'\n").expect("named")
        );
        assert_eq!(
            yaml_has_catalog_table("catalog: []\n").expect_err("sequence"),
            CatalogYamlError::Invalid
        );
        assert_eq!(
            yaml_has_catalog_table("catalog: [\n").expect_err("unclosed"),
            CatalogYamlError::Invalid
        );
    }

    #[test]
    fn json_default_catalog_is_rewritten() {
        let src = "{\n  \"catalog\": {\n    \"core\": \"^0.1.0\"\n  }\n}\n";
        let out = rewrite_catalog_json(src, None, "core", "^0.2.0").expect("json");
        assert_eq!(
            out,
            "{\n  \"catalog\": {\n    \"core\": \"^0.2.0\"\n  }\n}\n"
        );
    }

    #[test]
    fn json_named_catalog_is_rewritten() {
        let src =
            "{\n  \"catalogs\": {\n    \"pinned\": {\n      \"core\": \"1.0.0\"\n    }\n  }\n}\n";
        let out = rewrite_catalog_json(src, Some("pinned"), "core", "2.0.0").expect("json");
        assert_eq!(
            out,
            "{\n  \"catalogs\": {\n    \"pinned\": {\n      \"core\": \"2.0.0\"\n    }\n  }\n}\n"
        );
    }

    #[test]
    fn json_default_falls_back_to_catalogs_default() {
        let src =
            "{\n  \"catalogs\": {\n    \"default\": {\n      \"core\": \"^0.1.0\"\n    }\n  }\n}\n";
        let out = rewrite_catalog_json(src, None, "core", "^0.2.0").expect("json");
        assert_eq!(
            out,
            "{\n  \"catalogs\": {\n    \"default\": {\n      \"core\": \"^0.2.0\"\n    }\n  }\n}\n"
        );
    }

    #[test]
    fn json_both_default_tables_is_duplicate() {
        let src = "{\n  \"catalog\": {\n    \"core\": \"^0.1.0\"\n  },\n  \"catalogs\": {\n    \"default\": {\n      \"core\": \"^9.0.0\"\n    }\n  }\n}\n";
        let err = rewrite_catalog_json(src, None, "core", "^0.2.0").expect_err("xor");
        assert!(matches!(
            err,
            crate::manifest::json::JsonEditError::Duplicate { path } if path == "catalog"
        ));
    }

    #[test]
    fn json_both_default_tables_is_duplicate_when_package_is_only_in_one() {
        let src = "{\n  \"catalog\": {\n    \"other\": \"^0.1.0\"\n  },\n  \"catalogs\": {\n    \"default\": {\n      \"core\": \"^9.0.0\"\n    }\n  }\n}\n";
        let err = rewrite_catalog_json(src, None, "core", "^0.2.0").expect_err("xor");
        assert!(matches!(
            err,
            crate::manifest::json::JsonEditError::Duplicate { path } if path == "catalog"
        ));
    }

    #[test]
    fn json_empty_catalog_object_xor_catalogs_default() {
        let src = "{\n  \"catalog\": {},\n  \"catalogs\": {\n    \"default\": {\n      \"core\": \"^9.0.0\"\n    }\n  }\n}\n";
        let err = rewrite_catalog_json(src, None, "core", "^0.2.0").expect_err("xor");
        assert!(matches!(
            err,
            crate::manifest::json::JsonEditError::Duplicate { path } if path == "catalog"
        ));
    }

    #[test]
    fn json_missing_on_both_tables_names_the_primary_path() {
        let src =
            "{\n  \"catalogs\": {\n    \"default\": {\n      \"other\": \"^9.0.0\"\n    }\n  }\n}\n";
        let err = rewrite_catalog_json(src, None, "core", "^0.2.0").expect_err("missing");
        assert!(matches!(
            err,
            crate::manifest::json::JsonEditError::Missing { path } if path == "catalog/core"
        ));
    }

    #[test]
    fn json_named_miss_names_the_named_path() {
        let src = "{\n  \"catalog\": {\n    \"core\": \"^0.1.0\"\n  },\n  \"catalogs\": {\n    \"pinned\": {\n      \"other\": \"1.0.0\"\n    }\n  }\n}\n";
        let err = rewrite_catalog_json(src, Some("pinned"), "core", "2.0.0").expect_err("missing");
        assert!(matches!(
            err,
            crate::manifest::json::JsonEditError::Missing { path } if path == "catalogs/pinned/core"
        ));
    }

    #[test]
    fn json_null_catalog_falls_back_to_catalogs_default() {
        let src = "{\n  \"catalog\": null,\n  \"catalogs\": {\n    \"default\": {\n      \"core\": \"^9.0.0\"\n    }\n  }\n}\n";
        let out = rewrite_catalog_json(src, None, "core", "^0.2.0").expect("json");
        assert_eq!(
            out,
            "{\n  \"catalog\": null,\n  \"catalogs\": {\n    \"default\": {\n      \"core\": \"^0.2.0\"\n    }\n  }\n}\n"
        );
    }

    #[test]
    fn json_sequence_catalog_is_not_a_table() {
        let src = "{\n  \"catalog\": [],\n  \"catalogs\": {\n    \"default\": {\n      \"core\": \"^9.0.0\"\n    }\n  }\n}\n";
        let err = rewrite_catalog_json(src, None, "core", "^0.2.0").expect_err("seq");
        assert!(matches!(
            err,
            crate::manifest::json::JsonEditError::NotObject { path } if path == "catalog"
        ));
    }

    #[test]
    fn json_number_catalog_is_not_a_table() {
        let src = "{\n  \"catalog\": 1,\n  \"catalogs\": {\n    \"default\": {\n      \"core\": \"^9.0.0\"\n    }\n  }\n}\n";
        let err = rewrite_catalog_json(src, None, "core", "^0.2.0").expect_err("num");
        assert!(matches!(
            err,
            crate::manifest::json::JsonEditError::NotObject { path } if path == "catalog"
        ));
    }

    #[test]
    fn json_named_rewrite_ignores_sequence_catalog() {
        let src = "{\n  \"catalog\": [],\n  \"catalogs\": {\n    \"pinned\": {\n      \"core\": \"1.0.0\"\n    }\n  }\n}\n";
        let out = rewrite_catalog_json(src, Some("pinned"), "core", "2.0.0").expect("json");
        assert_eq!(
            out,
            "{\n  \"catalog\": [],\n  \"catalogs\": {\n    \"pinned\": {\n      \"core\": \"2.0.0\"\n    }\n  }\n}\n"
        );
    }

    #[test]
    fn json_sequence_default_does_not_block_catalog() {
        let src = "{\n  \"catalog\": {\n    \"core\": \"^0.1.0\"\n  },\n  \"catalogs\": {\n    \"default\": []\n  }\n}\n";
        let out = rewrite_catalog_json(src, None, "core", "^0.2.0").expect("json");
        assert_eq!(
            out,
            "{\n  \"catalog\": {\n    \"core\": \"^0.2.0\"\n  },\n  \"catalogs\": {\n    \"default\": []\n  }\n}\n"
        );
    }

    #[test]
    fn json_null_default_without_catalog_is_missing() {
        let src = "{\n  \"catalogs\": {\n    \"default\": null\n  }\n}\n";
        let err = rewrite_catalog_json(src, None, "core", "^0.2.0").expect_err("null default");
        assert!(matches!(
            err,
            crate::manifest::json::JsonEditError::Missing { path } if path == "catalog/core"
        ));
    }

    #[test]
    fn json_null_catalog_without_default_is_missing() {
        let src = "{\n  \"catalog\": null\n}\n";
        let err = rewrite_catalog_json(src, None, "core", "^0.2.0").expect_err("null catalog");
        assert!(matches!(
            err,
            crate::manifest::json::JsonEditError::Missing { path } if path == "catalog/core"
        ));
    }

    #[test]
    fn json_both_null_default_tables_are_missing() {
        let src = "{\n  \"catalog\": null,\n  \"catalogs\": {\n    \"default\": null\n  }\n}\n";
        let err = rewrite_catalog_json(src, None, "core", "^0.2.0").expect_err("both null");
        assert!(matches!(
            err,
            crate::manifest::json::JsonEditError::Missing { path } if path == "catalog/core"
        ));
    }

    #[test]
    fn json_sequence_default_without_catalog_is_not_a_table() {
        let src = "{\n  \"catalogs\": {\n    \"default\": []\n  }\n}\n";
        let err = rewrite_catalog_json(src, None, "core", "^0.2.0").expect_err("seq default");
        assert!(matches!(
            err,
            crate::manifest::json::JsonEditError::NotObject { path } if path == "catalogs/default"
        ));
    }

    #[test]
    fn json_non_object_catalogs_does_not_block_catalog() {
        let src = "{\n  \"catalog\": {\n    \"core\": \"^0.1.0\"\n  },\n  \"catalogs\": 1\n}\n";
        let out = rewrite_catalog_json(src, None, "core", "^0.2.0").expect("json");
        assert_eq!(
            out,
            "{\n  \"catalog\": {\n    \"core\": \"^0.2.0\"\n  },\n  \"catalogs\": 1\n}\n"
        );
    }

    #[test]
    fn json_named_default_rewrites_only_catalogs_default() {
        let src = "{\n  \"catalog\": {\n    \"core\": \"^0.1.0\"\n  },\n  \"catalogs\": {\n    \"default\": {\n      \"core\": \"^9.0.0\"\n    }\n  }\n}\n";
        let out = rewrite_catalog_json(src, Some("default"), "core", "^0.2.0").expect("json");
        assert_eq!(
            out,
            "{\n  \"catalog\": {\n    \"core\": \"^0.1.0\"\n  },\n  \"catalogs\": {\n    \"default\": {\n      \"core\": \"^0.2.0\"\n    }\n  }\n}\n"
        );
    }

    #[test]
    fn json_blank_pin_retargeted_to_blank_is_unchanged() {
        for src in [
            "{\n  \"catalog\": {\n    \"core\": \"\"\n  }\n}\n",
            "{\n  \"catalog\": {\n    \"core\": \" \"\n  }\n}\n",
        ] {
            let out = rewrite_catalog_json(src, None, "core", "").expect(src);
            assert_eq!(out, src);
        }
    }

    #[test]
    fn json_non_string_catalog_does_not_fall_back() {
        let src = "{\n  \"catalog\": {\n    \"core\": 1\n  }\n}\n";
        let err = rewrite_catalog_json(src, None, "core", "^0.2.0").expect_err("kind");
        assert!(matches!(
            err,
            crate::manifest::json::JsonEditError::NotString { path } if path == "catalog/core"
        ));
    }

    #[test]
    fn yaml_both_default_tables_is_duplicate() {
        let src = "catalog:\n  core: '^0.1.0'\ncatalogs:\n  default:\n    core: '^9.0.0'\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("xor");
        assert_eq!(err, yaml_duplicate("catalog"));
    }

    #[test]
    fn yaml_both_default_tables_is_duplicate_when_package_is_only_in_one() {
        let src = "catalog:\n  other: '^0.1.0'\ncatalogs:\n  default:\n    core: '^9.0.0'\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("xor");
        assert_eq!(err, yaml_duplicate("catalog"));
    }

    #[test]
    fn yaml_null_catalog_falls_back_to_catalogs_default() {
        let src = "catalog:\ncatalogs:\n  default:\n    core: '^9.0.0'\n";
        let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect("null catalog");
        assert_eq!(out, "catalog:\ncatalogs:\n  default:\n    core: '^0.2.0'\n");
    }

    #[test]
    fn yaml_empty_flow_catalog_xor_catalogs_default() {
        let src = "catalog: {}\ncatalogs:\n  default:\n    core: '^9.0.0'\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("xor");
        assert_eq!(err, yaml_duplicate("catalog"));
    }

    #[test]
    fn yaml_flow_catalogs_default_xor_block_catalog() {
        let src = "catalog:\n  core: '^0.1.0'\ncatalogs: { default: { core: '^9.0.0' } }\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("xor");
        assert_eq!(err, yaml_duplicate("catalog"));
    }

    #[test]
    fn yaml_next_line_flow_catalogs_default_xor_block_catalog() {
        let src = "catalog:\n  core: '^0.1.0'\ncatalogs:\n  { default: { core: '^9.0.0' } }\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("xor");
        assert_eq!(err, yaml_duplicate("catalog"));
    }

    #[test]
    fn yaml_flow_named_catalogs_does_not_xor() {
        let src = "catalog:\n  core: '^0.1.0'\ncatalogs: { pinned: { core: '^9.0.0' } }\n";
        let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect("yaml");
        assert_eq!(
            out,
            "catalog:\n  core: '^0.2.0'\ncatalogs: { pinned: { core: '^9.0.0' } }\n"
        );
    }

    #[test]
    fn yaml_sequence_catalog_is_not_a_schema() {
        let src = "catalog: []\ncatalogs:\n  default:\n    core: '^9.0.0'\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("seq");
        assert_eq!(err, CatalogYamlError::Invalid);
    }

    #[test]
    fn yaml_next_line_sequence_catalog_is_not_a_schema() {
        let src = "catalog:\n  [core: '^0.1.0']\ncatalogs:\n  default:\n    core: '^9.0.0'\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("seq");
        assert_eq!(err, CatalogYamlError::Invalid);
    }

    #[test]
    fn yaml_block_sequence_catalog_is_not_a_schema() {
        let src = "catalog:\n  - core: '^0.1.0'\ncatalogs:\n  default:\n    core: '^9.0.0'\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("seq");
        assert_eq!(err, CatalogYamlError::Invalid);
    }

    #[test]
    fn yaml_next_line_flow_named_catalogs_does_not_xor() {
        let src = "catalog:\n  core: '^0.1.0'\ncatalogs:\n  { pinned: { core: '^9.0.0' } }\n";
        let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect("yaml");
        assert_eq!(
            out,
            "catalog:\n  core: '^0.2.0'\ncatalogs:\n  { pinned: { core: '^9.0.0' } }\n"
        );
    }

    #[test]
    fn yaml_multiline_flow_catalogs_default_xor_block_catalog() {
        let src = "catalog:\n  core: '^0.1.0'\ncatalogs: {\n  default: { core: '^9.0.0' }\n}\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("xor");
        assert_eq!(err, yaml_duplicate("catalog"));
    }

    #[test]
    fn yaml_tagged_flow_catalogs_default_xor_block_catalog() {
        let src = "catalog:\n  core: '^0.1.0'\ncatalogs: !!map { default: { core: '^9.0.0' } }\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("xor");
        assert_eq!(err, yaml_duplicate("catalog"));
    }

    #[test]
    fn yaml_implicit_nested_leaf_is_not_a_default_table() {
        let src = "catalog: core: '^0.1.0'\ncatalogs:\n  default:\n    core: '^9.0.0'\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("nested");
        assert_eq!(err, CatalogYamlError::Invalid);
    }

    #[test]
    fn yaml_implicit_parent_catalogs_default_is_not_a_schema() {
        let src = "catalogs: default:\n  core: '^0.1.0'\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("implicit");
        assert_eq!(err, CatalogYamlError::Invalid);
    }

    #[test]
    fn yaml_implicit_catalogs_default_is_not_a_schema() {
        let src = "catalog:\n  other: '^0.1.0'\ncatalogs: default:\n  core: '^9.0.0'\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("implicit");
        assert_eq!(err, CatalogYamlError::Invalid);
    }

    #[test]
    fn yaml_named_default_rewrites_only_catalogs_default() {
        let src = "catalog:\n  core: '^0.1.0'\ncatalogs:\n  default:\n    core: '^9.0.0'\n";
        let out = rewrite_catalog_yaml(src, Some("default"), "core", "^0.2.0").expect("yaml");
        assert_eq!(
            out,
            "catalog:\n  core: '^0.1.0'\ncatalogs:\n  default:\n    core: '^0.2.0'\n"
        );
    }

    #[test]
    fn yaml_non_string_catalog_does_not_fall_back() {
        let src = "catalog:\n  core: null\ncatalogs:\n  pinned:\n    core: '^9.0.0'\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("kind");
        assert_eq!(err, yaml_not_string("catalog/core"));
    }

    #[test]
    fn yaml_unquoted_number_pin_is_rewritten() {
        let src = "catalog:\n  core: 1\n";
        let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect("number");
        assert_eq!(out, "catalog:\n  core: ^0.2.0\n");
    }

    #[test]
    fn yaml_implicit_nested_map_is_not_a_string() {
        let src = "catalog: core: '^0.1.0'\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("nested");
        assert_eq!(err, CatalogYamlError::Invalid);
    }

    #[test]
    fn yaml_implicit_nested_leaf_is_not_a_string() {
        let src = "catalog:\n  core: nested: '^0.1.0'\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("nested");
        assert_eq!(err, CatalogYamlError::Invalid);
    }

    #[test]
    fn yaml_default_catalog_keeps_comment_and_neighbor() {
        let src = "packages:\n  - '*'\ncatalog:\n  unused: '^9.0.0'\n  core: '^0.1.0'   # pin\n";
        let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect("yaml");
        assert_eq!(
            out,
            "packages:\n  - '*'\ncatalog:\n  unused: '^9.0.0'\n  core: '^0.2.0'   # pin\n"
        );
    }

    #[test]
    fn yaml_quoted_key_is_rewritten() {
        let src = "catalog:\n  '@oakum/core': '^0.1.0'\n";
        let out = rewrite_catalog_yaml(src, None, "@oakum/core", "^0.2.0").expect("yaml");
        assert_eq!(out, "catalog:\n  '@oakum/core': '^0.2.0'\n");
    }

    #[test]
    fn yaml_named_catalog_does_not_touch_default() {
        let src = "catalog:\n  core: '^0.1.0'\ncatalogs:\n  pinned:\n    core: '1.0.0'\n";
        let out = rewrite_catalog_yaml(src, Some("pinned"), "core", "2.0.0").expect("yaml");
        assert_eq!(
            out,
            "catalog:\n  core: '^0.1.0'\ncatalogs:\n  pinned:\n    core: '2.0.0'\n"
        );
    }

    #[test]
    fn yaml_missing_entry_is_an_error() {
        let src = "catalog:\n  other: '^0.1.0'\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("missing");
        assert_eq!(err, yaml_missing("catalog/core"));
    }

    #[test]
    fn yaml_missing_on_both_tables_names_the_primary_path() {
        let src = "catalogs:\n  default:\n    other: '^9.0.0'\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("missing");
        assert_eq!(err, yaml_missing("catalog/core"));
        assert!(!err.to_string().contains("catalogs/default/core"), "{err}");
    }

    #[test]
    fn yaml_named_miss_is_an_error() {
        let src = "catalog:\n  core: '^0.1.0'\ncatalogs:\n  pinned:\n    other: '1.0.0'\n";
        let err = rewrite_catalog_yaml(src, Some("pinned"), "core", "2.0.0").expect_err("missing");
        assert_eq!(err, yaml_missing("catalogs/pinned/core"));
    }

    #[test]
    fn yaml_null_named_catalog_is_not_a_schema() {
        let src = "catalogs:\n  default: null\n  pinned:\n    core: '^9.0.0'\n";
        let err =
            rewrite_catalog_yaml(src, Some("pinned"), "core", "^0.2.0").expect_err("null table");
        assert_eq!(err, CatalogYamlError::Invalid);
    }

    #[test]
    fn yaml_null_non_default_named_catalog_is_not_a_schema() {
        let src = "catalog:\n  lodash: '^4.0.0'\ncatalogs:\n  pinned: null\n";
        let err = rewrite_catalog_yaml(src, None, "lodash", "^4.1.0").expect_err("null pinned");
        assert_eq!(err, CatalogYamlError::Invalid);
    }

    #[test]
    fn yaml_default_falls_back_to_catalogs_default() {
        let src = "catalogs:\n  default:\n    core: '^0.1.0'\n";
        let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect("yaml");
        assert_eq!(out, "catalogs:\n  default:\n    core: '^0.2.0'\n");
    }

    #[test]
    fn yaml_flow_leaf_is_schema_invalid() {
        let src = "catalog:\n  core: { version: '^0.1.0' }\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("flow");
        assert_eq!(err, CatalogYamlError::Invalid);
    }

    #[test]
    fn yaml_flow_parent_is_rewritten() {
        let src = "catalog: { core: '^0.1.0' }\n";
        let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect("flow");
        assert_eq!(out, "catalog: { core: '^0.2.0' }\n");
    }

    #[test]
    fn yaml_flow_named_catalogs_is_rewritten() {
        let src = "catalogs: { default: { core: '^9.0.0' } }\n";
        let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect("flow named");
        assert_eq!(out, "catalogs: { default: { core: '^0.2.0' } }\n");
    }

    #[test]
    fn yaml_block_scalar_is_rewritten() {
        let src = "catalog:\n  core: |\n    ^0.1.0\n";
        let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect("block");
        assert_eq!(out, "catalog:\n  core: |\n    ^0.2.0\n");
    }

    #[test]
    fn yaml_folded_scalar_is_rewritten() {
        let src = "catalog:\n  core: >\n    ^0.1.0\n";
        let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect("folded");
        assert_eq!(out, "catalog:\n  core: >\n    ^0.2.0\n");
    }

    const BLANK_PINS: [&str; 13] = [
        "catalog:\n  core: !!str ''\n",
        "catalog:\n  core: |\n",
        "catalog:\n  core: |-\n",
        "catalog:\n  core: >+\n",
        "catalog:\n  core: |2\n",
        "catalog:\n  core: | # note\n",
        "catalog:\n  core: |+\n\n\n  other: '^1.0.0'\n",
        "catalog:\n  core: !!str |\n",
        "catalog:\n  core: ''\n",
        "catalog:\n  core: ' '\n",
        "catalog:\n  core: \"\"\n",
        "catalog: { core: '' }\n",
        "catalogs:\n  default:\n    core: ''\n",
    ];

    #[test]
    fn yaml_blank_pin_retargeted_to_blank_is_unchanged() {
        for src in BLANK_PINS {
            let out = rewrite_catalog_yaml(src, None, "core", "").expect(src);
            assert_eq!(out, src);
        }
    }

    /// Discovery refuses these as ranges, so `version` never retargets them.
    #[test]
    fn yaml_tagged_str_tilde_and_null_are_strings() {
        for src in [
            "catalog:\n  core: !!str ~\n",
            "catalog:\n  core: !!str null\n",
        ] {
            let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect(src);
            assert_eq!(out, "catalog:\n  core: !!str ^0.2.0\n");
        }
    }

    /// A blank scalar's span can cover its block indicator and a trailing
    /// comment, so writing over it would drop the comment.
    #[test]
    fn yaml_blank_pin_is_not_a_string_for_a_range() {
        for src in BLANK_PINS {
            let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err(src);
            assert!(
                matches!(err, CatalogYamlError::NotString { .. }),
                "{src}: {err}"
            );
        }
    }

    #[test]
    fn yaml_multiline_block_scalar_is_not_a_string() {
        let src = "catalog:\n  core: |\n    ^0.1.0\n    extra\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("multiline");
        assert_eq!(err, yaml_not_string("catalog/core"));
    }

    #[test]
    fn yaml_multiline_chomped_literal_is_not_a_string() {
        for src in [
            "catalog:\n  core: |-\n    ^0.1.0\n    extra\n",
            "catalog:\n  core: |+\n    ^0.1.0\n    extra\n",
        ] {
            let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err(src);
            assert_eq!(err, yaml_not_string("catalog/core"), "{src}");
        }
    }

    #[test]
    fn yaml_multiline_folded_scalar_is_not_a_string() {
        let src = "catalog:\n  core: >\n    ^0.1.0\n    extra\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("folded");
        assert_eq!(err, yaml_not_string("catalog/core"));
    }

    #[test]
    fn yaml_multiline_quoted_is_not_a_string() {
        for src in [
            "catalog:\n  core: \"hello\n    world\"\n",
            "catalog:\n  core: 'hello\n    world'\n",
        ] {
            let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err(src);
            assert_eq!(err, yaml_not_string("catalog/core"), "{src}");
        }
    }

    #[test]
    fn yaml_folded_cr_is_not_a_string() {
        let src = "catalog:\n  core: >\n    ^0.1.0\r    extra\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("cr");
        assert_eq!(err, yaml_not_string("catalog/core"));
    }

    #[test]
    fn yaml_underindented_quoted_is_invalid() {
        let src = "catalog:\n  core: \"hello\n  world\"\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("indent");
        assert_eq!(err, CatalogYamlError::Invalid);
    }

    #[test]
    fn yaml_continued_plain_is_not_a_string() {
        let src = "catalog:\n  core: hello\n    world\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("continued");
        assert_eq!(err, yaml_not_string("catalog/core"));
    }

    #[test]
    fn yaml_bom_flow_is_rewritten() {
        let src = "\u{feff}catalog: { core: '^0.1.0' }\n";
        let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect("bom");
        assert_eq!(out, "\u{feff}catalog: { core: '^0.2.0' }\n");
    }

    #[test]
    fn yaml_non_ascii_neighbor_is_rewritten() {
        let src = "catalog:\n  other: 'café'\n  core: '^0.1.0'\n";
        let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect("utf8");
        assert_eq!(out, "catalog:\n  other: 'café'\n  core: '^0.2.0'\n");
    }

    #[test]
    fn yaml_non_ascii_pin_is_rewritten() {
        let src = "catalog:\n  core: 'café'\n";
        let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect("utf8 pin");
        assert_eq!(out, "catalog:\n  core: '^0.2.0'\n");
    }

    #[test]
    fn yaml_bom_non_ascii_neighbor_is_rewritten() {
        let src = "\u{feff}catalog:\n  other: 'café'\n  core: '^0.1.0'\n";
        let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect("bom utf8");
        assert_eq!(out, "\u{feff}catalog:\n  other: 'café'\n  core: '^0.2.0'\n");
    }

    #[test]
    fn yaml_defined_alias_is_written_out() {
        let src = "x: &p '^0.1.0'\ncatalog:\n  core: *p\n";
        assert_eq!(
            noted(src, "^0.2.0"),
            (
                "x: &p '^0.1.0'\ncatalog:\n  core: '^0.2.0'\n".to_owned(),
                vec![
                    "`catalog/core` was the alias `*p`; it is written out, so `&p` does not move with it"
                        .to_owned()
                ]
            )
        );
    }

    fn noted(src: &str, range: &str) -> (String, Vec<String>) {
        let (out, notes) = super::rewrite_catalog_yaml(src, None, "core", range).expect(src);
        (out, notes.iter().map(ToString::to_string).collect())
    }

    #[test]
    fn yaml_skipped_mapping_alias_is_written_out() {
        let src = "defaults:\n  pin: &p '^0.1.0'\ncatalog:\n  core: *p\n";
        let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect("skipped anchor");
        assert_eq!(
            out,
            "defaults:\n  pin: &p '^0.1.0'\ncatalog:\n  core: '^0.2.0'\n"
        );
    }

    #[test]
    fn yaml_skipped_nested_mapping_alias_is_written_out() {
        let src = "outer:\n  inner:\n    pin: &p '^0.1.0'\ncatalog:\n  core: *p\n";
        let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect("nested skip");
        assert_eq!(
            out,
            "outer:\n  inner:\n    pin: &p '^0.1.0'\ncatalog:\n  core: '^0.2.0'\n"
        );
    }

    #[test]
    fn yaml_skipped_sequence_alias_is_written_out() {
        let src = "items:\n  - &p '^0.1.0'\ncatalog:\n  core: *p\n";
        let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect("sequence skip");
        assert_eq!(out, "items:\n  - &p '^0.1.0'\ncatalog:\n  core: '^0.2.0'\n");
    }

    #[test]
    fn yaml_flow_alias_is_written_out_quoted() {
        let src = "x: &p \"npm:@o/core@^0.1.0\"\ncatalog: { core: *p, other: *p }\n";
        let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect("flow alias");
        assert_eq!(
            out,
            "x: &p \"npm:@o/core@^0.1.0\"\ncatalog: { core: 'npm:@o/core@^0.2.0', other: *p }\n"
        );
    }

    #[test]
    fn yaml_alias_to_a_range_yaml_reads_as_an_alias_is_written_out_quoted() {
        let src = "x: &p ^0.1.0\ncatalog:\n  core: *p\n";
        let (out, _) = noted(src, "*");
        assert_eq!(out, "x: &p ^0.1.0\ncatalog:\n  core: '*'\n");
    }

    #[test]
    fn yaml_anchored_pin_writes_its_aliases_out() {
        let src = "catalog:\n  core: &p ^0.1.0\ncatalogs:\n  spare:\n    x: *p\n    y: *p\n";
        assert_eq!(
            noted(src, "^0.2.0"),
            (
                "catalog:\n  core: &p ^0.2.0\ncatalogs:\n  spare:\n    x: '^0.1.0'\n    y: '^0.1.0'\n".to_owned(),
                vec!["`catalog/core` carries `&p`; its 2 aliases are written out rather than moved with it".to_owned()]
            )
        );
    }

    #[test]
    fn yaml_written_out_alias_keeps_its_string_and_its_neighbours() {
        let cases = [
            (
                "catalog:\n  core: &p !!str 1\ncatalogs:\n  s:\n    z: *p\n",
                "catalog:\n  core: &p !!str 2\ncatalogs:\n  s:\n    z: '1'\n",
            ),
            (
                "catalog:\n  core: &p ^1,y\ncatalogs:\n  s: { x: *p }\n",
                "catalog:\n  core: &p ^2\ncatalogs:\n  s: { x: '^1,y' }\n",
            ),
        ];
        for (src, want) in cases {
            let range = if src.contains("!!str") { "2" } else { "^2" };
            assert_eq!(noted(src, range).0, want, "{src}");
        }
    }

    #[test]
    fn yaml_written_out_aliases_are_only_those_of_the_pins_anchor() {
        let src = "catalog:\n  core: &p ^0.1.0\n  other: &q ^9.0.0\ncatalogs:\n  s:\n    x: *q\n    y: *p\n";
        assert_eq!(
            noted(src, "^0.2.0"),
            (
                "catalog:\n  core: &p ^0.2.0\n  other: &q ^9.0.0\ncatalogs:\n  s:\n    x: *q\n    y: '^0.1.0'\n".to_owned(),
                vec!["`catalog/core` carries `&p`; its 1 alias is written out rather than moved with it".to_owned()]
            )
        );
    }

    #[test]
    fn yaml_written_out_alias_after_non_ascii_text_lands_on_the_alias() {
        let src = "x: &p 'é^0.1.0'\ncatalog:\n  café: 'ü'\n  core: *p\n";
        assert_eq!(
            noted(src, "^0.2.0").0,
            "x: &p 'é^0.1.0'\ncatalog:\n  café: 'ü'\n  core: '^0.2.0'\n"
        );
    }

    #[test]
    fn yaml_block_anchor_written_out_drops_its_final_newline() {
        let src = "catalog:\n  core: &p |\n    ^0.1.0\ncatalogs:\n  s:\n    z: *p\n";
        let (out, notes) = noted(src, "^0.2.0");
        assert!(out.ends_with("    z: '^0.1.0'\n"), "{out}");
        assert_eq!(
            notes,
            ["`catalog/core` carries `&p`; its 1 alias is written out rather than moved with it"]
        );
    }

    #[test]
    fn yaml_table_anchor_moving_the_same_package_elsewhere_is_named() {
        let src = "catalogs:\n  a: &m\n    core: '^0.1.0'\n  b: *m\n";
        let (out, notes) =
            super::rewrite_catalog_yaml(src, Some("a"), "core", "^0.2.0").expect("table anchor");
        assert_eq!(out, "catalogs:\n  a: &m\n    core: '^0.2.0'\n  b: *m\n");
        assert_eq!(
            notes.iter().map(ToString::to_string).collect::<Vec<_>>(),
            ["`catalogs/a/core` also moves `catalogs/b/core`, which reach it through an alias"]
        );
    }

    #[test]
    fn yaml_alias_already_at_the_range_is_unchanged() {
        let src = "x: &p '^0.2.0'\ncatalog:\n  core: *p\n";
        assert_eq!(noted(src, "^0.2.0"), (src.to_owned(), Vec::new()));
    }

    #[test]
    fn yaml_plain_pin_whose_new_range_yaml_reads_differently_is_refused() {
        for range in ["*", "^1 #x"] {
            let err = rewrite_catalog_yaml("catalog:\n  core: ^0.1.0\n", None, "core", range)
                .expect_err(range);
            assert_eq!(err, yaml_not_string("catalog/core"), "{range}");
        }
    }

    #[test]
    fn yaml_merge_key_pin_is_not_a_string() {
        let src = "defaults: &d\n  core: '^0.1.0'\ncatalog:\n  <<: *d\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("merge");
        assert_eq!(err, yaml_not_string("catalog/core"));
    }

    #[test]
    fn yaml_flow_comment_is_kept() {
        let src = "catalog: { core: '^0.1.0' } # pin\n";
        let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect("flow comment");
        assert_eq!(out, "catalog: { core: '^0.2.0' } # pin\n");
    }

    #[test]
    fn yaml_duplicate_leaf_is_an_error() {
        let src = "catalog:\n  core: '^0.1.0'\n  core: '^0.3.0'\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("dup");
        assert_eq!(err, CatalogYamlError::Invalid);
    }

    #[test]
    fn yaml_npm_alias_keeps_package_name() {
        let src = "catalog:\n  core: 'npm:@oakum/core@^0.1.0'\n";
        let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect("yaml");
        assert_eq!(out, "catalog:\n  core: 'npm:@oakum/core@^0.2.0'\n");
    }

    #[test]
    fn yaml_unquoted_npm_alias_is_rewritten() {
        let src = "catalog:\n  core: npm:@oakum/core@^0.1.0\n";
        let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect("yaml");
        assert_eq!(out, "catalog:\n  core: npm:@oakum/core@^0.2.0\n");
    }

    #[test]
    fn yaml_anchored_flow_catalogs_default_xor_block_catalog() {
        let src = "catalog:\n  core: '^0.1.0'\ncatalogs: &c { default: { core: '^9.0.0' } }\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("xor");
        assert_eq!(err, yaml_duplicate("catalog"));
    }

    #[test]
    fn json_npm_alias_keeps_package_name() {
        let src = "{\n  \"catalog\": {\n    \"core\": \"npm:@oakum/core@^0.1.0\"\n  }\n}\n";
        let out = rewrite_catalog_json(src, None, "core", "^0.2.0").expect("json");
        assert_eq!(
            out,
            "{\n  \"catalog\": {\n    \"core\": \"npm:@oakum/core@^0.2.0\"\n  }\n}\n"
        );
    }

    #[test]
    fn yaml_anchored_quoted_pin_is_rewritten() {
        let src = "catalog:\n  core: &pin '^0.1.0'\n";
        let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect("anchor");
        assert_eq!(out, "catalog:\n  core: &pin '^0.2.0'\n");
    }

    #[test]
    fn yaml_unquoted_schema_string_is_rewritten() {
        for value in ["1", "true", "yes", "0B10", "2024-01-01"] {
            let src = format!("catalog:\n  core: {value}\n");
            let out = rewrite_catalog_yaml(&src, None, "core", "^0.2.0").expect(value);
            assert_eq!(out, "catalog:\n  core: ^0.2.0\n", "{value}");
        }
    }

    #[test]
    fn yaml_unquoted_non_string_is_not_a_string() {
        for value in ["null", "Null", "~"] {
            let src = format!("catalog:\n  core: {value}\n");
            let err = rewrite_catalog_yaml(&src, None, "core", "^0.2.0").expect_err(value);
            assert_eq!(err, yaml_not_string("catalog/core"), "{value}");
        }
        for value in ["!!int 1", "*p"] {
            let src = format!("catalog:\n  core: {value}\n");
            let err = rewrite_catalog_yaml(&src, None, "core", "^0.2.0").expect_err(value);
            assert_eq!(err, CatalogYamlError::Invalid, "{value}");
        }
    }

    #[test]
    fn yaml_named_unquoted_non_string_is_not_a_string() {
        for value in ["null", "Null", "~"] {
            let src = format!("catalogs:\n  pinned:\n    core: {value}\n");
            let err = rewrite_catalog_yaml(&src, Some("pinned"), "core", "2.0.0").expect_err(value);
            assert_eq!(err, yaml_not_string("catalogs/pinned/core"), "{value}");
        }
        for value in ["!!int 1", "*p"] {
            let src = format!("catalogs:\n  pinned:\n    core: {value}\n");
            let err = rewrite_catalog_yaml(&src, Some("pinned"), "core", "2.0.0").expect_err(value);
            assert_eq!(err, CatalogYamlError::Invalid, "{value}");
        }
    }

    #[test]
    fn yaml_quoted_escape_is_rewritten() {
        let src = "catalog:\n  core: 'it''s'\n";
        let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect("single");
        assert_eq!(out, "catalog:\n  core: '^0.2.0'\n");
        let src = "catalog:\n  core: \"a\\\"b\"\n";
        let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect("double");
        assert_eq!(out, "catalog:\n  core: \"^0.2.0\"\n");
        let src = "catalog:\n  core: 'unclosed\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err(src);
        assert_eq!(err, CatalogYamlError::Invalid, "{src}");
    }

    #[test]
    fn yaml_quoted_lookalike_is_rewritten() {
        let src = "catalog:\n  core: '1'\n";
        let out = rewrite_catalog_yaml(src, None, "core", "1.0.0").expect("quoted");
        assert_eq!(out, "catalog:\n  core: '1.0.0'\n");
        let src = "catalog:\n  core: \"null\"\n";
        let out = rewrite_catalog_yaml(src, None, "core", "1.0.0").expect("quoted");
        assert_eq!(out, "catalog:\n  core: \"1.0.0\"\n");
    }

    #[test]
    fn yaml_unquoted_version_is_rewritten() {
        let src = "catalog:\n  core: ^0.1.0\n";
        let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect("yaml");
        assert_eq!(out, "catalog:\n  core: ^0.2.0\n");
    }
}
