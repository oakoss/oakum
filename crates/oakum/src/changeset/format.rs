//! Parse and write the changeset-format intersection (ADR-0005).
//!
//! Pure string I/O for one bump file's body. Skip-list discovery, workspace
//! membership, and continue-on-malformed live in [`super::read`] (`okm-wnp`);
//! foreign-parser fixtures are `okm-x4u`.
//!
//! Safe to write: line 1 exactly `---`; one `name: patch|minor|major` per line;
//! unquoted keys except scoped npm names (quoted); no blank lines; no duplicate
//! keys; closing `---`; no preamble; no BOM. The note after the closing
//! delimiter is kept verbatim.

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use core::str::FromStr;

use crate::plan::BumpLevel;

/// One bump file after parsing the intersection grammar.
///
/// Package names are the frontmatter keys as written (quotes stripped). Mapping
/// onto [`crate::plan::PackageId`] needs a workspace and is a later step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeFile {
    entries: Vec<(String, BumpLevel)>,
    note: String,
}

impl ChangeFile {
    #[must_use]
    pub fn entries(&self) -> &[(String, BumpLevel)] {
        &self.entries
    }

    #[must_use]
    pub fn note(&self) -> &str {
        &self.note
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    Bom,
    MissingOpeningDelimiter,
    MissingClosingDelimiter,
    EmptyFrontmatter,
    BlankLineInFrontmatter,
    DuplicatePackage(String),
    InvalidLine(String),
    UnquotedScopedName(String),
    QuotedUnscopedName(String),
    UnknownLevel(String),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bom => f.write_str("bump file must not start with a UTF-8 BOM"),
            Self::MissingOpeningDelimiter => f.write_str("bump file must start with --- on line 1"),
            Self::MissingClosingDelimiter => f.write_str("bump file is missing the closing ---"),
            Self::EmptyFrontmatter => {
                f.write_str("bump file frontmatter must name at least one package")
            }
            Self::BlankLineInFrontmatter => {
                f.write_str("bump file frontmatter must not contain blank lines")
            }
            Self::DuplicatePackage(name) => {
                write!(f, "bump file names package `{name}` more than once")
            }
            Self::InvalidLine(line) => {
                write!(f, "bump file frontmatter line is not `name: level`: {line}")
            }
            Self::UnquotedScopedName(name) => write!(
                f,
                "scoped package `{name}` must be quoted (`\"{name}\": level`)"
            ),
            Self::QuotedUnscopedName(name) => write!(
                f,
                "package `{name}` must not be quoted (only scoped npm names are quoted)"
            ),
            Self::UnknownLevel(level) => {
                write!(f, "bump level `{level}` is not patch, minor, or major")
            }
        }
    }
}

impl core::error::Error for ParseError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteError {
    EmptyEntries,
    EmptyPackageName,
    DuplicatePackage(String),
    /// Name contains characters that cannot appear in an intersection key
    /// (quotes, colon, or line breaks).
    InvalidPackageName(String),
    /// Scoped npm name plus `knope.toml`: quoted form is invisible to knope
    /// (ADR-0005). Refuse rather than write a silent no-op for one reader.
    ScopedPackageWithKnope(String),
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyEntries => f.write_str("bump file must name at least one package"),
            Self::EmptyPackageName => f.write_str("bump file package name must not be empty"),
            Self::DuplicatePackage(name) => {
                write!(f, "bump file names package `{name}` more than once")
            }
            Self::InvalidPackageName(name) => {
                write!(f, "bump file package name `{name}` is not writable in the intersection grammar")
            }
            Self::ScopedPackageWithKnope(name) => write!(
                f,
                "refusing to write scoped package `{name}` while knope.toml is present (quoted keys are invisible to knope)"
            ),
        }
    }
}

impl core::error::Error for WriteError {}

/// Whether a `knope.toml` shares the repo (ADR-0005 scoped-quote refuse).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KnopePresence {
    Absent,
    Present,
}

