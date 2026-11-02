//! Locates one single-line string scalar in YAML by key path, with the byte
//! span a rewrite replaces, so comments and layout around it stay.

use std::collections::BTreeMap;

use saphyr_parser::{Event, Parser, ScalarStyle, Span as YamlSpan, SpannedEventReceiver};

use super::catalog::CatalogYamlError;

pub(super) struct Span {
    pub(super) start: usize,
    pub(super) end: usize,
}

/// The rewrite span of a single-line string at `path`, and its decoded value.
pub(super) fn find_yaml_scalar(
    text: &str,
    path: &[&str],
    schema_is_string: bool,
) -> Result<(Span, String), CatalogYamlError> {
    locate_yaml_scalar(text, path, schema_is_string).map(|found| (found.span, found.decoded))
}

/// Replaces each span with its text; spans must not overlap.
pub(super) fn apply_edits(text: &str, mut edits: Vec<(Span, String)>) -> String {
    edits.sort_by_key(|(span, _)| core::cmp::Reverse(span.start));
    let mut out = text.to_owned();
    for (span, value) in edits {
        out.replace_range(span.start..span.end, &value);
    }
    out
}

/// `span` is in the anchored scalar when the entry is the alias `alias`;
/// `uses` are the aliases of an anchor the entry carries.
pub(super) struct Located {
    pub(super) span: Span,
    pub(super) decoded: String,
    pub(super) alias: Option<Span>,
    pub(super) uses: Vec<Span>,
}

pub(super) fn locate_yaml_scalar(
    text: &str,
    path: &[&str],
    schema_is_string: bool,
) -> Result<Located, CatalogYamlError> {
    if path.is_empty() {
        return Err(CatalogYamlError::missing(path));
    }
    let mut finder = PathFinder {
        text,
        path,
        keys: Vec::new(),
        pending: None,
        expect_key: false,
        skip: 0,
        mapping_depth: 0,
        found: None,
        err: None,
        anchors: BTreeMap::new(),
        aliases: BTreeMap::new(),
    };
    if Parser::new_from_str(text).load(&mut finder, false).is_err() {
        return Err(finder.err.unwrap_or(CatalogYamlError::Invalid));
    }
    if let Some(err) = finder.err {
        return Err(err);
    }
    let Some(found) = finder.found else {
        return Err(if schema_is_string {
            CatalogYamlError::not_string(path)
        } else {
            CatalogYamlError::missing(path)
        });
    };
    if !schema_is_string {
        return Err(CatalogYamlError::not_string(path));
    }
    let start = char_index_to_byte(text, found.start)?;
    let end = char_index_to_byte(text, found.end)?;
    let span = scalar_rewrite_span(text, start, end, found.style, &found.decoded, path)?;
    let byte_span = |(start, end): (usize, usize)| -> Result<Span, CatalogYamlError> {
        Ok(Span {
            start: char_index_to_byte(text, start)?,
            end: char_index_to_byte(text, end)?,
        })
    };
    let alias = found.alias.map(byte_span).transpose()?;
    let uses = match found.anchor {
        0 => Vec::new(),
        id => finder
            .aliases
            .remove(&id)
            .unwrap_or_default()
            .into_iter()
            .map(byte_span)
            .collect::<Result<_, _>>()?,
    };
    Ok(Located {
        span,
        decoded: found.decoded,
        alias,
        uses,
    })
}

/// saphyr-parser `Marker::index` is a character offset, measured on 0.0.12 and
/// 0.2.2; 0.2.2's doc comment says bytes, which its scanner does not do.
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
    text: &'a str,
    path: &'a [&'a str],
    keys: Vec<String>,
    pending: Option<String>,
    expect_key: bool,
    skip: usize,
    mapping_depth: usize,
    found: Option<Found>,
    err: Option<CatalogYamlError>,
    anchors: BTreeMap<usize, (usize, usize, ScalarStyle, String)>,
    aliases: BTreeMap<usize, Vec<(usize, usize)>>,
}

