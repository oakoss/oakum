//! Catalog pins live in `pnpm-workspace.yaml`, `.yarnrc.yml`, or
//! `package.json`, not on the member `catalog:` line
//! (`DeclaredRange::retargeted_text` is `None` there).

use std::collections::BTreeMap;

use saphyr_parser::{Event, Parser, ScalarStyle, Span as YamlSpan, SpannedEventReceiver};

use super::json::{json_string, json_table_present, replace_json_string, JsonEditError};
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
    replace_json_string(text, path, &retarget_catalog_value(&current, new_range))
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
/// are refused. A defined alias rewrites the anchored scalar.
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
) -> Result<String, CatalogYamlError> {
    let file = CatalogFile::parse(text).map_err(|_| CatalogYamlError::Invalid)?;
    if file.has_null_named_table() {
        return Err(CatalogYamlError::Invalid);
    }
    let bom = text.starts_with('\u{feff}');
    let body = text.strip_prefix('\u{feff}').unwrap_or(text);
    let span = match catalog_target(
        catalog,
        package,
        file.catalog.is_some(),
        file.has_default_table(),
    ) {
        CatalogTarget::Duplicate => return Err(CatalogYamlError::duplicate(&["catalog"])),
        CatalogTarget::At { path, missing_path } => {
            match find_yaml_string(body, &path, file.string_at(&path).is_some()) {
                Ok(span) => span,
                Err(CatalogYamlError::Missing { .. }) => {
                    return Err(CatalogYamlError::Missing { path: missing_path })
                }
                Err(err) => return Err(err),
            }
        }
    };
    let next = retarget_catalog_value(&body[span.start..span.end], new_range);
    let mut out = String::with_capacity(
        usize::from(bom) * "\u{feff}".len() + body.len() - (span.end - span.start) + next.len(),
    );
    if bom {
        out.push('\u{feff}');
    }
    out.push_str(&body[..span.start]);
    out.push_str(&next);
    out.push_str(&body[span.end..]);
    Ok(out)
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
    fn missing(path: &[&str]) -> Self {
        Self::Missing {
            path: path.join("/"),
        }
    }

    fn not_string(path: &[&str]) -> Self {
        Self::NotString {
            path: path.join("/"),
        }
    }

    fn duplicate(path: &[&str]) -> Self {
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

struct Span {
    start: usize,
    end: usize,
}

fn find_yaml_string(
    text: &str,
    path: &[&str],
    schema_is_string: bool,
) -> Result<Span, CatalogYamlError> {
    if path.is_empty() {
        return Err(CatalogYamlError::missing(path));
    }
    let mut finder = PathFinder {
        path,
        keys: Vec::new(),
        pending: None,
        expect_key: false,
        skip: 0,
        mapping_depth: 0,
        found: None,
        err: None,
        anchors: BTreeMap::new(),
    };
    if Parser::new_from_str(text).load(&mut finder, false).is_err() {
        return Err(finder.err.unwrap_or(CatalogYamlError::Invalid));
    }
    if let Some(err) = finder.err {
        return Err(err);
    }
    let Some((start, end, style, decoded)) = finder.found else {
        return Err(if schema_is_string {
            CatalogYamlError::not_string(path)
        } else {
            CatalogYamlError::missing(path)
        });
    };
    if !schema_is_string {
        return Err(CatalogYamlError::not_string(path));
    }
    let start = char_index_to_byte(text, start)?;
    let end = char_index_to_byte(text, end)?;
    scalar_rewrite_span(text, start, end, style, &decoded, path)
}

/// saphyr-parser 0.0.12 `Marker::index` is a character offset.
fn char_index_to_byte(text: &str, chars: usize) -> Result<usize, CatalogYamlError> {
    if chars == 0 {
        return Ok(0);
    }
    let mut count = 0;
    for (byte, _) in text.char_indices() {
        if count == chars {
            return Ok(byte);
        }
        count += 1;
    }
    if count == chars {
        Ok(text.len())
    } else {
        Err(CatalogYamlError::Invalid)
    }
}

struct PathFinder<'a> {
    path: &'a [&'a str],
    keys: Vec<String>,
    pending: Option<String>,
    expect_key: bool,
    skip: usize,
    mapping_depth: usize,
    found: Option<(usize, usize, ScalarStyle, String)>,
    err: Option<CatalogYamlError>,
    anchors: BTreeMap<usize, (usize, usize, ScalarStyle, String)>,
}

