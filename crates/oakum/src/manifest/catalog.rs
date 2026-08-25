//! Catalog pins live in `pnpm-workspace.yaml`, `.yarnrc.yml`, or
//! `package.json`, not on the member `catalog:` line
//! (`DeclaredRange::retargeted_text` is `None` there).

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
/// Comments, indent, and newlines stay. Flow mappings, sequences, and
/// block scalars are refused.
///
/// # Errors
///
/// Returns [`CatalogYamlError`] when the file is not a catalog schema, or
/// the entry is missing, duplicated, or not a plain string.
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
    let span = match catalog_target(
        catalog,
        package,
        file.catalog.is_some(),
        file.has_default_table(),
    ) {
        CatalogTarget::Duplicate => return Err(CatalogYamlError::duplicate(&["catalog"])),
        CatalogTarget::At { path, missing_path } => {
            match find_yaml_string(text, &path, file.string_at(&path).is_some()) {
                Ok(span) => span,
                Err(CatalogYamlError::Missing { .. }) => {
                    return Err(CatalogYamlError::Missing { path: missing_path })
                }
                Err(err) => return Err(err),
            }
        }
    };
    let next = retarget_catalog_value(&text[span.start..span.end], new_range);
    let mut out = String::with_capacity(text.len() - (span.end - span.start) + next.len());
    out.push_str(&text[..span.start]);
    out.push_str(&next);
    out.push_str(&text[span.end..]);
    Ok(out)
}

/// Missing `catalog` / `catalogs` is `false`. A present table that is not a
/// map is [`CatalogYamlError::Invalid`], matching [`rewrite_catalog_yaml`].
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
                write!(f, "catalog path `{path}` is not a rewriteable plain string")
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
    let Some((leaf, parents)) = path.split_last() else {
        return Err(CatalogYamlError::missing(path));
    };
    let mut stack: Vec<(usize, &str)> = Vec::new();
    let mut found = None;
    let mut offset = 0;
    for line in text.split_inclusive(['\n']) {
        let line_start = offset;
        offset += line.len();
        let body = line
            .strip_suffix("\r\n")
            .or_else(|| line.strip_suffix('\n'))
            .unwrap_or(line);
        let trimmed = body.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = body.len() - trimmed.len();
        while stack.last().is_some_and(|(parent, _)| *parent >= indent) {
            stack.pop();
        }
        let Some((key, rest, colon)) = split_yaml_key(trimmed) else {
            continue;
        };
        let parent_ok = stack.len() == parents.len()
            && stack
                .iter()
                .zip(parents.iter())
                .all(|((_, have), want)| *have == *want);
        if rest_is_flow(rest) {
            if parent_ok && key == *leaf {
                return Err(CatalogYamlError::not_string(path));
            }
            if rest_is_flow_map(rest) && stack.len() < parents.len() && parents[stack.len()] == key
            {
                return Err(CatalogYamlError::not_string(path));
            }
            continue;
        }
        if let Some((child, child_rest)) = implicit_nested(rest) {
            if parent_ok && key == *leaf {
                return Err(CatalogYamlError::not_string(path));
            }
            if stack.len() < parents.len() && parents[stack.len()] == key {
                if stack.len() + 1 == parents.len() && child == *leaf {
                    return Err(CatalogYamlError::not_string(path));
                }
                if parents.get(stack.len() + 1) == Some(&child) {
                    stack.push((indent, key));
                    if rest_is_flow(child_rest) {
                        return Err(CatalogYamlError::not_string(path));
                    }
                    if rest_is_empty_mapping(child_rest) {
                        stack.push((indent + 1, child));
                    }
                    continue;
                }
            }
        }
        if parent_ok && key == *leaf {
            let span = scalar_span(line_start, indent, colon, rest, schema_is_string, path)?;
            if found.is_some() {
                return Err(CatalogYamlError::duplicate(path));
            }
            found = Some(span);
            continue;
        }
        if rest_is_empty_mapping(rest) {
            stack.push((indent, key));
        }
    }
    found.ok_or_else(|| CatalogYamlError::missing(path))
}

fn rest_is_flow(rest: &str) -> bool {
    matches!(yaml_plain_value(rest).as_bytes().first(), Some(b'{' | b'['))
}

fn rest_is_flow_map(rest: &str) -> bool {
    yaml_plain_value(rest).starts_with('{')
}

fn rest_is_empty_mapping(rest: &str) -> bool {
    let value = yaml_plain_value(rest);
    value.is_empty() || value.starts_with('#')
}

fn yaml_plain_value(rest: &str) -> &str {
    let mut value = rest.trim_start();
    while let Some(next) = strip_yaml_prefix(value) {
        value = next;
    }
    value
}

