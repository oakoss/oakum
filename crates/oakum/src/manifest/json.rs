//! `jsonc-parser` CST edits. `serde_json` pretty-print drops comments, CRLF,
//! and the trailing newline (okm-za4).
//!
//! Full jsonpath for release-please `extra-files` is still open. This helper
//! walks object keys and numeric array indexes, which covers `package.json`
//! `version` and `marketplace.json` `plugins[].version`.

use std::fmt;

use jsonc_parser::cst::{CstArray, CstInputValue, CstObject, CstObjectProp, CstRootNode};
use jsonc_parser::errors::ParseError;
use jsonc_parser::ParseOptions;

/// Replace the string at `path`, or append it when the last key is missing.
///
/// Intermediate segments must already exist. Numeric segments index arrays.
///
/// # Errors
///
/// Returns [`JsonEditError`] when the document is not an object, a path
/// segment is missing or the wrong kind, or the text is not JSONC.
pub fn set_json_string(text: &str, path: &[&str], next: &str) -> Result<String, JsonEditError> {
    write_json_string(text, path, next, true)
}

/// Like [`set_json_string`], but a missing last key is an error, not an insert.
pub(super) fn replace_json_string(
    text: &str,
    path: &[&str],
    next: &str,
) -> Result<String, JsonEditError> {
    write_json_string(text, path, next, false)
}

/// [`JsonEditError::Missing`] when the last key is absent.
pub(crate) fn json_string(text: &str, path: &[&str]) -> Result<String, JsonEditError> {
    let Some((last, parents)) = path.split_last() else {
        return Err(JsonEditError::EmptyPath);
    };
    let root = CstRootNode::parse(text, &jsonc_options())?;
    let Some(root_obj) = root.object_value() else {
        return Err(JsonEditError::RootNotObject);
    };
    let mut cursor = Cursor::Object(root_obj);
    let mut walked = String::new();
    for segment in parents {
        cursor = descend(cursor, segment, &walked)?;
        push_segment(&mut walked, segment);
    }
    let path = joined(&walked, last);
    match cursor {
        Cursor::Object(obj) => {
            let matches = named_props(&obj, last);
            match matches.as_slice() {
                [] => Err(JsonEditError::Missing { path }),
                [prop] => {
                    let Some(value) = prop.value() else {
                        return Err(JsonEditError::NotObject { path });
                    };
                    let Some(lit) = value.as_string_lit() else {
                        return Err(JsonEditError::NotObject { path });
                    };
                    lit.decoded_value()
                        .map_err(|_| JsonEditError::NotObject { path })
                }
                _ => Err(JsonEditError::Duplicate { path }),
            }
        }
        Cursor::Array(_) => Err(JsonEditError::NotObject { path: walked }),
    }
}

fn write_json_string(
    text: &str,
    path: &[&str],
    next: &str,
    create: bool,
) -> Result<String, JsonEditError> {
    let Some((last, parents)) = path.split_last() else {
        return Err(JsonEditError::EmptyPath);
    };
    let root = CstRootNode::parse(text, &jsonc_options())?;
    let Some(root_obj) = root.object_value() else {
        return Err(JsonEditError::RootNotObject);
    };
    let mut cursor = Cursor::Object(root_obj);
    let mut walked = String::new();
    for segment in parents {
        cursor = descend(cursor, segment, &walked)?;
        push_segment(&mut walked, segment);
    }
    match cursor {
        Cursor::Object(obj) => {
            let matches = named_props(&obj, last);
            match matches.as_slice() {
                [] if create => {
                    obj.append(last, CstInputValue::String(next.to_owned()));
                }
                [] => {
                    return Err(JsonEditError::Missing {
                        path: joined(&walked, last),
                    });
                }
                [prop] => prop.set_value(CstInputValue::String(next.to_owned())),
                _ => {
                    return Err(JsonEditError::Duplicate {
                        path: joined(&walked, last),
                    });
                }
            }
        }
        Cursor::Array(_) => {
            return Err(JsonEditError::NotObject {
                path: walked.clone(),
            });
        }
    }
    Ok(root.to_string())
}

enum Cursor {
    Object(CstObject),
    Array(CstArray),
}

fn descend(cursor: Cursor, segment: &str, walked: &str) -> Result<Cursor, JsonEditError> {
    let path = joined(walked, segment);
    match cursor {
        Cursor::Object(obj) => {
            let matches = named_props(&obj, segment);
            match matches.as_slice() {
                [] => Err(JsonEditError::Missing { path }),
                [prop] => {
                    let Some(value) = prop.value() else {
                        return Err(JsonEditError::NotObject { path });
                    };
                    if let Some(child) = value.as_object() {
                        return Ok(Cursor::Object(child));
                    }
                    if let Some(child) = value.as_array() {
                        return Ok(Cursor::Array(child));
                    }
                    Err(JsonEditError::NotObject { path })
                }
                _ => Err(JsonEditError::Duplicate { path }),
            }
        }
        Cursor::Array(arr) => {
            let index: usize = segment.parse().map_err(|_| JsonEditError::BadIndex {
                segment: segment.to_owned(),
            })?;
            let Some(elem) = arr.elements().into_iter().nth(index) else {
                return Err(JsonEditError::Missing { path });
            };
            if let Some(child) = elem.as_object() {
                return Ok(Cursor::Object(child));
            }
            if let Some(child) = elem.as_array() {
                return Ok(Cursor::Array(child));
            }
            Err(JsonEditError::NotObject { path })
        }
    }
}