/// # Errors
///
/// Returns [`ParseError`] when the text is outside the intersection grammar.
pub fn parse(text: &str) -> Result<ChangeFile, ParseError> {
    if text.starts_with('\u{FEFF}') || text.as_bytes().starts_with(&[0xEF, 0xBB, 0xBF]) {
        return Err(ParseError::Bom);
    }

    let mut lines = LineCursor::new(text);
    let Some((_, first)) = lines.next_line() else {
        return Err(ParseError::MissingOpeningDelimiter);
    };
    if first != "---" {
        return Err(ParseError::MissingOpeningDelimiter);
    }

    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();
    let mut closed_at = None;

    while let Some((_, line)) = lines.next_line() {
        if line == "---" {
            closed_at = Some(lines.position_after_current_line());
            break;
        }
        if line.trim().is_empty() {
            return Err(ParseError::BlankLineInFrontmatter);
        }
        let (name, level) = parse_entry(line)?;
        if !seen.insert(name.clone()) {
            return Err(ParseError::DuplicatePackage(name));
        }
        entries.push((name, level));
    }

    let Some(note_start) = closed_at else {
        return Err(ParseError::MissingClosingDelimiter);
    };
    if entries.is_empty() {
        return Err(ParseError::EmptyFrontmatter);
    }

    Ok(ChangeFile {
        entries,
        note: String::from(&text[note_start..]),
    })
}

/// Render a bump-file body in the intersection grammar (LF line endings).
///
/// Scoped names (`@scope/pkg`) are quoted. With [`KnopePresence::Present`], a
/// scoped name is refused instead: knope retains the quotes and skips the file.
///
/// # Errors
///
/// Returns [`WriteError`] for empty entries, illegal or duplicate names, or a
/// scoped package while knope is present.
pub fn write(
    entries: &[(impl AsRef<str>, BumpLevel)],
    note: &str,
    knope: KnopePresence,
) -> Result<String, WriteError> {
    if entries.is_empty() {
        return Err(WriteError::EmptyEntries);
    }

    let mut seen = BTreeSet::new();
    let mut out = String::from("---\n");
    for (name, level) in entries {
        let name = name.as_ref();
        validate_write_name(name)?;
        if !seen.insert(String::from(name)) {
            return Err(WriteError::DuplicatePackage(String::from(name)));
        }
        if is_scoped(name) {
            if knope == KnopePresence::Present {
                return Err(WriteError::ScopedPackageWithKnope(String::from(name)));
            }
            out.push('"');
            out.push_str(name);
            out.push('"');
        } else {
            out.push_str(name);
        }
        out.push_str(": ");
        out.push_str(&level.to_string());
        out.push('\n');
    }
    out.push_str("---\n");
    out.push_str(note);
    Ok(out)
}

fn validate_write_name(name: &str) -> Result<(), WriteError> {
    if name.trim().is_empty() {
        return Err(WriteError::EmptyPackageName);
    }
    if name != name.trim() || name.contains(['"', '\'', '\n', '\r', ':']) {
        return Err(WriteError::InvalidPackageName(String::from(name)));
    }
    Ok(())
}

fn is_scoped(name: &str) -> bool {
    name.starts_with('@')
}

fn parse_entry(line: &str) -> Result<(String, BumpLevel), ParseError> {
    let Some((raw_key, raw_level)) = line.split_once(':') else {
        return Err(ParseError::InvalidLine(String::from(line)));
    };
    // YAML mapping form; `core:minor` is a scalar to @changesets/parse.
    if !raw_level.starts_with([' ', '\t']) {
        return Err(ParseError::InvalidLine(String::from(line)));
    }
    let key = raw_key.trim();
    let level_text = raw_level.trim();
    if key.is_empty() || level_text.is_empty() {
        return Err(ParseError::InvalidLine(String::from(line)));
    }

    let (name, quoted) = decode_package_key(key, line)?;
    if name.is_empty() {
        return Err(ParseError::InvalidLine(String::from(line)));
    }

    if is_scoped(name) {
        if !quoted {
            return Err(ParseError::UnquotedScopedName(String::from(name)));
        }
    } else if quoted {
        return Err(ParseError::QuotedUnscopedName(String::from(name)));
    }

    let level = BumpLevel::from_str(level_text)
        .map_err(|err| ParseError::UnknownLevel(String::from(err.text())))?;
    Ok((String::from(name), level))
}

fn decode_package_key<'a>(key: &'a str, line: &str) -> Result<(&'a str, bool), ParseError> {
    if key.contains('\'') {
        return Err(ParseError::InvalidLine(String::from(line)));
    }
    if key.contains('"') {
        match strip_quotes(key) {
            Some(inner) if !inner.contains('"') => Ok((inner, true)),
            _ => Err(ParseError::InvalidLine(String::from(line))),
        }
    } else {
        Ok((key, false))
    }
}

fn strip_quotes(key: &str) -> Option<&str> {
    let bytes = key.as_bytes();
    if bytes.len() >= 2 && bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"' {
        Some(&key[1..key.len() - 1])
    } else {
        None
    }
}