fn strip_yaml_prefix(value: &str) -> Option<&str> {
    if let Some(rest) = value.strip_prefix('&') {
        let end = rest
            .find(|c: char| c.is_whitespace() || c == '{' || c == '[')
            .unwrap_or(rest.len());
        if end == 0 {
            return None;
        }
        return Some(rest[end..].trim_start());
    }
    let tagged = value
        .strip_prefix("!!")
        .or_else(|| value.strip_prefix('!'))?;
    let end = tagged
        .find(|c: char| c.is_whitespace() || c == '{' || c == '[')
        .unwrap_or(tagged.len());
    if end == 0 {
        return None;
    }
    Some(tagged[end..].trim_start())
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

fn implicit_nested(rest: &str) -> Option<(&str, &str)> {
    let value = yaml_plain_value(rest);
    if value.is_empty() || value.starts_with('#') || value.starts_with(['{', '[']) {
        return None;
    }
    if value
        .as_bytes()
        .first()
        .is_some_and(|b| *b == b'\'' || *b == b'"')
    {
        return None;
    }
    let colon = value.find(':')?;
    let after = &value[colon + 1..];
    if !after.is_empty() && !after.starts_with(char::is_whitespace) {
        return None;
    }
    let key = unquote(value[..colon].trim())?;
    Some((key, after))
}

fn split_yaml_key(trimmed: &str) -> Option<(&str, &str, usize)> {
    let colon = trimmed.find(':')?;
    let raw = trimmed[..colon].trim();
    let key = unquote(raw)?;
    Some((key, &trimmed[colon + 1..], colon))
}

fn unquote(raw: &str) -> Option<&str> {
    if raw.len() >= 2 {
        let bytes = raw.as_bytes();
        if (bytes[0] == b'\'' && bytes[raw.len() - 1] == b'\'')
            || (bytes[0] == b'"' && bytes[raw.len() - 1] == b'"')
        {
            return Some(&raw[1..raw.len() - 1]);
        }
    }
    if raw.is_empty() {
        return None;
    }
    Some(raw)
}

fn scalar_span(
    line_start: usize,
    indent: usize,
    colon_in_trimmed: usize,
    rest: &str,
    schema_is_string: bool,
    path: &[&str],
) -> Result<Span, CatalogYamlError> {
    let value_part = rest.trim_start();
    if value_part.is_empty() || value_part.starts_with('#') {
        return Err(CatalogYamlError::not_string(path));
    }
    if matches!(
        value_part.as_bytes().first(),
        Some(b'|' | b'>' | b'{' | b'[')
    ) {
        return Err(CatalogYamlError::not_string(path));
    }
    let leading = rest.len() - value_part.len();
    let (quoted, inner_start, inner_len) = if value_part
        .as_bytes()
        .first()
        .is_some_and(|b| *b == b'\'' || *b == b'"')
    {
        let quote = value_part.as_bytes()[0];
        let Some(end) = value_part[1..].find(quote as char) else {
            return Err(CatalogYamlError::not_string(path));
        };
        let after = value_part[1 + end + 1..].trim_start();
        if !after.is_empty() && !after.starts_with('#') {
            return Err(CatalogYamlError::not_string(path));
        }
        (true, 1, end)
    } else {
        let end = value_part
            .find('#')
            .map_or(value_part.trim_end().len(), |i| {
                value_part[..i].trim_end().len()
            });
        if end == 0 {
            return Err(CatalogYamlError::not_string(path));
        }
        if !unquoted_scalar_is_string(&value_part[..end], schema_is_string) {
            return Err(CatalogYamlError::not_string(path));
        }
        (false, 0, end)
    };
    let key_prefix = indent + colon_in_trimmed + 1 + leading;
    let start = line_start + key_prefix + if quoted { inner_start } else { 0 };
    Ok(Span {
        start,
        end: start + inner_len,
    })
}

fn unquoted_scalar_is_string(value: &str, schema_is_string: bool) -> bool {
    match value.as_bytes().first() {
        Some(b'&' | b'*' | b'!') => false,
        _ if schema_is_string => true,
        _ => !yaml_non_string_token(value),
    }
}

fn yaml_non_string_token(value: &str) -> bool {
    matches!(
        value,
        "null"
            | "Null"
            | "NULL"
            | "~"
            | "true"
            | "True"
            | "TRUE"
            | "false"
            | "False"
            | "FALSE"
            | "yes"
            | "Yes"
            | "YES"
            | "no"
            | "No"
            | "NO"
            | "on"
            | "On"
            | "ON"
            | "off"
            | "Off"
            | "OFF"
    ) || yaml_number(value)
        || yaml_timestamp(value)
}

fn yaml_number(value: &str) -> bool {
    let digits = value.strip_prefix(['+', '-']).unwrap_or(value);
    if digits.is_empty() {
        return false;
    }
    if let Some(hex) = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        return !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit() || b == b'_');
    }
    if let Some(oct) = digits
        .strip_prefix("0o")
        .or_else(|| digits.strip_prefix("0O"))
    {
        return !oct.is_empty() && oct.bytes().all(|b| matches!(b, b'0'..=b'7' | b'_'));
    }
    if let Some(bin) = digits.strip_prefix("0b") {
        return !bin.is_empty() && bin.bytes().all(|b| matches!(b, b'0' | b'1' | b'_'));
    }
    matches!(digits, ".inf" | ".Inf" | ".INF" | ".nan" | ".NaN" | ".NAN") || decimal_number(digits)
}