fn push_segment(walked: &mut String, segment: &str) {
    if !walked.is_empty() {
        walked.push('/');
    }
    walked.push_str(segment);
}

/// JSONC: comments and trailing commas. JSON5 extras (unquoted keys, missing
/// commas, single quotes, hex, unary plus) stay off so a write cannot succeed
/// on text that is not package.json / marketplace.json.
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

fn named_props(obj: &CstObject, name: &str) -> Vec<CstObjectProp> {
    obj.properties()
        .into_iter()
        .filter(|prop| {
            prop.name()
                .and_then(|n| n.decoded_value().ok())
                .is_some_and(|decoded| decoded == name)
        })
        .collect()
}

fn joined(walked: &str, segment: &str) -> String {
    if walked.is_empty() {
        segment.to_owned()
    } else {
        format!("{walked}/{segment}")
    }
}

#[derive(Debug)]
pub enum JsonEditError {
    Parse(ParseError),
    EmptyPath,
    RootNotObject,
    Missing { path: String },
    NotObject { path: String },
    Duplicate { path: String },
    BadIndex { segment: String },
}

impl fmt::Display for JsonEditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(err) => write!(f, "JSONC parse failed: {err}"),
            Self::EmptyPath => f.write_str("JSON edit path must not be empty"),
            Self::RootNotObject => f.write_str("JSON document root must be an object"),
            Self::Missing { path } => write!(f, "JSON path `{path}` does not exist"),
            Self::NotObject { path } => {
                write!(f, "JSON path `{path}` is not an object")
            }
            Self::Duplicate { path } => {
                write!(f, "JSON path `{path}` has duplicate keys")
            }
            Self::BadIndex { segment } => {
                write!(f, "JSON array index `{segment}` is not a number")
            }
        }
    }
}

impl std::error::Error for JsonEditError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Parse(err) => Some(err),
            Self::EmptyPath
            | Self::RootNotObject
            | Self::Missing { .. }
            | Self::NotObject { .. }
            | Self::Duplicate { .. }
            | Self::BadIndex { .. } => None,
        }
    }
}

impl From<ParseError> for JsonEditError {
    fn from(err: ParseError) -> Self {
        Self::Parse(err)
    }
}

#[cfg(test)]
mod tests {
    use super::{replace_json_string, set_json_string, JsonEditError};

    #[test]
    fn helper_keeps_comments_and_trailing_newline() {
        let src = "{\n  // keep\n  \"name\": \"demo\",\n  \"version\": \"0.1.0\"\n}\n";
        let out = set_json_string(src, &["version"], "0.2.0").expect("edit");
        assert_eq!(
            out,
            "{\n  // keep\n  \"name\": \"demo\",\n  \"version\": \"0.2.0\"\n}\n"
        );
    }

    #[test]
    fn helper_keeps_crlf_and_trailing_newline() {
        let src = "{\r\n  \"version\": \"0.1.0\"\r\n}\r\n";
        let out = set_json_string(src, &["version"], "0.2.0").expect("edit");
        assert_eq!(out, "{\r\n  \"version\": \"0.2.0\"\r\n}\r\n");
    }

    #[test]
    fn helper_sets_nested_plugins_version() {
        let src = "{\n  \"plugins\": [\n    {\n      \"name\": \"demo\",\n      \"version\": \"0.1.0\"\n    }\n  ]\n}\n";
        let out = set_json_string(src, &["plugins", "0", "version"], "0.2.0").expect("edit");
        assert_eq!(
            out,
            "{\n  \"plugins\": [\n    {\n      \"name\": \"demo\",\n      \"version\": \"0.2.0\"\n    }\n  ]\n}\n"
        );
    }

    #[test]
    fn helper_inserts_a_missing_version_key() {
        let src = "{\n  \"name\": \"demo\"\n}\n";
        let out = set_json_string(src, &["version"], "0.1.0").expect("edit");
        assert_eq!(
            out,
            "{\n  \"name\": \"demo\",\n  \"version\": \"0.1.0\"\n}\n"
        );
    }

    #[test]
    fn replace_missing_last_key_is_an_error() {
        let err = replace_json_string("{\n  \"name\": \"demo\"\n}\n", &["version"], "0.1.0")
            .expect_err("missing");
        assert!(matches!(err, JsonEditError::Missing { path } if path == "version"));
    }

    #[test]
    fn missing_intermediate_path_is_an_error() {
        let err = set_json_string(
            "{\n  \"name\": \"demo\"\n}\n",
            &["plugins", "0", "version"],
            "0.2.0",
        )
        .expect_err("missing");
        assert!(matches!(err, JsonEditError::Missing { path } if path == "plugins"));
    }

