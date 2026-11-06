//! Why a config failed to parse, with the line and column TOML points at.

use std::borrow::Cow;
use std::fmt;

use super::error_key::qualified_key;

#[derive(Debug)]
pub struct ParseError {
    pub(super) kind: ParseErrorKind,
    location: Option<(usize, usize)>,
}

impl ParseError {
    pub(super) fn new(kind: ParseErrorKind) -> Self {
        Self {
            kind,
            location: None,
        }
    }

    pub(super) fn at(kind: ParseErrorKind, text: &str, offset: usize) -> Self {
        Self {
            kind,
            location: Some(line_and_column(text, offset)),
        }
    }
}

#[derive(Debug)]
pub(super) enum ParseErrorKind {
    BothIntentMechanismsDisabled,
    DuplicateKey,
    InvalidSyntax,
    InvalidValue,
    InvalidExtraFile(String),
    EmptySelectionName,
    EmptyPublishCommand,
    UnknownSelectionName(String),
    TemplateDoesNotExecute,
    MissingField,
    UnknownKey(Option<String>),
    RemovalChangedOtherKeys,
}

impl ParseErrorKind {
    fn message(&self) -> Cow<'_, str> {
        match self {
            Self::BothIntentMechanismsDisabled => Cow::Borrowed(
                "both `change-files` and `conventional-commits` are disabled; enable one so the plan has intent to read (ADR-0019 / ADR-0029)",
            ),
            Self::DuplicateKey => Cow::Borrowed("duplicate configuration key"),
            Self::InvalidSyntax => Cow::Borrowed("invalid TOML syntax"),
            Self::InvalidValue => Cow::Borrowed("invalid configuration value"),
            Self::InvalidExtraFile(reason) => {
                Cow::Owned(format!("invalid extra-files entry: {reason}"))
            }
            Self::EmptySelectionName => {
                Cow::Borrowed(
                    "`include` / `exclude` entries must be non-empty package names with no leading or trailing whitespace",
                )
            }
            Self::EmptyPublishCommand => Cow::Borrowed("`publish-command` must not be empty"),
            Self::UnknownSelectionName(name) => {
                Cow::Owned(format!("unknown package name in include/exclude: {name}"))
            }
            Self::TemplateDoesNotExecute => {
                Cow::Borrowed("templates render; they do not execute (ADR-0006)")
            }
            Self::MissingField => Cow::Borrowed("a required key is missing"),
            Self::UnknownKey(None) => Cow::Borrowed("unknown configuration key"),
            Self::UnknownKey(Some(key)) => {
                Cow::Owned(format!("unknown configuration key `{key}`"))
            }
            Self::RemovalChangedOtherKeys => {
                Cow::Borrowed("removing `tool-version` would change other keys")
            }
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some((line, column)) = self.location {
            write!(
                f,
                "TOML parse error at line {line}, column {column}: {}",
                self.kind.message()
            )
        } else {
            f.write_str(&self.kind.message())
        }
    }
}

impl std::error::Error for ParseError {}

pub(super) fn structured_toml_error(text: &str, error: &toml::de::Error) -> ParseError {
    let offset = error.span().map(|span| span.start);
    let kind = match error.message() {
        message if message.starts_with("unknown field") => {
            ParseErrorKind::UnknownKey(offset.and_then(|offset| qualified_key(text, offset)))
        }
        message
            if message.starts_with("duplicate field") || message.starts_with("duplicate key") =>
        {
            ParseErrorKind::DuplicateKey
        }
        message if message.contains("do not execute") => ParseErrorKind::TemplateDoesNotExecute,
        // Syntactically perfect TOML that omits a required key was reported as
        // `invalid TOML syntax`, which sends a reader looking for a typo.
        message if message.starts_with("missing field") => ParseErrorKind::MissingField,
        message
            if message.starts_with("invalid type")
                || message.starts_with("invalid value")
                || message.starts_with("unknown variant")
                || message.contains("`file` is empty")
                || message.contains("template table needs")
                || message.contains("unknown template table key") =>
        {
            ParseErrorKind::InvalidValue
        }
        _ => ParseErrorKind::InvalidSyntax,
    };
    match offset {
        Some(offset) => ParseError::at(kind, text, offset),
        None => ParseError::new(kind),
    }
}

fn line_and_column(text: &str, offset: usize) -> (usize, usize) {
    let prefix = &text.as_bytes()[..offset.min(text.len())];
    let line = prefix.split(|byte| *byte == b'\n').count();
    let current_line = prefix
        .rsplit(|byte| *byte == b'\n')
        .next()
        .unwrap_or_default();
    let column = String::from_utf8_lossy(current_line).chars().count() + 1;
    (line, column)
}