/// Spans in characters, as saphyr reports them.
struct Found {
    start: usize,
    end: usize,
    style: ScalarStyle,
    decoded: String,
    alias: Option<(usize, usize)>,
    anchor: usize,
}

impl PathFinder<'_> {
    /// Where a scalar's own text begins. saphyr-parser 0.3 starts a scalar's
    /// span at its `&anchor` or `!tag` property, where a bare scalar's starts
    /// at its value, and a bare block scalar's at its first content line. So
    /// the properties, the separation after them (YAML's space, tab, line
    /// break and comments) and, for a block scalar, its indicator line are
    /// stepped over. Spans count characters.
    fn value_start(
        &self,
        start: usize,
        anchor_id: usize,
        tagged: bool,
        style: ScalarStyle,
    ) -> usize {
        fn separation(c: char) -> bool {
            matches!(c, ' ' | '\t' | '\n' | '\r')
        }
        if anchor_id == 0 && !tagged {
            return start;
        }
        let mut at = start;
        let mut chars = self.text.chars().skip(start).peekable();
        let skip_separation = |chars: &mut std::iter::Peekable<_>, at: &mut usize| loop {
            while chars.next_if(|c| separation(*c)).is_some() {
                *at += 1;
            }
            if chars.peek() != Some(&'#') {
                break;
            }
            while chars.next_if(|c| *c != '\n').is_some() {
                *at += 1;
            }
        };
        while matches!(chars.peek(), Some('&' | '!')) {
            while chars.next_if(|c| !separation(*c)).is_some() {
                at += 1;
            }
            skip_separation(&mut chars, &mut at);
        }
        if matches!(style, ScalarStyle::Literal | ScalarStyle::Folded)
            && matches!(chars.peek(), Some('|' | '>'))
        {
            while chars.next_if(|c| *c != '\n').is_some() {
                at += 1;
            }
            while chars.next_if(|c| separation(*c)).is_some() {
                at += 1;
            }
        }
        at
    }

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

    fn record_scalar(&mut self, found: Found) {
        if self.found.is_some() {
            self.fail(CatalogYamlError::duplicate(self.path));
            return;
        }
        self.found = Some(found);
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
            self.record_scalar(Found {
                start,
                end,
                style,
                decoded: value,
                alias: None,
                anchor: anchor_id,
            });
        } else if self.continues() {
            self.fail(CatalogYamlError::not_string(self.path));
        }
        self.pending = None;
        self.expect_key = true;
    }

    fn alias(&mut self, id: usize, token: (usize, usize)) {
        if self.expect_key {
            self.fail(CatalogYamlError::Invalid);
            return;
        }
        if self.at_target() {
            if let Some((start, end, style, decoded)) = self.anchors.get(&id).cloned() {
                self.record_scalar(Found {
                    start,
                    end,
                    style,
                    decoded,
                    alias: Some(token),
                    anchor: 0,
                });
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
        if let Event::Alias(id) = ev {
            self.aliases
                .entry(id)
                .or_default()
                .push((span.start.index(), span.end.index()));
        }
        if self.skip > 0 {
            match ev {
                Event::MappingStart(_, _) | Event::SequenceStart(_, _) => self.skip += 1,
                Event::MappingEnd | Event::SequenceEnd => self.skip -= 1,
                Event::Scalar(value, style, anchor_id, tag) if anchor_id > 0 => {
                    let start =
                        self.value_start(span.start.index(), anchor_id, tag.is_some(), style);
                    self.anchors.insert(
                        anchor_id,
                        (start, span.end.index(), style, value.into_owned()),
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
            Event::Scalar(value, style, anchor_id, tag) => {
                let start = self.value_start(span.start.index(), anchor_id, tag.is_some(), style);
                self.scalar(
                    value.into_owned(),
                    style,
                    anchor_id,
                    start,
                    span.end.index(),
                );
            }
            Event::Alias(id) => self.alias(id, (span.start.index(), span.end.index())),
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
