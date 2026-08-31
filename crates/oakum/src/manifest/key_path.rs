//! Declarative version-key paths for [`super::json`] edits (ADR-0033).
//!
//! Segments are dotted. A bare name is an object key; a digit-only segment is
//! an array index; `{field=value}` selects the unique array element whose
//! `field` string equals `value`.

use std::fmt;

use jsonc_parser::cst::{CstArray, CstObject, CstRootNode};
use jsonc_parser::ParseOptions;

use super::json::JsonEditError;

/// One segment of a declared `key`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeySegment {
    /// Object property name, or a numeric array index as a string of digits.
    Name(String),
    /// Unique array element where `field` is the string `value`.
    Match { field: String, value: String },
}

/// Parse `plugins.{name=review-cycle}.version` into segments.
///
/// # Errors
///
/// Empty input, empty segments, malformed `{field=value}`, or a trailing dot.
pub fn parse_key_path(key: &str) -> Result<Vec<KeySegment>, KeyPathError> {
    if key.is_empty() {
        return Err(KeyPathError::Empty);
    }
    let mut segments = Vec::new();
    let mut rest = key;
    while !rest.is_empty() {
        if rest.starts_with('.') {
            return Err(KeyPathError::EmptySegment);
        }
        if rest.starts_with('{') {
            let Some(end) = rest.find('}') else {
                return Err(KeyPathError::UnclosedMatch);
            };
            let inner = &rest[1..end];
            let Some((field, value)) = inner.split_once('=') else {
                return Err(KeyPathError::BadMatch {
                    text: inner.to_owned(),
                });
            };
            if field.is_empty() || value.is_empty() {
                return Err(KeyPathError::BadMatch {
                    text: inner.to_owned(),
                });
            }
            if field.contains('{')
                || field.contains('}')
                || value.contains('{')
                || value.contains('}')
            {
                return Err(KeyPathError::BadMatch {
                    text: inner.to_owned(),
                });
            }
            segments.push(KeySegment::Match {
                field: field.to_owned(),
                value: value.to_owned(),
            });
            rest = &rest[end + 1..];
            if rest.is_empty() {
                break;
            }
            if !rest.starts_with('.') {
                return Err(KeyPathError::ExpectedDot);
            }
            rest = &rest[1..];
            continue;
        }
        let (name, after) = match rest.find('.') {
            Some(i) => {
                let after = &rest[i + 1..];
                if after.is_empty() {
                    return Err(KeyPathError::EmptySegment);
                }
                (&rest[..i], after)
            }
            None => (rest, ""),
        };
        if name.is_empty() {
            return Err(KeyPathError::EmptySegment);
        }
        if name.contains('{') || name.contains('}') {
            return Err(KeyPathError::BadName {
                text: name.to_owned(),
            });
        }
        segments.push(KeySegment::Name(name.to_owned()));
        rest = after;
    }
    if segments.is_empty() {
        return Err(KeyPathError::Empty);
    }
    Ok(segments)
}

/// Parse a key that must resolve to a replaceable string leaf (ADR-0033).
///
/// # Errors
///
/// Same as [`parse_key_path`], plus [`KeyPathError::MatchAsLeaf`] when the last
/// segment is `{field=value}`.
pub fn parse_write_key_path(key: &str) -> Result<Vec<KeySegment>, KeyPathError> {
    let segments = parse_key_path(key)?;
    if matches!(segments.last(), Some(KeySegment::Match { .. })) {
        return Err(KeyPathError::MatchAsLeaf);
    }
    Ok(segments)
}