fn yaml_timestamp(value: &str) -> bool {
    let (date, rest) = value.split_at_checked(10).unwrap_or((value, ""));
    let bytes = date.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes[0..4].iter().all(u8::is_ascii_digit)
        || !bytes[5..7].iter().all(u8::is_ascii_digit)
        || !bytes[8..10].iter().all(u8::is_ascii_digit)
    {
        return false;
    }
    if rest.is_empty() {
        return true;
    }
    let time = rest.strip_prefix(['T', 't', ' ']).unwrap_or(rest);
    time_suffix(time)
}

fn time_suffix(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() < 8 || bytes[2] != b':' || bytes[5] != b':' {
        return false;
    }
    if !bytes[0..2].iter().all(u8::is_ascii_digit)
        || !bytes[3..5].iter().all(u8::is_ascii_digit)
        || !bytes[6..8].iter().all(u8::is_ascii_digit)
    {
        return false;
    }
    let mut rest = &value[8..];
    if let Some(frac) = rest.strip_prefix('.') {
        let digits = frac.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return false;
        }
        rest = &frac[digits..];
    }
    rest.is_empty()
        || rest == "Z"
        || rest == "z"
        || rest.strip_prefix(['+', '-']).is_some_and(|zone| {
            zone.len() == 5
                && zone.as_bytes()[2] == b':'
                && zone
                    .bytes()
                    .enumerate()
                    .all(|(i, b)| i == 2 || b.is_ascii_digit())
        })
}

fn decimal_number(value: &str) -> bool {
    let mut saw_digit = false;
    let mut saw_dot = false;
    let mut saw_exp = false;
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '0'..='9' => saw_digit = true,
            '_' if saw_digit => {}
            '.' if !saw_dot && !saw_exp => saw_dot = true,
            'e' | 'E' if saw_digit && !saw_exp => {
                saw_exp = true;
                if matches!(chars.peek(), Some('+' | '-')) {
                    chars.next();
                }
            }
            _ => return false,
        }
    }
    saw_digit
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
        assert!(yaml_has_catalog_table("catalog: {}\n").expect("empty catalog"));
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
    fn yaml_flow_leaf_is_not_a_string() {
        let src = "catalog:\n  core: { version: '^0.1.0' }\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("flow");
        assert_eq!(err, CatalogYamlError::Invalid);
    }

    #[test]
    fn yaml_flow_parent_is_not_a_string() {
        let src = "catalog: { core: '^0.1.0' }\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("flow");
        assert_eq!(err, yaml_not_string("catalog/core"));
    }

    #[test]
    fn yaml_block_scalar_is_not_a_string() {
        let src = "catalog:\n  core: |\n    ^0.1.0\n";
        let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err("block");
        assert_eq!(err, yaml_not_string("catalog/core"));
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
    fn yaml_unquoted_schema_string_is_rewritten() {
        for value in ["1", "true", "yes", "0B10", "2024-01-01"] {
            let src = format!("catalog:\n  core: {value}\n");
            let out = rewrite_catalog_yaml(&src, None, "core", "^0.2.0").expect(value);
            assert_eq!(out, format!("catalog:\n  core: ^0.2.0\n"), "{value}");
        }
    }

    #[test]
    fn yaml_unquoted_non_string_is_not_a_string() {
        for value in ["null", "Null", "~", "&pin '^0.1.0'"] {
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
    fn yaml_quoted_escape_is_not_a_string() {
        for src in [
            "catalog:\n  core: 'it''s'\n",
            "catalog:\n  core: \"a\\\"b\"\n",
        ] {
            let err = rewrite_catalog_yaml(src, None, "core", "^0.2.0").expect_err(src);
            assert_eq!(err, yaml_not_string("catalog/core"), "{src}");
        }
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