/// Byte-oriented line walk so the note can be sliced from the source.
struct LineCursor<'a> {
    text: &'a str,
    pos: usize,
    line_end: usize,
}

impl<'a> LineCursor<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            pos: 0,
            line_end: 0,
        }
    }

    fn next_line(&mut self) -> Option<(usize, &'a str)> {
        if self.pos > self.text.len() {
            return None;
        }
        if self.pos == self.text.len() {
            // At EOF after the previous line — do not invent an empty line.
            return None;
        }

        let start = self.pos;
        let bytes = self.text.as_bytes();
        let mut i = start;
        while i < bytes.len() && bytes[i] != b'\n' {
            i += 1;
        }
        let mut content_end = i;
        if content_end > start && bytes[content_end - 1] == b'\r' {
            content_end -= 1;
        }
        let content = &self.text[start..content_end];
        self.line_end = if i < bytes.len() { i + 1 } else { bytes.len() };
        self.pos = self.line_end;
        Some((start, content))
    }

    fn position_after_current_line(&self) -> usize {
        self.line_end
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    #[test]
    fn round_trips_unscoped_intersection_file() {
        let body = "---\ncore: minor\nutils: patch\n---\n\nNotes here.\n";
        let parsed = parse(body).expect("parse");
        assert_eq!(
            parsed.entries(),
            &[
                ("core".to_string(), BumpLevel::Minor),
                ("utils".to_string(), BumpLevel::Patch),
            ]
        );
        assert_eq!(parsed.note(), "\nNotes here.\n");
        let written = write(parsed.entries(), parsed.note(), KnopePresence::Absent).expect("write");
        assert_eq!(written, body);
    }

    #[test]
    fn accepts_crlf_and_keeps_note_bytes() {
        let body = "---\r\ncore: major\r\n---\r\nbody\r\n";
        let parsed = parse(body).expect("parse");
        assert_eq!(parsed.entries(), &[("core".to_string(), BumpLevel::Major)]);
        assert_eq!(parsed.note(), "body\r\n");
    }

    #[test]
    fn writes_and_parses_quoted_scoped_name() {
        let written = write(
            &[("@oakum/core", BumpLevel::Minor)],
            "note\n",
            KnopePresence::Absent,
        )
        .expect("write");
        assert_eq!(written, "---\n\"@oakum/core\": minor\n---\nnote\n");
        let parsed = parse(&written).expect("parse");
        assert_eq!(
            parsed.entries(),
            &[("@oakum/core".to_string(), BumpLevel::Minor)]
        );
    }

    #[test]
    fn refuses_scoped_write_when_knope_present() {
        let err = write(
            &[("@oakum/core", BumpLevel::Patch)],
            "",
            KnopePresence::Present,
        )
        .unwrap_err();
        assert_eq!(
            err,
            WriteError::ScopedPackageWithKnope("@oakum/core".to_string())
        );
    }

    #[test]
    fn rejects_bom() {
        assert_eq!(
            parse("\u{FEFF}---\ncore: patch\n---\n"),
            Err(ParseError::Bom)
        );
    }

    #[test]
    fn rejects_preamble() {
        assert_eq!(
            parse("preamble\n---\ncore: patch\n---\n"),
            Err(ParseError::MissingOpeningDelimiter)
        );
    }

    #[test]
    fn rejects_blank_line_in_frontmatter() {
        assert_eq!(
            parse("---\ncore: patch\n\nutils: minor\n---\n"),
            Err(ParseError::BlankLineInFrontmatter)
        );
        assert_eq!(
            parse("---\ncore: patch\n   \nutils: minor\n---\n"),
            Err(ParseError::BlankLineInFrontmatter)
        );
    }

    #[test]
    fn rejects_duplicate_package() {
        assert_eq!(
            parse("---\ncore: patch\ncore: minor\n---\n"),
            Err(ParseError::DuplicatePackage("core".to_string()))
        );
    }

    #[test]
    fn rejects_empty_frontmatter() {
        assert_eq!(parse("---\n---\n"), Err(ParseError::EmptyFrontmatter));
    }

    #[test]
    fn rejects_none_and_unknown_levels() {
        assert_eq!(
            parse("---\ncore: none\n---\n"),
            Err(ParseError::UnknownLevel("none".to_string()))
        );
        assert_eq!(
            parse("---\ncore: Major\n---\n"),
            Err(ParseError::UnknownLevel("Major".to_string()))
        );
    }

    #[test]
    fn rejects_unquoted_scoped_and_quoted_unscoped() {
        assert_eq!(
            parse("---\n@oakum/core: minor\n---\n"),
            Err(ParseError::UnquotedScopedName("@oakum/core".to_string()))
        );
        assert_eq!(
            parse("---\n\"core\": minor\n---\n"),
            Err(ParseError::QuotedUnscopedName("core".to_string()))
        );
    }

    #[test]
    fn rejects_missing_closing_delimiter() {
        assert_eq!(
            parse("---\ncore: patch\n"),
            Err(ParseError::MissingClosingDelimiter)
        );
    }

    #[test]
    fn write_rejects_empty_entries() {
        let empty: Vec<(String, BumpLevel)> = vec![];
        assert_eq!(
            write(&empty, "", KnopePresence::Absent),
            Err(WriteError::EmptyEntries)
        );
    }

    #[test]
    fn write_rejects_empty_and_illegal_names() {
        assert_eq!(
            write(&[("", BumpLevel::Patch)], "", KnopePresence::Absent),
            Err(WriteError::EmptyPackageName)
        );
        assert_eq!(
            write(&[(" ", BumpLevel::Patch)], "", KnopePresence::Absent),
            Err(WriteError::EmptyPackageName)
        );
        assert_eq!(
            write(
                &[("core:extra", BumpLevel::Patch)],
                "",
                KnopePresence::Absent
            ),
            Err(WriteError::InvalidPackageName("core:extra".to_string()))
        );
        assert_eq!(
            write(&[("a\nb", BumpLevel::Patch)], "", KnopePresence::Absent),
            Err(WriteError::InvalidPackageName("a\nb".to_string()))
        );
        assert_eq!(
            write(
                &[("\"core\"", BumpLevel::Patch)],
                "",
                KnopePresence::Present
            ),
            Err(WriteError::InvalidPackageName("\"core\"".to_string()))
        );
    }

    #[test]
    fn write_rejects_duplicate_packages() {
        assert_eq!(
            write(
                &[("core", BumpLevel::Patch), ("core", BumpLevel::Minor)],
                "",
                KnopePresence::Absent,
            ),
            Err(WriteError::DuplicatePackage("core".to_string()))
        );
    }

    #[test]
    fn writes_unscoped_when_knope_present() {
        let written =
            write(&[("core", BumpLevel::Patch)], "n\n", KnopePresence::Present).expect("write");
        assert_eq!(written, "---\ncore: patch\n---\nn\n");
        assert_eq!(parse(&written).expect("parse").note(), "n\n");
    }

    #[test]
    fn rejects_invalid_frontmatter_lines() {
        assert_eq!(
            parse("---\ncore patch\n---\n"),
            Err(ParseError::InvalidLine("core patch".to_string()))
        );
        assert_eq!(
            parse("---\n: patch\n---\n"),
            Err(ParseError::InvalidLine(": patch".to_string()))
        );
        assert_eq!(
            parse("---\ncore:\n---\n"),
            Err(ParseError::InvalidLine("core:".to_string()))
        );
        assert_eq!(
            parse("---\ncore:minor\n---\n"),
            Err(ParseError::InvalidLine("core:minor".to_string()))
        );
        assert_eq!(
            parse("---\n\"\": patch\n---\n"),
            Err(ParseError::InvalidLine("\"\": patch".to_string()))
        );
    }

    #[test]
    fn rejects_malformed_quoting() {
        assert_eq!(
            parse("---\n\"core: patch\n---\n"),
            Err(ParseError::InvalidLine("\"core: patch".to_string()))
        );
        assert_eq!(
            parse("---\n'@oakum/core': minor\n---\n"),
            Err(ParseError::InvalidLine("'@oakum/core': minor".to_string()))
        );
        assert_eq!(
            parse("---\n\"@oakum/core: minor\n---\n"),
            Err(ParseError::InvalidLine("\"@oakum/core: minor".to_string()))
        );
    }

    #[test]
    fn accepts_whitespace_around_key_and_level() {
        let parsed = parse("---\n core : minor \n---\n").expect("parse");
        assert_eq!(parsed.entries(), &[("core".to_string(), BumpLevel::Minor)]);
    }

    #[test]
    fn round_trips_empty_note() {
        let body = "---\ncore: patch\n---\n";
        let parsed = parse(body).expect("parse");
        assert_eq!(parsed.note(), "");
        assert_eq!(
            write(parsed.entries(), "", KnopePresence::Absent).expect("write"),
            body
        );
    }
}
