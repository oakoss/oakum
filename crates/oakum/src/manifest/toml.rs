//! `toml_edit` is the right crate for Cargo.toml and still has two traps:
//! assigning through `Item` resets decor, and structural `to_string`
//! newlines are `\n` (multiline string interiors can keep `\r\n`).

use std::fmt;

use toml_edit::{value, DocumentMut, Item, Value};

/// Replace the string at `path`, restoring exclusive-CRLF originals.
///
/// Intermediate **standard** tables must already exist (not inline tables
/// or arrays of tables). Path segments are literal keys, not dotted
/// paths. A missing last key is inserted.
///
/// # Errors
///
/// Returns [`TomlEditError`] when the text is not TOML or a path segment
/// is missing or the wrong kind.
pub fn set_toml_string(text: &str, path: &[&str], next: &str) -> Result<String, TomlEditError> {
    let Some((last, parents)) = path.split_last() else {
        return Err(TomlEditError::EmptyPath);
    };
    if last.is_empty() || parents.iter().any(|segment| segment.is_empty()) {
        return Err(TomlEditError::EmptyPath);
    }
    let mut doc: DocumentMut = text.parse().map_err(TomlEditError::Parse)?;
    let mut cursor = doc.as_item_mut();
    let mut landed_on: Option<&str> = None;
    for segment in parents {
        if !cursor.is_table() {
            return Err(TomlEditError::NotTable {
                path: landed_on.unwrap_or(segment).to_string(),
            });
        }
        cursor = table_child(cursor, segment)?;
        landed_on = Some(segment);
    }
    if !cursor.is_table() {
        return Err(TomlEditError::NotTable {
            path: landed_on.unwrap_or(last).to_string(),
        });
    }
    if let Some(existing) = existing_key(cursor, last) {
        if existing.as_str().is_none() {
            return Err(TomlEditError::NotString {
                path: (*last).to_owned(),
            });
        }
    }
    set_preserving_decor(&mut cursor[last], next);
    Ok(emit_toml(&doc, text))
}

/// True for `version.workspace = true` or `version = { workspace = true }`.
///
/// # Errors
///
/// Returns [`TomlEditError::Parse`] when the text is not TOML.
pub fn cargo_package_version_inherits_workspace(text: &str) -> Result<bool, TomlEditError> {
    let doc: DocumentMut = text.parse().map_err(TomlEditError::Parse)?;
    let Some(version) = doc
        .get("package")
        .and_then(|package| package.get("version"))
    else {
        return Ok(false);
    };
    if version.as_str().is_some() {
        return Ok(false);
    }
    Ok(version.get("workspace").and_then(Item::as_bool) == Some(true))
}

/// `Table::get_mut` returns `None` for a missing key. `Item::get_mut`
/// inserts `Item::None` and looks occupied.
fn table_child<'a>(parent: &'a mut Item, segment: &str) -> Result<&'a mut Item, TomlEditError> {
    parent
        .as_table_mut()
        .and_then(|table| table.get_mut(segment))
        .ok_or_else(|| TomlEditError::Missing {
            path: segment.to_owned(),
        })
}

fn existing_key<'a>(parent: &'a Item, key: &str) -> Option<&'a Item> {
    parent.as_table().and_then(|table| table.get(key))
}

/// Assign `next` without dropping the item's trailing comment or padding.
///
/// `*item = value(v)` clones none of that trivia. Restore it here so every
/// call site is not its own copy of the trap (okm-299).
pub(crate) fn set_preserving_decor(item: &mut Item, next: impl Into<Value>) {
    let decor = item.as_value().map(Value::decor).cloned();
    *item = value(next);
    if let (Some(decor), Some(written)) = (decor, item.as_value_mut()) {
        *written.decor_mut() = decor;
    }
}

/// Serialize `doc`, restoring CRLF only when every original newline was CRLF.
///
/// `toml_edit` emits `\n` for structural newlines; multiline string
/// interiors can still hold `\r\n`. Without this, rewriting a CRLF
/// Cargo.toml turns the file LF. Convert only lone `\n` so an existing
/// `\r\n` in a string body cannot become `\r\r\n`.
pub(crate) fn emit_toml(doc: &DocumentMut, original: &str) -> String {
    let mut out = doc.to_string();
    if uses_only_crlf(original) {
        out = restore_lone_lf(&out);
    }
    if !original.ends_with('\n') {
        out = strip_trailing_newline(&out);
    }
    out
}

fn strip_trailing_newline(text: &str) -> String {
    text.strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text)
        .to_owned()
}

fn restore_lone_lf(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\n' && bytes.get(i.wrapping_sub(1)) != Some(&b'\r') {
            out.push_str(&text[start..i]);
            out.push_str("\r\n");
            i += 1;
            start = i;
        } else {
            i += 1;
        }
    }
    out.push_str(&text[start..]);
    out
}