impl PathFinder<'_> {
    fn at_target(&self) -> bool {
        let Some(key) = self.pending.as_deref() else {
            return false;
        };
        self.keys.len() + 1 == self.path.len()
            && self
                .keys
                .iter()
                .zip(self.path.iter())
                .all(|(have, want)| have == *want)
            && self.path[self.keys.len()] == key
    }

    fn continues(&self) -> bool {
        let Some(key) = self.pending.as_deref() else {
            return false;
        };
        let depth = self.keys.len();
        depth < self.path.len()
            && self
                .keys
                .iter()
                .zip(self.path.iter())
                .all(|(have, want)| have == *want)
            && self.path[depth] == key
    }

    fn fail(&mut self, err: CatalogYamlError) {
        if self.err.is_none() {
            self.err = Some(err);
        }
    }

    fn record_scalar(&mut self, start: usize, end: usize, style: ScalarStyle, decoded: String) {
        if self.found.is_some() {
            self.fail(CatalogYamlError::duplicate(self.path));
            return;
        }
        self.found = Some((start, end, style, decoded));
    }

    fn refuse_target(&mut self) {
        if self.found.is_some() {
            self.fail(CatalogYamlError::duplicate(self.path));
        } else {
            self.fail(CatalogYamlError::not_string(self.path));
        }
    }

    fn mapping_start(&mut self) {
        if self.pending.is_none() && self.keys.is_empty() && !self.expect_key {
            self.mapping_depth += 1;
            self.expect_key = true;
            return;
        }
        if self.expect_key {
            self.fail(CatalogYamlError::Invalid);
            return;
        }
        if self.at_target() {
            self.refuse_target();
            self.skip = 1;
            self.pending = None;
            self.expect_key = true;
            return;
        }
        if self.continues() {
            if let Some(key) = self.pending.take() {
                self.keys.push(key);
            }
            self.mapping_depth += 1;
            self.expect_key = true;
            return;
        }
        self.skip = 1;
        self.pending = None;
        self.expect_key = true;
    }

    fn sequence_start(&mut self) {
        if self.expect_key {
            self.fail(CatalogYamlError::Invalid);
            return;
        }
        if self.at_target() {
            self.refuse_target();
        } else if self.continues() {
            self.fail(CatalogYamlError::not_string(self.path));
        }
        self.skip = 1;
        self.pending = None;
        self.expect_key = true;
    }

    fn mapping_end(&mut self) {
        self.mapping_depth = self.mapping_depth.saturating_sub(1);
        self.keys.pop();
        self.pending = None;
        self.expect_key = self.mapping_depth > 0;
    }

    fn scalar(
        &mut self,
        value: String,
        style: ScalarStyle,
        anchor_id: usize,
        start: usize,
        end: usize,
    ) {
        if self.expect_key {
            self.pending = Some(value);
            self.expect_key = false;
            return;
        }
        if anchor_id > 0 {
            self.anchors
                .insert(anchor_id, (start, end, style, value.clone()));
        }
        if self.at_target() {
            self.record_scalar(start, end, style, value);
        } else if self.continues() {
            self.fail(CatalogYamlError::not_string(self.path));
        }
        self.pending = None;
        self.expect_key = true;
    }

    fn alias(&mut self, id: usize) {
        if self.expect_key {
            self.fail(CatalogYamlError::Invalid);
            return;
        }
        if self.at_target() {
            if let Some((start, end, style, decoded)) = self.anchors.get(&id).cloned() {
                self.record_scalar(start, end, style, decoded);
            } else {
                self.refuse_target();
            }
        } else if self.continues() {
            self.fail(CatalogYamlError::not_string(self.path));
        }
        self.pending = None;
        self.expect_key = true;
    }
}

impl<'input> SpannedEventReceiver<'input> for PathFinder<'_> {
    fn on_event(&mut self, ev: Event<'input>, span: YamlSpan) {
        if self.err.is_some() {
            return;
        }
        if self.skip > 0 {
            match ev {
                Event::MappingStart(_, _) | Event::SequenceStart(_, _) => self.skip += 1,
                Event::MappingEnd | Event::SequenceEnd => self.skip -= 1,
                Event::Scalar(value, style, anchor_id, _) if anchor_id > 0 => {
                    self.anchors.insert(
                        anchor_id,
                        (
                            span.start.index(),
                            span.end.index(),
                            style,
                            value.into_owned(),
                        ),
                    );
                }
                _ => {}
            }
            return;
        }
        match ev {
            Event::MappingStart(_, _) => self.mapping_start(),
            Event::SequenceStart(_, _) => self.sequence_start(),
            Event::MappingEnd => self.mapping_end(),
            Event::Scalar(value, style, anchor_id, _) => self.scalar(
                value.into_owned(),
                style,
                anchor_id,
                span.start.index(),
                span.end.index(),
            ),
            Event::Alias(id) => self.alias(id),
            Event::StreamStart
            | Event::StreamEnd
            | Event::DocumentStart(_)
            | Event::DocumentEnd
            | Event::SequenceEnd
            | Event::Nothing => {}
        }
    }
}

