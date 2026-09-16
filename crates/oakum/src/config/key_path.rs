//! Byte offset to dotted TOML key path, for naming the key serde refused.
//!
//! Pure, and ignorant of oakum's config shape: the tests below cover offsets
//! serde never reports as well as the ones it does.

/// The key a reader has to find, spelled the way TOML resolved it rather than
/// the way it was typed. A scalar written below a table header is scoped into
/// that table, so the line the error names and the key the parser saw can sit
/// paragraphs apart — measured: a top-level-looking `commit-message` read as
/// `private-packages.commit-message`, and the line number alone made that a
/// puzzle.
///
/// Resolved by a span-preserving parse rather than by scanning the source. The
/// scan this replaced got four classes of document wrong, each of them by
/// naming a key that is valid: a header carrying a trailing comment was not
/// recognised as a header, so the key was attributed to the table above it; a
/// key inside an inline table was reported as the inline table's own name; a
/// bracketed line inside a multi-line string was read as a header; and a
/// quoted or spaced header was printed as typed rather than as resolved.
/// `None` keeps the unqualified message.
pub(super) fn qualified_key(text: &str, offset: usize) -> Option<String> {
    let document: toml_edit::Document<String> = text.parse().ok()?;
    let mut path = Vec::new();
    named_at(document.as_table(), offset, &mut path).then(|| path.join("."))
}

/// One path segment, quoted exactly when TOML requires it. Pushing the decoded
/// name printed `packages.lodash.merge.nope` for a package genuinely named
/// `lodash.merge` — a path the document does not hold and nobody can grep for,
/// and dotted or scoped package names make that shape common.
fn segment(key: &toml_edit::Key) -> String {
    key.default_repr()
        .as_raw()
        .as_str()
        .map_or_else(|| key.get().to_owned(), ToOwned::to_owned)
}

/// Depth-first for the key whose own span covers `offset`, recording the path
/// walked to reach it. Every segment comes from the parser, spelled by
/// [`segment`].
fn named_at(table: &toml_edit::Table, offset: usize, path: &mut Vec<String>) -> bool {
    for (name, item) in table {
        let Some(key) = table.key(name) else { continue };
        path.push(segment(key));
        if key.span().is_some_and(|span| span.contains(&offset))
            || named_in_item(item, offset, path)
        {
            return true;
        }
        path.pop();
    }
    false
}

/// Inline tables nest, so this recurses the same way [`named_at`] does: a
/// `packages = { core = { publish = true } }` puts the offending key two levels
/// inside one line.
fn named_in_inline(inline: &toml_edit::InlineTable, offset: usize, path: &mut Vec<String>) -> bool {
    for (name, value) in inline {
        let Some(key) = inline.key(name) else {
            continue;
        };
        path.push(segment(key));
        let found = key.span().is_some_and(|span| span.contains(&offset))
            || value
                .as_inline_table()
                .is_some_and(|nested| named_in_inline(nested, offset, path))
            || value.as_array().is_some_and(|array| {
                array.iter().any(|element| {
                    element
                        .as_inline_table()
                        .is_some_and(|nested| named_in_inline(nested, offset, path))
                })
            });
        if found {
            return true;
        }
        path.pop();
    }
    false
}

fn named_in_item(item: &toml_edit::Item, offset: usize, path: &mut Vec<String>) -> bool {
    if let Some(child) = item.as_table() {
        return named_at(child, offset, path);
    }
    if let Some(inline) = item.as_inline_table() {
        return named_in_inline(inline, offset, path);
    }
    if let Some(array) = item.as_array_of_tables() {
        return array.iter().any(|child| named_at(child, offset, path));
    }
    // `extra-files = [{ path = "a.json", … }]` is the same data as
    // `[[…extra-files]]` and an accepted spelling; without this arm only the
    // header form kept its qualified name.
    if let Some(array) = item.as_array() {
        return array.iter().any(|value| {
            value
                .as_inline_table()
                .is_some_and(|nested| named_in_inline(nested, offset, path))
        });
    }
    false
}