fn uses_only_crlf(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut saw_crlf = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' if bytes.get(i + 1) == Some(&b'\n') => {
                saw_crlf = true;
                i += 2;
            }
            b'\n' => return false,
            _ => i += 1,
        }
    }
    saw_crlf
}

/// Failures from [`set_toml_string`].
#[derive(Debug)]
pub enum TomlEditError {
    Parse(toml_edit::TomlError),
    EmptyPath,
    Missing { path: String },
    NotTable { path: String },
    NotString { path: String },
}

impl fmt::Display for TomlEditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(err) => write!(f, "{err}"),
            Self::EmptyPath => {
                f.write_str("TOML edit path must not be empty or contain an empty segment")
            }
            Self::Missing { path } => write!(f, "TOML path `{path}` does not exist"),
            Self::NotTable { path } => {
                write!(f, "TOML path `{path}` is not a standard table")
            }
            Self::NotString { path } => write!(f, "TOML path `{path}` is not a string"),
        }
    }
}

impl std::error::Error for TomlEditError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Parse(err) => Some(err),
            Self::EmptyPath
            | Self::Missing { .. }
            | Self::NotTable { .. }
            | Self::NotString { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use toml_edit::{value, DocumentMut};

    use super::{
        cargo_package_version_inherits_workspace, emit_toml, set_preserving_decor, set_toml_string,
        uses_only_crlf, TomlEditError,
    };

    fn parse(src: &str) -> DocumentMut {
        src.parse()
            .unwrap_or_else(|err| panic!("document should parse: {err}"))
    }

    #[test]
    fn naive_assignment_drops_trailing_comment_and_padding() {
        let mut doc = parse("version = \"0.1.0\"   # keep me\n");
        doc["version"] = value("0.2.0");
        assert_eq!(doc.to_string(), "version = \"0.2.0\"\n");
    }

    #[test]
    fn helper_keeps_trailing_comment_and_padding() {
        let mut doc = parse("version = \"0.1.0\"   # keep me\n");
        set_preserving_decor(&mut doc["version"], "0.2.0");
        assert_eq!(doc.to_string(), "version = \"0.2.0\"   # keep me\n");
    }

    #[test]
    fn helper_keeps_padding_between_equals_and_value() {
        let mut doc = parse("version =    \"0.1.0\"\n");
        set_preserving_decor(&mut doc["version"], "0.2.0");
        assert_eq!(doc.to_string(), "version =    \"0.2.0\"\n");
    }

    #[test]
    fn helper_leaves_neighbor_keys_untouched() {
        let src = "\
[package]
name = \"demo\"  # name stays
version = \"0.1.0\"   # keep me
edition = \"2021\"
";
        let mut doc = parse(src);
        set_preserving_decor(&mut doc["package"]["version"], "0.2.0");
        assert_eq!(
            doc.to_string(),
            "\
[package]
name = \"demo\"  # name stays
version = \"0.2.0\"   # keep me
edition = \"2021\"
"
        );
    }

    #[test]
    fn helper_inserts_a_missing_key_with_default_spacing() {
        let mut doc = parse("[package]\nname = \"demo\"\n");
        set_preserving_decor(&mut doc["package"]["version"], "0.1.0");
        assert_eq!(
            doc.to_string(),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n"
        );
    }

    #[test]
    fn emit_toml_restores_crlf_and_leaves_lf_alone() {
        let crlf = "[package]\r\nversion = \"0.1.0\"\r\n";
        let mut doc = parse(crlf);
        set_preserving_decor(&mut doc["package"]["version"], "0.2.0");
        assert_eq!(
            emit_toml(&doc, crlf),
            "[package]\r\nversion = \"0.2.0\"\r\n"
        );

        let lf = "[package]\nversion = \"0.1.0\"\n";
        let mut doc = parse(lf);
        set_preserving_decor(&mut doc["package"]["version"], "0.2.0");
        assert_eq!(emit_toml(&doc, lf), "[package]\nversion = \"0.2.0\"\n");
    }

    #[test]
    fn emit_toml_keeps_a_missing_final_newline() {
        let lf = "[package]\nversion = \"0.1.0\"";
        let mut doc = parse(lf);
        set_preserving_decor(&mut doc["package"]["version"], "0.2.0");
        assert_eq!(emit_toml(&doc, lf), "[package]\nversion = \"0.2.0\"");

        let crlf = "[package]\r\nversion = \"0.1.0\"";
        let mut doc = parse(crlf);
        set_preserving_decor(&mut doc["package"]["version"], "0.2.0");
        assert_eq!(emit_toml(&doc, crlf), "[package]\r\nversion = \"0.2.0\"");
    }

    #[test]
    fn emit_toml_does_not_promote_mixed_endings() {
        let mixed = "[package]\nversion = \"0.1.0\"\r\nname = \"demo\"\n";
        let mut doc = parse(mixed);
        set_preserving_decor(&mut doc["package"]["version"], "0.2.0");
        let out = emit_toml(&doc, mixed);
        assert!(!out.contains("\r\n"), "{out:?}");
        assert_eq!(out, "[package]\nversion = \"0.2.0\"\nname = \"demo\"\n");
    }

    #[test]
    fn emit_toml_restores_crlf_around_a_multiline_string_body() {
        let src = "[package]\r\nversion = \"0.1.0\"\r\ndescription = \"\"\"\r\nkeep\r\nthis\r\n\"\"\"\r\n";
        let mut doc = parse(src);
        set_preserving_decor(&mut doc["package"]["version"], "0.2.0");
        let out = emit_toml(&doc, src);
        assert!(uses_only_crlf(&out), "expected exclusive CRLF, got {out:?}");
        assert!(out.contains("version = \"0.2.0\""));
        assert!(out.contains("keep\r\nthis"));
        assert!(!out.contains("\r\r\n"), "{out:?}");
    }

    #[test]
    fn emit_toml_keeps_utf8_when_restoring_crlf() {
        let src = "[package]\r\n# café\r\nversion = \"0.1.0\"\r\n";
        let mut doc = parse(src);
        set_preserving_decor(&mut doc["package"]["version"], "0.2.0");
        assert_eq!(
            emit_toml(&doc, src),
            "[package]\r\n# café\r\nversion = \"0.2.0\"\r\n"
        );
    }

    #[test]
    fn set_toml_string_empty_path_is_an_error() {
        let err =
            set_toml_string("[package]\nversion = \"0.1.0\"\n", &[], "0.2.0").expect_err("empty");
        assert!(matches!(err, TomlEditError::EmptyPath));
    }

    #[test]
    fn set_toml_string_empty_segment_is_an_error() {
        let src = "[package]\nversion = \"0.1.0\"\n";
        let last = set_toml_string(src, &["package", ""], "0.2.0").expect_err("empty last");
        assert!(matches!(last, TomlEditError::EmptyPath), "{last:?}");
        let mid = set_toml_string(src, &["", "version"], "0.2.0").expect_err("empty mid");
        assert!(matches!(mid, TomlEditError::EmptyPath), "{mid:?}");
    }

    #[test]
    fn set_toml_string_missing_table_is_an_error() {
        let err = set_toml_string("name = \"demo\"\n", &["package", "version"], "0.2.0")
            .expect_err("missing");
        assert!(
            matches!(err, TomlEditError::Missing { ref path } if path == "package"),
            "{err:?}"
        );
    }

    #[test]
    fn set_toml_string_scalar_parent_is_an_error() {
        let err = set_toml_string("name = \"demo\"\n", &["name", "version"], "0.2.0")
            .expect_err("scalar");
        assert!(
            matches!(err, TomlEditError::NotTable { ref path } if path == "name"),
            "{err:?}"
        );
    }

    #[test]
    fn set_toml_string_nested_scalar_parent_names_the_scalar() {
        let err = set_toml_string("name = \"demo\"\n", &["name", "sub", "version"], "0.2.0")
            .expect_err("nested scalar");
        assert!(
            matches!(err, TomlEditError::NotTable { ref path } if path == "name"),
            "{err:?}"
        );
    }

    #[test]
    fn set_toml_string_invalid_toml_is_a_parse_error() {
        let err = set_toml_string("this is not toml {", &["package", "version"], "0.2.0")
            .expect_err("parse");
        assert!(matches!(err, TomlEditError::Parse(_)), "{err:?}");
    }

    #[test]
    fn cargo_package_version_inherits_workspace_dotted_and_inline() {
        assert!(cargo_package_version_inherits_workspace(
            "[package]\nname = \"demo\"\nversion.workspace = true\n"
        )
        .unwrap());
        assert!(cargo_package_version_inherits_workspace(
            "[package]\nname = \"demo\"\nversion = { workspace = true }\n"
        )
        .unwrap());
        assert!(!cargo_package_version_inherits_workspace(
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n"
        )
        .unwrap());
        assert!(!cargo_package_version_inherits_workspace("[package]\nname = \"demo\"\n").unwrap());
    }

    #[test]
    fn set_toml_string_table_last_key_is_an_error() {
        let err = set_toml_string("[package]\nversion = \"0.1.0\"\n", &["package"], "0.2.0")
            .expect_err("table");
        assert!(
            matches!(err, TomlEditError::NotString { ref path } if path == "package"),
            "{err:?}"
        );
    }
}