fn scalar_rewrite_span(
    text: &str,
    start: usize,
    end: usize,
    style: ScalarStyle,
    decoded: &str,
    path: &[&str],
) -> Result<Span, CatalogYamlError> {
    let bytes = text.as_bytes();
    if start > end || end > bytes.len() {
        return Err(CatalogYamlError::Invalid);
    }
    if decoded.strip_suffix('\n').unwrap_or(decoded).contains('\n') {
        return Err(CatalogYamlError::not_string(path));
    }
    let mut s = start;
    let mut e = end;
    while s < e && bytes[s].is_ascii_whitespace() {
        s += 1;
    }
    while e > s && bytes[e - 1].is_ascii_whitespace() {
        e -= 1;
    }
    match style {
        ScalarStyle::SingleQuoted => {
            let close = close_single_quote(bytes, s, e)
                .ok_or_else(|| CatalogYamlError::not_string(path))?;
            s += 1;
            e = close;
        }
        ScalarStyle::DoubleQuoted => {
            let close = close_double_quote(bytes, s, e)
                .ok_or_else(|| CatalogYamlError::not_string(path))?;
            s += 1;
            e = close;
        }
        ScalarStyle::Literal | ScalarStyle::Folded => {
            if e > s && bytes[e - 1] == b'\n' {
                e -= 1;
            }
        }
        ScalarStyle::Plain => {
            let slice = std::str::from_utf8(&bytes[s..e]).map_err(|_| CatalogYamlError::Invalid)?;
            if slice != decoded {
                return Err(CatalogYamlError::not_string(path));
            }
        }
    }
    if s >= e || bytes[s..e].contains(&b'\n') || bytes[s..e].contains(&b'\r') {
        return Err(CatalogYamlError::not_string(path));
    }
    Ok(Span { start: s, end: e })
}

fn close_single_quote(bytes: &[u8], start: usize, end: usize) -> Option<usize> {
    if start >= end || bytes[start] != b'\'' {
        return None;
    }
    let mut i = start + 1;
    while i < end {
        if bytes[i] == b'\'' {
            if i + 1 < end && bytes[i + 1] == b'\'' {
                i += 2;
                continue;
            }
            return Some(i);
        }
        i += 1;
    }
    None
}

fn close_double_quote(bytes: &[u8], start: usize, end: usize) -> Option<usize> {
    if start >= end || bytes[start] != b'"' {
        return None;
    }
    let mut i = start + 1;
    while i < end {
        if bytes[i] == b'\\' {
            i = i.saturating_add(2);
            continue;
        }
        if bytes[i] == b'"' {
            return Some(i);
        }
        i += 1;
    }
    None
}

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
    use super::{
        rewrite_catalog_json, rewrite_catalog_yaml, yaml_has_catalog_table, CatalogYamlError,
    };

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

    #[test]
    fn yaml_empty_block_scalar_is_not_a_string() {
        let src = "catalog:\n  core: |\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("empty");
        assert_eq!(err, yaml_not_string("catalog/core"));
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
    fn yaml_defined_alias_is_rewritten() {
        let src = "x: &p '^0.1.0'\ncatalog:\n  core: *p\n";
        let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect("alias");
        assert_eq!(out, "x: &p '^0.2.0'\ncatalog:\n  core: *p\n");
    }

    #[test]
    fn yaml_skipped_mapping_alias_is_rewritten() {
        let src = "defaults:\n  pin: &p '^0.1.0'\ncatalog:\n  core: *p\n";
        let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect("skipped anchor");
        assert_eq!(out, "defaults:\n  pin: &p '^0.2.0'\ncatalog:\n  core: *p\n");
    }

    #[test]
    fn yaml_skipped_nested_mapping_alias_is_rewritten() {
        let src = "outer:\n  inner:\n    pin: &p '^0.1.0'\ncatalog:\n  core: *p\n";
        let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect("nested skip");
        assert_eq!(
            out,
            "outer:\n  inner:\n    pin: &p '^0.2.0'\ncatalog:\n  core: *p\n"
        );
    }

    #[test]
    fn yaml_skipped_sequence_alias_is_rewritten() {
        let src = "items:\n  - &p '^0.1.0'\ncatalog:\n  core: *p\n";
        let out = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect("sequence skip");
        assert_eq!(out, "items:\n  - &p '^0.2.0'\ncatalog:\n  core: *p\n");
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
            assert_eq!(out, format!("catalog:\n  core: ^0.2.0\n"), "{value}");
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