#[cfg(test)]
mod tests {
    use super::qualified_key;

    /// Direct walker cases as `(document, offset, text under the offset,
    /// expected path)`. The third column is asserted against the document so a
    /// miscounted offset fails naming itself instead of testing a stray byte.
    ///
    /// The `parse`-level tests in [`super::super::tests`] pin what a reader sees
    /// for documents serde rejects; every offset there is one serde chose.
    /// This table reaches the walker with offsets serde never reports (inside
    /// a value, mid-key, end of document, a deeper dotted segment) and pins in
    /// one row each the two classes a document sweep once had to find: an
    /// unknown key in the first of two `[[x]]` elements, and a quoted segment
    /// inside an inline table.
    #[test]
    fn offsets_resolve_to_the_path_toml_resolved() {
        let cases: &[(&str, usize, &str, Option<&str>)] = &[
            // Top-level key.
            ("a = 1\nnope = 2\n", 6, "nope", Some("nope")),
            // The span covers the whole key, not only its first byte.
            ("a = 1\nnope = 2\n", 8, "pe", Some("nope")),
            // Nested table header scopes the key.
            ("[a.b]\nnope = 1\n", 6, "nope", Some("a.b.nope")),
            // Dotted key: the last segment names the full path; the first,
            // which serde refuses, names only itself.
            ("a.b.nope = 1\n", 4, "nope", Some("a.b.nope")),
            ("a.b.nope = 1\n", 0, "a", Some("a")),
            // Array of tables: either element, including the first of two.
            (
                "[[x]]\nnope = 1\n\n[[x]]\nnope = 2\n",
                6,
                "nope",
                Some("x.nope"),
            ),
            (
                "[[x]]\nnope = 1\n\n[[x]]\nnope = 2\n",
                22,
                "nope",
                Some("x.nope"),
            ),
            // Inline table members, at any depth.
            ("t = { a = 1, nope = 2 }\n", 13, "nope", Some("t.nope")),
            ("t = { u = { nope = 1 } }\n", 12, "nope", Some("t.u.nope")),
            // Inline array of tables, at a table's top level and inside an
            // inline table.
            ("t = [{ nope = 1 }]\n", 7, "nope", Some("t.nope")),
            ("t = { u = [{ nope = 1 }] }\n", 13, "nope", Some("t.u.nope")),
            // Quoted segments keep their quotes, in a header and inside an
            // inline table: `t.a.b.nope` names two levels the document lacks.
            (
                "[t.\"a.b\"]\nnope = 1\n",
                10,
                "nope",
                Some("t.\"a.b\".nope"),
            ),
            (
                "t = { \"a.b\" = { nope = 1 } }\n",
                16,
                "nope",
                Some("t.\"a.b\".nope"),
            ),
            (
                "[t.\"@s/p\"]\nnope = 1\n",
                11,
                "nope",
                Some("t.\"@s/p\".nope"),
            ),
            // Quotes that TOML does not require are dropped.
            ("[t.\"a\"]\nnope = 1\n", 8, "nope", Some("t.a.nope")),
            // A table header's own key.
            ("[t]\nnope = 1\n", 1, "t", Some("t")),
            // Inside a value, not a key.
            ("a = 1\n", 4, "1", None),
            ("t = { a = 1 }\n", 10, "1", None),
            // End of document, and past it.
            ("a = 1\n", 6, "", None),
            ("a = 1\n", 100, "", None),
            // Not TOML: no path rather than a guess.
            ("a = \n", 0, "a", None),
        ];
        let mut mismatches = Vec::new();
        for (document, offset, under, expected) in cases {
            assert!(
                document.get(*offset..).unwrap_or("").starts_with(under),
                "offset {offset} of {document:?} is not on {under:?}"
            );
            let actual = qualified_key(document, *offset);
            if actual.as_deref() != *expected {
                mismatches.push(format!(
                    "offset {offset} ({under:?}) of {document:?}: got {actual:?}, expected {expected:?}"
                ));
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    }
}
