//! `toml_edit` is the right crate for Cargo.toml and still has one trap:
//! assigning through `Item` resets decor (trailing comments and padding).

use toml_edit::{value, Item, Value};

/// Assign `next` without dropping the item's trailing comment or padding.
///
/// `*item = value(v)` clones none of that trivia. Restore it here so every
/// call site is not its own copy of the trap (okm-299).
pub fn set_preserving_decor(item: &mut Item, next: impl Into<Value>) {
    let decor = item.as_value().map(Value::decor).cloned();
    *item = value(next);
    if let (Some(decor), Some(written)) = (decor, item.as_value_mut()) {
        *written.decor_mut() = decor;
    }
}

#[cfg(test)]
mod tests {
    use toml_edit::{value, DocumentMut};

    use super::set_preserving_decor;

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
}