/// Resolve match segments against `text`, returning a path of bare names /
/// indexes suitable for [`super::json::replace_json_string`].
///
/// # Errors
///
/// Parse failures, missing paths, or zero/multiple match results.
pub fn resolve_json_key_path(text: &str, key: &str) -> Result<Vec<String>, KeyPathError> {
    let segments = parse_write_key_path(key)?;
    let root = CstRootNode::parse(text, &jsonc_options()).map_err(KeyPathError::Parse)?;
    let Some(root_obj) = root.object_value() else {
        return Err(KeyPathError::RootNotObject);
    };
    let mut cursor = Cursor::Object(root_obj);
    let mut resolved = Vec::new();
    let mut walked = String::new();
    let last = segments.len() - 1;
    for (i, segment) in segments.iter().enumerate() {
        let is_last = i == last;
        match segment {
            KeySegment::Name(name) => {
                if !is_last {
                    cursor = descend_name(cursor, name, &walked)?;
                    push_segment(&mut walked, name);
                }
                resolved.push(name.clone());
            }
            KeySegment::Match { field, value } => {
                if is_last {
                    return Err(KeyPathError::MatchAsLeaf);
                }
                let Cursor::Array(arr) = cursor else {
                    return Err(KeyPathError::MatchNotOnArray {
                        path: walked.clone(),
                    });
                };
                let index = match_index(&arr, field, value, &walked)?;
                let index_s = index.to_string();
                cursor = descend_name(Cursor::Array(arr), &index_s, &walked)?;
                push_segment(&mut walked, &index_s);
                resolved.push(index_s);
            }
        }
    }
    Ok(resolved)
}

/// Replace the string at `key` in `text` (ADR-0033). A missing final key is an error.
///
/// # Errors
///
/// When `key` does not parse, the document cannot be walked to a unique leaf,
/// or the leaf is not a replaceable string.
pub fn replace_json_at_key(text: &str, key: &str, next: &str) -> Result<String, KeyPathError> {
    let path = resolve_json_key_path(text, key)?;
    let refs: Vec<&str> = path.iter().map(String::as_str).collect();
    super::json::replace_json_string(text, &refs, next).map_err(KeyPathError::from)
}

enum Cursor {
    Object(CstObject),
    Array(CstArray),
}

fn descend_name(cursor: Cursor, segment: &str, walked: &str) -> Result<Cursor, KeyPathError> {
    let path = joined(walked, segment);
    match cursor {
        Cursor::Object(obj) => {
            let matches: Vec<_> = obj
                .properties()
                .into_iter()
                .filter(|prop| {
                    prop.name()
                        .and_then(|n| n.decoded_value().ok())
                        .is_some_and(|decoded| decoded == segment)
                })
                .collect();
            match matches.as_slice() {
                [] => Err(KeyPathError::Missing { path }),
                [prop] => {
                    let Some(value) = prop.value() else {
                        return Err(KeyPathError::NotObject { path });
                    };
                    if let Some(child) = value.as_object() {
                        return Ok(Cursor::Object(child));
                    }
                    if let Some(child) = value.as_array() {
                        return Ok(Cursor::Array(child));
                    }
                    Err(KeyPathError::NotObject { path })
                }
                _ => Err(KeyPathError::Duplicate { path }),
            }
        }
        Cursor::Array(arr) => {
            let index: usize = segment.parse().map_err(|_| KeyPathError::BadIndex {
                segment: segment.to_owned(),
            })?;
            let Some(elem) = arr.elements().into_iter().nth(index) else {
                return Err(KeyPathError::Missing { path });
            };
            if let Some(child) = elem.as_object() {
                return Ok(Cursor::Object(child));
            }
            if let Some(child) = elem.as_array() {
                return Ok(Cursor::Array(child));
            }
            Err(KeyPathError::NotObject { path })
        }
    }
}

fn match_index(
    arr: &CstArray,
    field: &str,
    value: &str,
    walked: &str,
) -> Result<usize, KeyPathError> {
    let path = if walked.is_empty() {
        format!("{{{field}={value}}}")
    } else {
        format!("{walked}/{{{field}={value}}}")
    };
    let mut hits = Vec::new();
    for (index, elem) in arr.elements().into_iter().enumerate() {
        let Some(obj) = elem.as_object() else {
            continue;
        };
        let named: Vec<_> = obj
            .properties()
            .into_iter()
            .filter(|prop| {
                prop.name()
                    .and_then(|n| n.decoded_value().ok())
                    .is_some_and(|decoded| decoded == field)
            })
            .collect();
        let prop = match named.as_slice() {
            [] => continue,
            [only] => only,
            _ => {
                return Err(KeyPathError::Duplicate {
                    path: format!("{path}[{index}]"),
                });
            }
        };
        let Some(value_node) = prop.value() else {
            continue;
        };
        let Some(lit) = value_node.as_string_lit() else {
            continue;
        };
        let Ok(decoded) = lit.decoded_value() else {
            continue;
        };
        if decoded == value {
            hits.push(index);
        }
    }
    match hits.as_slice() {
        [only] => Ok(*only),
        [] => Err(KeyPathError::NoMatch { path }),
        _ => Err(KeyPathError::AmbiguousMatch { path }),
    }
}