    #[test]
    fn empty_path_is_an_error() {
        let err = set_json_string("{\n}\n", &[], "0.1.0").expect_err("empty");
        assert!(matches!(err, JsonEditError::EmptyPath));
    }

    #[test]
    fn invalid_json_is_a_parse_error() {
        let err = set_json_string("{", &["version"], "0.1.0").expect_err("parse");
        assert!(matches!(err, JsonEditError::Parse(_)));
    }

    #[test]
    fn scalar_intermediate_is_not_object() {
        let err = set_json_string(
            "{\n  \"plugins\": \"nope\"\n}\n",
            &["plugins", "0", "version"],
            "0.2.0",
        )
        .expect_err("kind");
        assert!(matches!(err, JsonEditError::NotObject { path } if path == "plugins"));
    }

    #[test]
    fn duplicate_last_key_is_an_error() {
        let err = set_json_string(
            "{\n  \"version\": \"0.1.0\",\n  \"version\": \"9.9.9\"\n}\n",
            &["version"],
            "0.2.0",
        )
        .expect_err("dup");
        assert!(matches!(err, JsonEditError::Duplicate { path } if path == "version"));
    }

    #[test]
    fn helper_keeps_trailing_comment_on_the_edited_key() {
        let src = "{\n  \"version\": \"0.1.0\" // keep\n}\n";
        let out = set_json_string(src, &["version"], "0.2.0").expect("edit");
        assert_eq!(out, "{\n  \"version\": \"0.2.0\" // keep\n}\n");
    }

    #[test]
    fn helper_sets_the_second_plugin_and_leaves_the_first() {
        let src = "{\n  \"plugins\": [\n    { \"version\": \"0.1.0\" },\n    { \"version\": \"0.1.0\" }\n  ]\n}\n";
        let out = set_json_string(src, &["plugins", "1", "version"], "0.2.0").expect("edit");
        assert_eq!(
            out,
            "{\n  \"plugins\": [\n    { \"version\": \"0.1.0\" },\n    { \"version\": \"0.2.0\" }\n  ]\n}\n"
        );
    }

    #[test]
    fn array_root_is_not_object() {
        let err = set_json_string("[]\n", &["version"], "0.1.0").expect_err("root");
        assert!(matches!(err, JsonEditError::RootNotObject));
    }

    #[test]
    fn helper_walks_a_nested_object_key() {
        let src = "{\n  \"package\": {\n    \"version\": \"0.1.0\"\n  }\n}\n";
        let out = set_json_string(src, &["package", "version"], "0.2.0").expect("edit");
        assert_eq!(
            out,
            "{\n  \"package\": {\n    \"version\": \"0.2.0\"\n  }\n}\n"
        );
    }

    #[test]
    fn out_of_range_array_index_is_missing() {
        let err = set_json_string(
            "{\n  \"plugins\": [{ \"version\": \"0.1.0\" }]\n}\n",
            &["plugins", "1", "version"],
            "0.2.0",
        )
        .expect_err("oob");
        assert!(matches!(err, JsonEditError::Missing { path } if path == "plugins/1"));
    }

    #[test]
    fn non_numeric_array_index_is_bad_index() {
        let err = set_json_string(
            "{\n  \"plugins\": [{ \"version\": \"0.1.0\" }]\n}\n",
            &["plugins", "x", "version"],
            "0.2.0",
        )
        .expect_err("idx");
        assert!(matches!(err, JsonEditError::BadIndex { segment } if segment == "x"));
    }

    #[test]
    fn scalar_array_element_is_not_object() {
        let err = set_json_string(
            "{\n  \"plugins\": [\"0.1.0\"]\n}\n",
            &["plugins", "0", "version"],
            "0.2.0",
        )
        .expect_err("elem");
        assert!(matches!(err, JsonEditError::NotObject { path } if path == "plugins/0"));
    }

    #[test]
    fn duplicate_intermediate_key_is_an_error() {
        let err = set_json_string(
            "{\n  \"plugins\": [{ \"version\": \"0.1.0\" }],\n  \"plugins\": [{ \"version\": \"9.9.9\" }]\n}\n",
            &["plugins", "0", "version"],
            "0.2.0",
        )
        .expect_err("dup");
        assert!(matches!(err, JsonEditError::Duplicate { path } if path == "plugins"));
    }

    #[test]
    fn last_segment_on_an_array_is_not_object() {
        let err = set_json_string(
            "{\n  \"plugins\": [{ \"version\": \"0.1.0\" }]\n}\n",
            &["plugins", "0"],
            "0.2.0",
        )
        .expect_err("arr");
        assert!(matches!(err, JsonEditError::NotObject { path } if path == "plugins"));
    }

    #[test]
    fn json5_missing_comma_is_a_parse_error() {
        let err = set_json_string(
            "{ \"name\": \"demo\" \"version\": \"0.1.0\" }\n",
            &["version"],
            "0.2.0",
        )
        .expect_err("json5");
        assert!(matches!(err, JsonEditError::Parse(_)));
    }
}
