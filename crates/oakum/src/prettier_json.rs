//! JSON that Prettier 3 leaves unchanged with its defaults, so a generated
//! file survives a repository's formatter (ADR-0031's goal for generated
//! markdown, applied to `_schema.json`). Objects are always expanded, which
//! Prettier preserves because the first key starts on its own line; arrays
//! follow its print-width and fill rules. That is a fixed point of the
//! formatter, not a reproduction of it: a short object Prettier would print
//! on one line from other input stays expanded here. `tests/prettier_oracle.rs`
//! runs the real formatter over the output as the gate.

use serde_json::Value;

/// `value` in the shape Prettier leaves alone (see the module doc), with one
/// trailing newline.
///
/// # Panics
///
/// Never: every `serde_json::Value` serializes.
#[must_use]
pub fn to_prettier_string(value: &Value) -> String {
    let mut body = String::new();
    render_json(value, 0, 0, 0, &mut body);
    body.push('\n');
    body
}

/// Prettier's default print width.
const JSON_PRINT_WIDTH: usize = 80;

fn render_json(value: &Value, indent: usize, used: usize, trailing: usize, out: &mut String) {
    match value {
        Value::Array(items) if items.is_empty() => out.push_str("[]"),
        Value::Object(map) if map.is_empty() => out.push_str("{}"),
        Value::Array(items) => {
            if let Some(inline) = inline_json_array(items) {
                if used + inline.len() + trailing <= JSON_PRINT_WIDTH {
                    out.push_str(&inline);
                    return;
                }
            }
            out.push_str("[\n");
            let inner = indent + 2;
            if items.iter().all(Value::is_number) {
                fill_json_numbers(items, inner, out);
            } else {
                for (i, item) in items.iter().enumerate() {
                    let comma = usize::from(i + 1 < items.len());
                    push_indent(out, inner);
                    render_json(item, inner, inner, comma, out);
                    out.push_str(if comma == 1 { ",\n" } else { "\n" });
                }
            }
            push_indent(out, indent);
            out.push(']');
        }
        Value::Object(map) => {
            out.push_str("{\n");
            let inner = indent + 2;
            for (i, (key, item)) in map.iter().enumerate() {
                let comma = usize::from(i + 1 < map.len());
                let key = serde_json::to_string(key).expect("string key serializes");
                push_indent(out, inner);
                out.push_str(&key);
                out.push_str(": ");
                render_json(item, inner, inner + key.len() + 2, comma, out);
                out.push_str(if comma == 1 { ",\n" } else { "\n" });
            }
            push_indent(out, indent);
            out.push('}');
        }
        scalar => out.push_str(&serde_json::to_string(scalar).expect("scalar serializes")),
    }
}

/// One-line form of an array, or `None` where Prettier would always break it:
/// two or more elements that are all multi-element arrays, or any element
/// that is a non-empty object (objects are printed expanded, and an expanded
/// child breaks its parent).
fn inline_json_array(items: &[Value]) -> Option<String> {
    let all_wide_arrays = items.len() > 1
        && items
            .iter()
            .all(|item| matches!(item, Value::Array(inner) if inner.len() > 1));
    if all_wide_arrays {
        return None;
    }
    let mut parts = Vec::with_capacity(items.len());
    for item in items {
        parts.push(match item {
            Value::Object(map) if map.is_empty() => String::from("{}"),
            Value::Object(_) => return None,
            Value::Array(inner) if inner.is_empty() => String::from("[]"),
            Value::Array(inner) => inline_json_array(inner)?,
            scalar => serde_json::to_string(scalar).expect("scalar serializes"),
        });
    }
    Some(format!("[{}]", parts.join(", ")))
}

/// Prettier packs a broken array of numbers as many per line as fit, unlike
/// strings, which go one per line. The schema has no numeric array, so the
/// oracle never reaches this branch; the unit test is its only gate.
fn fill_json_numbers(items: &[Value], indent: usize, out: &mut String) {
    let mut column = 0;
    for (i, item) in items.iter().enumerate() {
        let mut token = serde_json::to_string(item).expect("number serializes");
        if i + 1 < items.len() {
            token.push(',');
        }
        if column == 0 {
            push_indent(out, indent);
            column = indent;
        } else if column + 1 + token.len() > JSON_PRINT_WIDTH {
            out.push('\n');
            push_indent(out, indent);
            column = indent;
        } else {
            out.push(' ');
            column += 1;
        }
        out.push_str(&token);
        column += token.len();
    }
    out.push('\n');
}

fn push_indent(out: &mut String, width: usize) {
    out.extend(std::iter::repeat_n(' ', width));
}

#[cfg(test)]
mod tests {
    use super::render_json;
    use serde_json::{json, Value};

    #[test]
    fn render_json_expands_arrays_past_the_print_width() {
        let long: Vec<Value> = (0..12).map(|i| json!(format!("item-number-{i}"))).collect();
        let mut out = String::new();
        render_json(
            &json!({ "k": long, "nested": [[1, 2], {}] }),
            0,
            0,
            0,
            &mut out,
        );
        let expected = "{\n  \"k\": [\n    \"item-number-0\",";
        assert!(out.starts_with(expected), "{out}");
        assert!(out.contains("\"nested\": [[1, 2], {}]"), "{out}");
    }

    #[test]
    fn render_json_follows_prettier_for_container_arrays() {
        let mut out = String::new();
        render_json(
            &json!({ "a": [[1, 2], [3, 4]], "b": [{}, {}], "c": [{ "k": 1 }] }),
            0,
            0,
            0,
            &mut out,
        );
        assert!(
            out.contains("\"a\": [\n    [1, 2],\n    [3, 4]\n  ]"),
            "{out}"
        );
        assert!(out.contains("\"b\": [{}, {}]"), "{out}");
        assert!(
            out.contains("\"c\": [\n    {\n      \"k\": 1\n    }\n  ]"),
            "{out}"
        );
    }

    #[test]
    fn render_json_fills_numeric_arrays_like_prettier() {
        let numbers: Vec<Value> = (1000..1030).map(|n| json!(n)).collect();
        let mut out = String::new();
        render_json(&json!({ "k": numbers }), 0, 0, 0, &mut out);
        let expected = "{\n  \"k\": [\n    1000, 1001, 1002, 1003, 1004, 1005, 1006, 1007, 1008, 1009, 1010, 1011,\n    1012, 1013, 1014, 1015, 1016, 1017, 1018, 1019, 1020, 1021, 1022, 1023,\n    1024, 1025, 1026, 1027, 1028, 1029\n  ]\n}";
        assert_eq!(out, expected);
    }

    #[test]
    fn render_json_honors_the_print_width_boundary() {
        // `  "k": ["<68 a's>"],` is exactly 80 columns; one more breaks.
        for (len, inline) in [(68, true), (69, false)] {
            let mut out = String::new();
            render_json(
                &json!({ "k": ["a".repeat(len)], "z": 1 }),
                0,
                0,
                0,
                &mut out,
            );
            let line = out.lines().nth(1).expect("key line");
            assert_eq!(line.ends_with("\"],"), inline, "{len}: {out}");
        }
    }
}