fn push_segment(walked: &mut String, segment: &str) {
    if !walked.is_empty() {
        walked.push('/');
    }
    walked.push_str(segment);
}

fn joined(walked: &str, segment: &str) -> String {
    if walked.is_empty() {
        segment.to_owned()
    } else {
        format!("{walked}/{segment}")
    }
}

fn jsonc_options() -> ParseOptions {
    ParseOptions {
        allow_comments: true,
        allow_trailing_commas: true,
        allow_loose_object_property_names: false,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
    }
}

#[derive(Debug)]
pub enum KeyPathError {
    Empty,
    EmptySegment,
    UnclosedMatch,
    ExpectedDot,
    BadMatch { text: String },
    BadName { text: String },
    Parse(jsonc_parser::errors::ParseError),
    RootNotObject,
    Missing { path: String },
    NotObject { path: String },
    Duplicate { path: String },
    BadIndex { segment: String },
    MatchNotOnArray { path: String },
    NoMatch { path: String },
    AmbiguousMatch { path: String },
    MatchAsLeaf,
    Json(JsonEditError),
}

impl From<JsonEditError> for KeyPathError {
    fn from(err: JsonEditError) -> Self {
        Self::Json(err)
    }
}

impl fmt::Display for KeyPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("extra-files key must not be empty"),
            Self::EmptySegment => f.write_str("extra-files key has an empty segment"),
            Self::UnclosedMatch => f.write_str("extra-files key has an unclosed `{field=value}`"),
            Self::ExpectedDot => f.write_str("extra-files key needs `.` after `}`"),
            Self::BadMatch { text } => {
                write!(f, "extra-files key has a bad match `{{{text}}}`")
            }
            Self::BadName { text } => write!(f, "extra-files key has a bad segment `{text}`"),
            Self::Parse(err) => write!(f, "JSONC parse failed: {err}"),
            Self::RootNotObject => f.write_str("JSON document root must be an object"),
            Self::Missing { path } => write!(f, "JSON path `{path}` does not exist"),
            Self::NotObject { path } => write!(f, "JSON path `{path}` is not an object or array"),
            Self::Duplicate { path } => write!(f, "JSON path `{path}` is duplicated"),
            Self::BadIndex { segment } => {
                write!(f, "JSON array index `{segment}` is not a number")
            }
            Self::MatchNotOnArray { path } => {
                write!(
                    f,
                    "extra-files match at `{path}` requires an array to select from"
                )
            }
            Self::NoMatch { path } => write!(f, "extra-files match `{path}` found no element"),
            Self::AmbiguousMatch { path } => {
                write!(f, "extra-files match `{path}` found more than one element")
            }
            Self::MatchAsLeaf => f.write_str("extra-files key cannot end with `{field=value}`"),
            Self::Json(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for KeyPathError {}

#[cfg(test)]
mod tests {
    use super::{
        parse_key_path, parse_write_key_path, replace_json_at_key, resolve_json_key_path,
        KeySegment,
    };

    #[test]
    fn parse_dotted_and_match() {
        assert_eq!(
            parse_key_path("version").expect("parse"),
            vec![KeySegment::Name(String::from("version"))]
        );
        assert_eq!(
            parse_key_path("plugins.{name=review-cycle}.version").expect("parse"),
            vec![
                KeySegment::Name(String::from("plugins")),
                KeySegment::Match {
                    field: String::from("name"),
                    value: String::from("review-cycle"),
                },
                KeySegment::Name(String::from("version")),
            ]
        );
    }

    #[test]
    fn resolve_match_to_index() {
        let text = r#"{
  "plugins": [
    { "name": "other", "version": "0.1.0" },
    { "name": "review-cycle", "version": "0.14.0" }
  ]
}
"#;
        assert_eq!(
            resolve_json_key_path(text, "plugins.{name=review-cycle}.version").expect("resolve"),
            vec![
                String::from("plugins"),
                String::from("1"),
                String::from("version")
            ]
        );
    }

    #[test]
    fn replace_matched_version() {
        let text = r#"{
  "plugins": [
    { "name": "other", "version": "0.1.0" },
    { "name": "review-cycle", "version": "0.14.0" }
  ]
}
"#;
        let out = replace_json_at_key(text, "plugins.{name=review-cycle}.version", "0.15.0")
            .expect("replace");
        assert!(out.contains("\"version\": \"0.15.0\""));
        assert!(out.contains("\"name\": \"other\""));
        assert!(out.contains("\"version\": \"0.1.0\""));
    }

    #[test]
    fn no_match_and_ambiguous_are_errors() {
        let missing = r#"{ "plugins": [ { "name": "other", "version": "0.1.0" } ] }"#;
        let err = resolve_json_key_path(missing, "plugins.{name=review-cycle}.version")
            .expect_err("none");
        assert!(err.to_string().contains("found no element"), "{err}");
        let dup = r#"{
  "plugins": [
    { "name": "review-cycle", "version": "0.1.0" },
    { "name": "review-cycle", "version": "0.2.0" }
  ]
}"#;
        let err =
            resolve_json_key_path(dup, "plugins.{name=review-cycle}.version").expect_err("dup");
        assert!(err.to_string().contains("more than one element"), "{err}");
    }

    #[test]
    fn trailing_dot_and_match_leaf_are_errors() {
        assert!(parse_key_path("version.").is_err());
        assert!(matches!(
            parse_write_key_path("plugins.{name=review-cycle}"),
            Err(super::KeyPathError::MatchAsLeaf)
        ));
    }

    #[test]
    fn non_string_match_field_is_skipped() {
        let text = r#"{
  "plugins": [
    { "name": 42, "version": "9.9.9" },
    { "name": "review-cycle", "version": "0.14.0" }
  ]
}"#;
        assert_eq!(
            resolve_json_key_path(text, "plugins.{name=review-cycle}.version").expect("resolve"),
            vec![
                String::from("plugins"),
                String::from("1"),
                String::from("version")
            ]
        );
        let only_number = r#"{ "plugins": [ { "name": 42, "version": "0.1.0" } ] }"#;
        let err =
            resolve_json_key_path(only_number, "plugins.{name=42}.version").expect_err("none");
        assert!(err.to_string().contains("found no element"), "{err}");
    }

    #[test]
    fn duplicate_match_field_on_one_element_is_an_error() {
        let text = r#"{
  "plugins": [
    { "name": "review-cycle", "name": "other", "version": "0.14.0" }
  ]
}"#;
        let err =
            resolve_json_key_path(text, "plugins.{name=review-cycle}.version").expect_err("dup");
        assert!(err.to_string().contains("duplicated"), "{err}");
    }

    #[test]
    fn match_on_object_is_an_error() {
        let text = r#"{ "plugins": { "name": "review-cycle", "version": "0.1.0" } }"#;
        let err =
            resolve_json_key_path(text, "plugins.{name=review-cycle}.version").expect_err("obj");
        assert!(err.to_string().contains("requires an array"), "{err}");
    }

    #[test]
    fn dotted_match_value_parses() {
        assert_eq!(
            parse_key_path("plugins.{name=foo.bar}.version").expect("parse"),
            vec![
                KeySegment::Name(String::from("plugins")),
                KeySegment::Match {
                    field: String::from("name"),
                    value: String::from("foo.bar"),
                },
                KeySegment::Name(String::from("version")),
            ]
        );
    }

    #[test]
    fn numeric_array_leaf_replaces_string() {
        let out = replace_json_at_key(
            "{\n  \"versions\": [\"0.1.0\", \"9.9.9\"]\n}\n",
            "versions.0",
            "0.2.0",
        )
        .expect("leaf");
        assert!(out.contains("\"0.2.0\""));
        assert!(out.contains("\"9.9.9\""));
    }
}
