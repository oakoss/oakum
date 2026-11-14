//! `CommonMark` primitives shared by the changelog reader and writer.

use core::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

/// Which lines of a document no version heading can be on, and what it leaves
/// open. Two layers decide it, as a renderer does: `CommonMark` (pulldown-cmark)
/// says what is raw HTML, a code block or a code span, and an HTML tokenizer
/// reads the raw HTML for comments, tags and quoted attributes. A line is hidden
/// when it belongs to an HTML block or a code block, starts inside an inline
/// HTML span, or starts inside an open comment or `<details>` element, which
/// renders everything after it collapsed until the matching `</details>`.
pub(super) struct Hidden {
    lines: Vec<bool>,
    /// The innermost open block's name and the line, 1-based, where the
    /// outermost one opened.
    open: Option<(&'static str, usize)>,
    fence_open: bool,
}

/// The four tags whose `CommonMark` HTML block runs to the first closer of any
/// of them, across blank lines.
#[derive(Clone, Copy)]
enum RawText {
    Pre,
    Script,
    Style,
    Textarea,
}

impl RawText {
    const ALL: [Self; 4] = [Self::Pre, Self::Script, Self::Style, Self::Textarea];

    const fn tag(self) -> &'static str {
        match self {
            Self::Pre => "pre",
            Self::Script => "script",
            Self::Style => "style",
            Self::Textarea => "textarea",
        }
    }

    const fn block(self) -> &'static str {
        match self {
            Self::Pre => "a `<pre>` block",
            Self::Script => "a `<script>` block",
            Self::Style => "a `<style>` block",
            Self::Textarea => "a `<textarea>` block",
        }
    }

    /// The tag `rest` opens with: `<tag` in any case, then a space, a tab,
    /// `>` or the end of the line.
    fn opening(rest: &str) -> Option<Self> {
        let open = rest.strip_prefix('<')?;
        Self::ALL.into_iter().find(|raw| {
            let tag = raw.tag();
            open.get(..tag.len())
                .is_some_and(|name| name.eq_ignore_ascii_case(tag))
                && (open.len() == tag.len()
                    || open[tag.len()..].starts_with([' ', '\t', '>', '\n', '\r']))
        })
    }

    /// Whether `block` holds any closer of the four, in any case.
    fn closed(block: &str) -> bool {
        let lower = block.to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .any(|raw| lower.contains(&format!("</{}>", raw.tag())))
    }
}

/// The HTML state the raw HTML read so far leaves.
#[derive(Default)]
struct Html {
    comment: bool,
    details: usize,
    /// The `script`, `style` or `textarea` whose text is open: nothing in it
    /// is a tag or a comment until its own end tag.
    raw: Option<&'static str>,
}

impl Html {
    const fn open(&self) -> bool {
        self.comment || self.details > 0 || self.raw.is_some()
    }

    /// Reads `piece`, which starts at `base` in the document, and records in
    /// `opened` where the state last went from closed to open.
    fn feed(&mut self, piece: &str, base: usize, opened: &mut Option<usize>) {
        let mut at = 0;
        while at < piece.len() {
            let was_open = self.open();
            let rest = &piece[at..];
            let mut start = at;
            if let Some(raw) = self.raw {
                match end_tag_at(rest, raw) {
                    Some(end) => {
                        self.raw = None;
                        at += end;
                    }
                    None => at = piece.len(),
                }
            } else if self.comment {
                match rest.find("-->") {
                    Some(end) => {
                        self.comment = false;
                        at += end + 3;
                    }
                    None => at = piece.len(),
                }
            } else if let Some(lt) = rest.find('<') {
                let tag_at = at + lt;
                start = tag_at;
                let tag = &piece[tag_at..];
                if let Some(after) = tag.strip_prefix("<!--") {
                    // `<!-->` and `<!--->` are whole, empty comments.
                    if after.starts_with('>') {
                        at = tag_at + 5;
                    } else if after.starts_with("->") {
                        at = tag_at + 6;
                    } else {
                        self.comment = true;
                        at = tag_at + 4;
                    }
                } else if tag.starts_with("<!") || tag.starts_with("<?") {
                    // A doctype, CDATA or processing instruction is a bogus
                    // comment to an HTML parser, ending at its first `>`.
                    at = tag.find('>').map_or(piece.len(), |end| tag_at + end + 1);
                } else if let Some((name, closing, len)) = scan_tag(tag) {
                    if name.eq_ignore_ascii_case("details") {
                        if closing {
                            self.details = self.details.saturating_sub(1);
                        } else {
                            self.details += 1;
                        }
                    } else if !closing {
                        self.raw = ["script", "style", "textarea"]
                            .into_iter()
                            .find(|raw| name.eq_ignore_ascii_case(raw));
                    }
                    at = tag_at + len;
                } else {
                    at = tag_at + 1;
                }
            } else {
                at = piece.len();
            }
            match (was_open, self.open()) {
                (false, true) => *opened = Some(base + start),
                (true, false) => *opened = None,
                _ => {}
            }
        }
    }
}

/// Where the end tag `</name>` that closes a raw-text element starts in `text`,
/// in any case and followed by whitespace, `/` or `>`.
fn end_tag_at(text: &str, name: &str) -> Option<usize> {
    let lower = text.to_ascii_lowercase();
    let closer = format!("</{name}");
    lower.match_indices(&closer).map(|(at, _)| at).find(|&at| {
        lower[at + closer.len()..]
            .chars()
            .next()
            .is_none_or(|next| next.is_ascii_whitespace() || next == '/' || next == '>')
    })
}

/// Where an HTML tokenizer stands inside a tag's attributes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Attr {
    Between,
    Name,
    /// Past `=`, where a quote opens a value and anything else starts an
    /// unquoted one.
    Value,
    Quoted(char),
    Unquoted,
}

/// A start or end tag at the head of `text`: its name as an HTML tokenizer
/// reads it (to whitespace, `/` or `>`), whether it closes, and its length
/// through `>`, or through the end of `text` when it never closes. A quote
/// opens a value only right after `=`, so a `>` or `<!--` inside one is part
/// of the tag while an apostrophe in an unquoted value is not a quote.
fn scan_tag(text: &str) -> Option<(&str, bool, usize)> {
    let after = text.strip_prefix('<')?;
    let (closing, name_at) = match after.strip_prefix('/') {
        Some(rest) => (true, rest),
        None => (false, after),
    };
    if !name_at.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return None;
    }
    let name_len = name_at
        .find(|c: char| c.is_ascii_whitespace() || c == '/' || c == '>')
        .unwrap_or(name_at.len());
    let name = &name_at[..name_len];
    let mut at = text.len() - name_at.len() + name_len;
    let mut state = Attr::Between;
    for c in text[at..].chars() {
        at += c.len_utf8();
        state = match (state, c) {
            (Attr::Quoted(quote), c) if c == quote => Attr::Between,
            (Attr::Quoted(quote), _) => Attr::Quoted(quote),
            (Attr::Value, '"' | '\'') => Attr::Quoted(c),
            (_, '>') => return Some((name, closing, at)),
            (Attr::Value, c) if c.is_ascii_whitespace() => Attr::Value,
            (_, c) if c.is_ascii_whitespace() => Attr::Between,
            (Attr::Value | Attr::Unquoted, _) => Attr::Unquoted,
            (Attr::Between | Attr::Name, '=') => Attr::Value,
            (Attr::Between | Attr::Name, _) => Attr::Name,
        };
    }
    Some((name, closing, text.len()))
}

/// What pulldown-cmark finds in a document: block ranges no heading is in, the
/// raw HTML in order, and what the document leaves open.
#[derive(Default)]
struct Scan {
    blocks: Vec<Range<usize>>,
    inline: Vec<Range<usize>>,
    /// Each HTML block whole, so a tag or quoted value spanning its lines
    /// reads as one, and each inline HTML span.
    pieces: Vec<Range<usize>>,
    /// List items and block quotes: a `<details>` left open inside one closes
    /// with it.
    containers: Vec<Range<usize>>,
    raw_open: Option<(RawText, usize)>,
    fence_open: bool,
}

/// A same-length rename in the copy pulldown-cmark parses.
struct Rename {
    at: usize,
    len: usize,
    head: &'static str,
}

impl Scan {
    /// pulldown-cmark 0.13.6 ends a `pre`, `script`, `style` or `textarea`
    /// block only on its opener's own closer in lower case
    /// (`get_html_end_tag` in `firstpass.rs`), where `CommonMark` ends it on
    /// the first line holding any of the four in any case. Each such block is
    /// fixed in a same-length copy, inside the block where no inline parse
    /// reaches, and the copy parsed again until none is left; every range
    /// indexes `text` as well.
    fn of(text: &str) -> Self {
        let mut copy = as_commonmark_reads_it(text);
        loop {
            let (scan, renames) = Self::read(text, &copy);
            let mut bytes = copy.into_bytes();
            let before = bytes.clone();
            for Rename { at, len, head } in renames {
                bytes[at..at + head.len()].copy_from_slice(head.as_bytes());
                bytes[at + head.len()..at + len].fill(b' ');
            }
            if bytes == before {
                return scan;
            }
            copy = String::from_utf8(bytes).expect("ASCII bytes were replaced with ASCII");
        }
    }

    fn read(text: &str, parsed: &str) -> (Self, Vec<Rename>) {
        let mut scan = Self::default();
        let mut fixes = Vec::new();
        let mut code: Option<usize> = None;
        let mut depth = 0_usize;
        for (event, range) in Parser::new_ext(parsed, Options::empty()).into_offset_iter() {
            match event {
                Event::Start(Tag::HtmlBlock) => {
                    let block = &text[range.clone()];
                    let lead = range.end - block.trim_start_matches([' ', '\t']).len();
                    fixes.extend(block_fixes(text, lead, range.end));
                    scan.raw_open = RawText::opening(block.trim_start_matches([' ', '\t']))
                        .filter(|_| range.end == text.len() && !RawText::closed(block))
                        .map(|raw| (raw, range.start));
                    scan.pieces.push(range.clone());
                    scan.blocks.push(range);
                }
                Event::Start(Tag::Item | Tag::BlockQuote(_) | Tag::Heading { .. }) => {
                    depth += 1;
                    scan.containers.push(range);
                }
                Event::End(TagEnd::Item | TagEnd::BlockQuote(_) | TagEnd::Heading(_)) => {
                    depth = depth.saturating_sub(1);
                }
                Event::Start(Tag::CodeBlock(kind)) => {
                    // Past the opener line, then past each line of code.
                    let opener_end = text[range.clone()]
                        .find('\n')
                        .map_or(range.end, |at| range.start + at + 1);
                    code = matches!(kind, CodeBlockKind::Fenced(_)).then_some(opener_end);
                    scan.blocks.push(range);
                }
                Event::Text(_) if code.is_some() => code = Some(range.end),
                Event::End(TagEnd::CodeBlock) => {
                    // A closing fence leaves text after the code, whatever
                    // container prefixes its lines; a fence in a container
                    // closes with it, at the first line that leaves it.
                    scan.fence_open = code.take().is_some_and(|after| {
                        depth == 0
                            && range.end == text.len()
                            && text[after..range.end].trim().is_empty()
                    });
                }
                Event::InlineHtml(_) => {
                    scan.pieces.push(range.clone());
                    scan.inline.push(range);
                }
                _ => {}
            }
        }
        (scan, fixes)
    }
}

/// What makes pulldown-cmark read the HTML block whose tag starts at `lead`
/// and runs to `end` as `CommonMark` does (`Scan::of`); none when it
/// already does.
fn block_fixes(text: &str, lead: usize, end: usize) -> Vec<Rename> {
    let Some(raw) = RawText::opening(&text[lead..]) else {
        return Vec::new();
    };
    let mut at = lead;
    for block_line in text[lead..end].split_inclusive('\n') {
        let line_end = at + block_line.len();
        if RawText::closed(block_line) {
            if line_end >= end {
                return Vec::new();
            }
            let closer = block_line.match_indices("</").find_map(|(offset, _)| {
                raw_closer_len(&block_line[offset..]).map(|len| Rename {
                    at: at + offset,
                    len,
                    head: "</pre>",
                })
            });
            let opener = Rename {
                at: lead,
                len: raw.tag().len() + 1,
                head: "<pre",
            };
            return [opener].into_iter().chain(closer).collect();
        }
        at = line_end;
    }
    Vec::new()
}

/// The copy of `text` pulldown-cmark first parses, at the same length: tabs
/// after a fence run ending a line become spaces, since pulldown-cmark
/// 0.13.6 closes a fence only on trailing spaces (`scan_closing_code_fence`
/// in `scanners.rs`) where `CommonMark` takes tabs too; and the four raw tag
/// names are lowercased (`with_raw_tag_names_lowercased`).
fn as_commonmark_reads_it(text: &str) -> String {
    let mut bytes = with_raw_tag_names_lowercased(text).into_bytes();
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        let trimmed = body.trim_end_matches([' ', '\t']);
        let fence = trimmed
            .chars()
            .next_back()
            .filter(|c| *c == '`' || *c == '~')
            .is_some_and(|mark| trimmed.chars().rev().take_while(|c| *c == mark).count() >= 3);
        if fence {
            for byte in &mut bytes[at + trimmed.len()..at + body.len()] {
                *byte = b' ';
            }
        }
        at += line.len();
    }
    String::from_utf8(bytes).expect("ASCII tabs were replaced with spaces")
}

/// `text` with the name in every opener and closer of the four lowercased,
/// so a block closed in another case ends on the first parse. Case changes
/// no length, and nothing inline reads a tag name's case.
fn with_raw_tag_names_lowercased(text: &str) -> String {
    let mut bytes = text.as_bytes().to_vec();
    let mut at = 0;
    while let Some(found) = text[at..].find('<') {
        at += found;
        let rest = &text[at..];
        let name = if let Some(len) = raw_closer_len(rest) {
            at + 2..at + len - 1
        } else if let Some(raw) = RawText::opening(rest) {
            at + 1..at + 1 + raw.tag().len()
        } else {
            at + 1..at + 1
        };
        bytes[name.clone()].make_ascii_lowercase();
        at = name.end.max(at + 1);
    }
    String::from_utf8(bytes).expect("ASCII letters were lowercased")
}

/// The length of the `</pre>`, `</script>`, `</style>` or `</textarea>`, in
/// any case, at the head of `rest`.
fn raw_closer_len(rest: &str) -> Option<usize> {
    let name = rest.as_bytes().strip_prefix(b"</")?;
    RawText::ALL.into_iter().find_map(|raw| {
        let tag = raw.tag().as_bytes();
        name.get(..=tag.len())
            .is_some_and(|head| {
                head[..tag.len()].eq_ignore_ascii_case(tag) && head[tag.len()] == b'>'
            })
            .then_some(tag.len() + 3)
    })
}

enum Step {
    Enter,
    Leave,
    Feed(Range<usize>),
}

impl Step {
    const fn rank(&self) -> u8 {
        match self {
            Self::Leave => 0,
            Self::Enter => 1,
            Self::Feed(_) => 2,
        }
    }
}

impl Hidden {
    pub(super) fn of(text: &str) -> Self {
        let starts: Vec<usize> = line_starts(text);
        let Scan {
            blocks,
            inline,
            pieces,
            containers,
            raw_open,
            fence_open,
        } = Scan::of(text);
        // At one offset a container's end comes first, then a start, then
        // the raw HTML read there.
        let mut steps: Vec<(usize, Step)> = containers
            .iter()
            .flat_map(|c| [(c.start, Step::Enter), (c.end, Step::Leave)])
            .chain(
                pieces
                    .iter()
                    .map(|piece| (piece.start, Step::Feed(piece.clone()))),
            )
            .collect();
        steps.sort_by_key(|(at, step)| (*at, step.rank()));
        let mut steps = steps.into_iter().peekable();
        let mut html = Html::default();
        let mut opened = None;
        let mut saved: Vec<usize> = Vec::new();
        let mut take = |html: &mut Html, opened: &mut Option<usize>, step: Step| match step {
            Step::Enter => saved.push(html.details),
            Step::Leave => {
                let outer = saved.pop().unwrap_or(0);
                if html.details > outer {
                    html.details = outer;
                    if !html.open() {
                        *opened = None;
                    }
                }
            }
            Step::Feed(piece) => html.feed(&text[piece.clone()], piece.start, opened),
        };
        // Both lists are in document order and do not overlap, so one cursor
        // each, passed over a range once it ends, keeps the walk linear.
        let mut blocks = blocks.iter().peekable();
        let mut inline = inline.iter().peekable();
        let mut lines = Vec::with_capacity(starts.len());
        for (index, &start) in starts.iter().enumerate() {
            let end = starts.get(index + 1).copied().unwrap_or(text.len());
            while let Some((_, step)) = steps
                .next_if(|(at, step)| *at < start || (*at == start && matches!(step, Step::Leave)))
            {
                take(&mut html, &mut opened, step);
            }
            while blocks.next_if(|block| block.end <= start).is_some() {}
            while inline.next_if(|span| span.end <= start).is_some() {}
            let in_block = blocks
                .peek()
                .is_some_and(|block| block.start < end.max(start + 1));
            let in_inline = inline.peek().is_some_and(|span| span.start < start);
            lines.push(in_block || in_inline || html.open());
        }
        for (_, step) in steps {
            take(&mut html, &mut opened, step);
        }
        let name = if html.comment {
            Some("an HTML comment")
        } else if let Some((raw, _)) = raw_open {
            Some(raw.block())
        } else if let Some(raw) = html
            .raw
            .and_then(|tag| RawText::ALL.into_iter().find(|raw| raw.tag() == tag))
        {
            Some(raw.block())
        } else if html.details > 0 {
            Some("a `<details>` block")
        } else {
            None
        };
        let opened_at = [opened, raw_open.map(|(_, at)| at)]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(0);
        let open = name.map(|name| (name, line_of(&starts, opened_at)));
        Self {
            lines,
            open,
            fence_open,
        }
    }

    /// Whether each line of the text, as `str::lines` splits it, is hidden.
    pub(super) fn lines(self) -> std::vec::IntoIter<bool> {
        self.lines.into_iter()
    }

    /// The HTML block `text` leaves open and its opening line, 1-based. It
    /// hides every heading after it, where an older entry's section may be.
    pub(super) fn open_html(text: &str) -> Option<(&'static str, usize)> {
        Self::of(text).open
    }

    /// What `text` leaves open to swallow whatever follows it.
    pub(super) fn left_open(text: &str) -> Option<&'static str> {
        let hidden = Self::of(text);
        if hidden.fence_open {
            Some("a code fence")
        } else {
            hidden.open.map(|(name, _)| name)
        }
    }
}

/// Where each line `str::lines` yields starts.
fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        starts.push(at);
        at += line.len();
    }
    starts
}

/// The 1-based line `offset` falls on.
fn line_of(starts: &[usize], offset: usize) -> usize {
    starts.partition_point(|&start| start <= offset).max(1)
}

/// Leading indent in `CommonMark` columns (a tab counts four) and the rest of
/// the line. Four columns opens an indented code block, which is why both a
/// fence marker and an ATX heading stop being one there.
pub(super) fn indent_columns(line: &str) -> (usize, &str) {
    let mut columns = 0;
    let mut rest = line;
    for c in line.chars() {
        match c {
            ' ' => columns += 1,
            '\t' => columns += 4,
            _ => break,
        }
        rest = &rest[c.len_utf8()..];
    }
    (columns, rest)
}

#[cfg(test)]
mod tests {
    use super::{with_raw_tag_names_lowercased, Hidden};

    #[test]
    fn only_the_four_tag_names_are_lowercased() {
        assert_eq!(
            with_raw_tag_names_lowercased("<PRE x>\n</Script>\n<Pres>\n</STYLE >\n<TextArea"),
            "<pre x>\n</script>\n<Pres>\n</STYLE >\n<textarea"
        );
    }

    #[test]
    fn a_tab_after_a_closing_fence_still_closes_it() {
        assert_eq!(Hidden::left_open("```\nx\n```\t\n"), None);
        assert_eq!(
            hidden("```\nx\n```\t\n## 1.0.0\n"),
            vec![true, true, true, false]
        );
    }

    #[test]
    fn an_opener_inside_a_block_is_reported_on_its_own_line() {
        assert_eq!(
            Hidden::open_html("<div>\n<b>x</b>\n<details>\n\n## 1.0.0\n"),
            Some(("a `<details>` block", 3))
        );
        assert_eq!(
            Hidden::open_html("<div>\nx\n<!-- a\n\n## 1.0.0\n"),
            Some(("an HTML comment", 3))
        );
    }

    #[test]
    fn a_multibyte_character_beside_a_tag_is_read_whole() {
        assert_eq!(hidden("Use `</pré>` here.\n## 1.0.0"), vec![false, false]);
        assert_eq!(
            hidden("<pre>\né</script>\n## 1.0.0\n"),
            vec![true, true, false]
        );
    }

    fn hidden(text: &str) -> Vec<bool> {
        Hidden::of(text).lines().collect()
    }

    #[test]
    fn a_comment_hides_through_its_closing_line() {
        assert_eq!(hidden("<!-- ## 0.1.1 -->\nafter"), vec![true, false]);
        assert_eq!(
            hidden("<!--\n## 0.1.1\n```\n-->\nafter"),
            vec![true, true, true, true, false]
        );
        // Indented code, which holds no heading either.
        assert_eq!(hidden("    <!--\nafter"), vec![true, false]);
        assert_eq!(hidden("text <!--\nafter"), vec![false, false]);
        assert_eq!(hidden("<!-->\nafter"), vec![true, false]);
        assert_eq!(hidden("<!--->\nafter"), vec![true, false]);
    }

    #[test]
    fn details_tags_ignore_case_and_space_before_the_bracket() {
        assert_eq!(
            hidden("<DETAILS>\n## 0.1.1\n</Details >\n\nafter"),
            vec![true, true, true, false, false]
        );
        assert_eq!(hidden("    <details>\n## 0.1.1"), vec![true, false]);
    }

    /// `CommonMark`'s first HTML block kind runs from its opener to the first
    /// closer of any of the four tags, across blank lines, and does not nest.
    #[test]
    fn a_raw_text_block_hides_through_its_first_closer() {
        assert_eq!(
            hidden("<pre>\n## 0.1.1\n\n</pre>\nafter"),
            vec![true, true, true, true, false]
        );
        assert_eq!(
            hidden("<Script type=\"x\">\n## 0.1.1\n</SCRIPT>\nafter"),
            vec![true, true, true, false]
        );
        assert_eq!(hidden("<style>a{}</style>\nafter"), vec![true, false]);
        // The block ends at `</style>`, but the page's `<textarea>` does not.
        assert_eq!(
            hidden("<textarea\n<pre>\n</style>\nafter"),
            vec![true, true, true, true]
        );
        assert_eq!(hidden("<pre\n## 0.1.1"), vec![true, true]);
        assert_eq!(hidden("    <pre>\n## 0.1.1"), vec![true, false]);
        // Any whole tag alone on a line opens an HTML block to the blank line.
        assert_eq!(hidden("<preamble>\n## 0.1.1"), vec![true, true]);
        assert_eq!(hidden("<preamble>\n\n## 0.1.1"), vec![true, false, false]);
        assert_eq!(hidden("text <pre>\n## 0.1.1"), vec![false, false]);
    }

    #[test]
    fn a_raw_text_block_left_open_is_named_with_its_line() {
        assert_eq!(
            Hidden::open_html("## a\n<pre>\n## b"),
            Some(("a `<pre>` block", 2))
        );
        assert_eq!(
            Hidden::left_open("x\n\n<STYLE>\n"),
            Some("a `<style>` block")
        );
        assert_eq!(Hidden::open_html("<pre>x</pre>\n## b"), None);
    }

    #[test]
    fn open_html_names_the_outermost_block_and_its_line() {
        assert_eq!(
            Hidden::open_html("## b\n<details>\n<details>\n</details>\n\n## a"),
            Some(("a `<details>` block", 2))
        );
        assert_eq!(
            Hidden::open_html("<details>\n<!-- x\n</details>\n"),
            Some(("an HTML comment", 1))
        );
        assert_eq!(Hidden::open_html("<!-- x -->\n```\n<!--\n```\n"), None);
        assert_eq!(Hidden::open_html("```\n<!--\n"), None);
    }

    #[test]
    fn a_details_block_hides_through_its_matching_close_past_blank_lines() {
        assert_eq!(
            hidden("<details>\n<summary>old</summary>\n\n## 0.1.1\n\n</details>\n\nafter"),
            vec![true, true, true, true, true, true, false, false]
        );
        assert_eq!(
            hidden("<details open>\n<details>\n</details>\n\n## 0.1.1\n\n</details>\n\nafter"),
            vec![true, true, true, true, true, true, true, false, false]
        );
        assert_eq!(
            hidden("<details><summary>x</summary>y</details>\n\nafter"),
            vec![true, false, false]
        );
        assert_eq!(hidden("<detailsx>\n\nafter"), vec![true, false, false]);
    }

    #[test]
    fn a_close_inside_a_fence_or_a_comment_opener_inside_one_is_content() {
        assert_eq!(
            hidden("<details>\n\n```\n</details>\n```\n\n</details>\n\nafter"),
            vec![true, true, true, true, true, true, true, false, false]
        );
        assert_eq!(
            hidden("```\n<!--\n```\nafter"),
            vec![true, true, true, false]
        );
    }

    /// `okm-vj6a`: a `</details>` inside an inline comment closes nothing, a
    /// comment left open in raw HTML runs on into the next raw HTML, and a
    /// code span is text whatever it spells, while a `<!--` inside a quoted
    /// attribute is part of its tag.
    #[test]
    fn inline_comments_and_code_spans_are_read_as_a_renderer_reads_them() {
        assert_eq!(
            hidden("<details>\n\nx <!-- </details> -->\n\n## 0.1.1\n\n</details>\n\nafter"),
            vec![true, true, true, true, true, true, true, false, false]
        );
        assert_eq!(
            hidden(
                "<details>\n\n<!-- a --><!-- x\n</details> -->\n\n## 0.1.1\n\n</details>\n\nafter"
            ),
            vec![true, true, true, true, true, true, true, true, false, false]
        );
        // A `<!--` that paragraph text cannot close is text, and the
        // `</details>` line after it interrupts the paragraph as HTML.
        assert_eq!(
            hidden("<details>\n\nx <!-- a\n</details>\n\n## 0.1.1"),
            vec![true, true, true, true, false, false]
        );
        assert_eq!(
            hidden("<details>\n\n<!-- c --></details>\n\n## 0.1.1"),
            vec![true, true, true, false, false]
        );
        assert_eq!(
            hidden("<details>\n\nUse `<!--` here. </details>\n\n## 0.1.1"),
            vec![true, true, true, false, false]
        );
        assert_eq!(
            hidden("<details>\n\nA <span title=\"<!--\">x</span> </details>\n\n## 0.1.1"),
            vec![true, true, true, false, false]
        );
        assert_eq!(
            hidden("<details>\n\nUse `</details>` to close.\n\n## 0.1.1\n\n</details>\n\nafter"),
            vec![true, true, true, true, true, true, true, false, false]
        );
    }

    /// A quoted value is part of its tag even across an HTML block's lines,
    /// and a `script`'s text holds no comment.
    #[test]
    fn a_tag_is_read_whole_and_raw_text_holds_no_markup() {
        for quote in ['"', '\''] {
            assert_eq!(
                hidden(&format!(
                    "<details>\n\nA <span title={quote}a>b <!--{quote}>x</span> </details>\n\n## 0.1.1"
                )),
                vec![true, true, true, false, false],
                "{quote}"
            );
        }
        let spanning = "<details title=\"x\nvalue <!--\n\">\n\n</details>\n\n## 2.0.0";
        assert_eq!(
            hidden(spanning),
            vec![true, true, true, true, true, false, false]
        );
        assert_eq!(Hidden::open_html(spanning), None);
        // A name runs to whitespace, `/` or `>`, and only a quote right after
        // `=` opens a value.
        assert_eq!(
            Hidden::open_html("<div>\n<details:widget>\n</div>\n\n## 0.1.0"),
            None
        );
        assert_eq!(
            Hidden::open_html("<details>\n\n<div>\n</details:widget>\n</div>\n\n## 0.1.0"),
            Some(("a `<details>` block", 1))
        );
        let apostrophe = "<details>\n<a href=it's>link</a>\n</details>\n\n## 0.1.0";
        assert_eq!(Hidden::open_html(apostrophe), None);
        assert_eq!(hidden(apostrophe).last(), Some(&false));
        let script = "<script>\n// <!--\n</script>\n\n## 1.0.0";
        assert_eq!(hidden(script), vec![true, true, true, false, false]);
        assert_eq!(Hidden::open_html(script), None);
    }

    /// The rest of a document after a raw-text block cut at a mixed-case
    /// closer is read as fully as the start.
    #[test]
    fn what_follows_a_raw_block_is_read_where_commonmark_ends_it() {
        assert_eq!(
            hidden("<pre>\n</PRE>\n<!--\n## 0.1.1"),
            vec![true, true, true, true]
        );
        assert_eq!(
            Hidden::left_open("<pre>\n</Pre>\n```\nx"),
            Some("a code fence")
        );
        // The closer's case, or which of the four it is, never moves the
        // block's end, in a list item or out of one.
        for text in [
            "- <pre>\n  x</pre>\n\n```\nopen\n## 1.0.0",
            "- <pre>\n  x</pre>\n  ```\n  open\n## 1.0.0",
            "- <pre>\n  x</pre>\n\n  ```\n  open\n## 1.0.0",
            "- a\n\n  <pre>\n  x</pre>\n  ```\n  open\n## 1.0.0",
            "<script>\nx</pre>\n```\nopen",
        ] {
            for closer in ["</PRE>", "</Script>", "</textarea>"] {
                let mixed = text.replace("</pre>", closer);
                assert_eq!(hidden(&mixed), hidden(text), "{mixed:?}");
                assert_eq!(
                    Hidden::left_open(&mixed),
                    Hidden::left_open(text),
                    "{mixed:?}"
                );
            }
        }
        assert_eq!(
            Hidden::left_open("- <pre>\n  x</PRE>\n\n```\nopen"),
            Some("a code fence")
        );
        assert_eq!(Hidden::left_open("- <pre>\n  x</PRE>\n  ```\n  open"), None);
        assert_eq!(
            hidden("text <script>a\nx</Script>\n\n## 0.1.0"),
            vec![false, true, false, false]
        );
        // Only a raw block's own tags are renamed, so a link holding one
        // reads as `CommonMark` reads it.
        for text in [
            "[a](x<textarea>y<!--)\ntext\n-->\n",
            "[a](x</Script>y<!--)\ntext\n-->\n",
        ] {
            assert_eq!(hidden(text), vec![false, false, false], "{text:?}");
            assert_eq!(Hidden::left_open(text), None, "{text:?}");
        }
        assert_eq!(
            Hidden::left_open("[x][<style>]\n\n[<script>]: /u\n\n## 1.0.0\n"),
            Some("a `<style>` block")
        );
        // A lone closer starts an HTML block, as commonmark.js 0.31.2 renders it.
        assert_eq!(
            hidden("a\n\n</script>\n## 1.0.0\n"),
            vec![false, false, true, true]
        );
        // A shortened tag's padding is read as nothing more than spaces.
        assert_eq!(hidden("x</textarea>\n## 0.1.0"), vec![false, false]);
        assert_eq!(hidden("## 0.1.0 </Textarea>\n## 0.0.1"), vec![false, false]);
    }

    /// A `<details>` left open in a list item or a block quote closes with
    /// it, as the page renders it; one in paragraph text opens a block.
    #[test]
    fn a_container_closes_the_details_left_open_in_it() {
        let text = "## 1.0.0\n\n- <details>\n\n## 0.9.0\n\n> <details>\n\n## 0.8.0\n";
        assert_eq!(
            hidden(text),
            vec![false, false, true, true, false, false, true, false, false]
        );
        assert_eq!(Hidden::left_open(text), None);
        assert_eq!(
            Hidden::open_html("Wrap old entries in a <details> element.\n\n## 0.9.0"),
            Some(("a `<details>` block", 1))
        );
        for heading in [
            "## 1.0.0 <details>\n\n## 0.9.0\n",
            "1.0.0 <details>\n---\n\n## 0.9.0\n",
        ] {
            assert_eq!(Hidden::open_html(heading), None, "{heading:?}");
            assert_eq!(hidden(heading).last(), Some(&false), "{heading:?}");
        }
    }

    /// A doctype, CDATA or processing instruction ends at its first `>`, so
    /// no tag inside one counts.
    #[test]
    fn a_declaration_holds_no_tag() {
        for declaration in [
            "<!DOCTYPE html PUBLIC \"<details>\">",
            "<![CDATA[ <details> ]]>",
            "<?x <details> ?>",
        ] {
            let text = format!("{declaration}\n\n## 0.1.0");
            assert_eq!(Hidden::open_html(&text), None, "{declaration}");
            assert_eq!(hidden(&text), vec![true, false, false], "{declaration}");
        }
    }

    /// Text in a `script` left open in a paragraph hides what follows, as
    /// the page swallows it.
    #[test]
    fn an_open_raw_text_element_hides_what_follows() {
        let text = "x <script>var s = 1;\n\n## 0.9.0\n";
        assert_eq!(hidden(text), vec![false, true, true]);
        assert!(Hidden::open_html(text).is_some());
    }

    #[test]
    fn a_closed_block_reports_nothing_and_a_later_one_its_own_line() {
        assert_eq!(Hidden::left_open("```\nx\n```"), None);
        assert_eq!(Hidden::left_open("~~~~\nx\n~~~~~"), None);
        assert_eq!(Hidden::left_open("> ```\n> text\n> ```"), None);
        assert_eq!(
            Hidden::left_open("1. x\n\n     ```\n     y\n     ```"),
            None
        );
        // Open in a container, a fence closes at the first line leaving it.
        assert_eq!(Hidden::left_open("> ```\n> text"), None);
        assert_eq!(Hidden::left_open("- ```\n  code\n"), None);
        assert_eq!(
            hidden("> ```\n> code\n\n## 0.1.0\n"),
            vec![true, true, false, false]
        );
        assert_eq!(Hidden::left_open("```\ncode\n"), Some("a code fence"));
        // After a raw block cut inside a list, the rest is read at the top.
        assert_eq!(
            Hidden::left_open("- <pre>\n  x</PRE>\n\n```\nopen"),
            Some("a code fence")
        );
        assert_eq!(
            Hidden::open_html("<!-- a -->\n<pre>\n## b"),
            Some(("a `<pre>` block", 2))
        );
        // The line that ends an inline comment begun on the line above.
        assert_eq!(hidden("x <!-- a\nb -->\nafter"), vec![false, true, false]);
    }

    #[test]
    fn left_open_names_the_block() {
        assert_eq!(Hidden::left_open("```\nx"), Some("a code fence"));
        assert_eq!(Hidden::left_open("<!--\nx"), Some("an HTML comment"));
        assert_eq!(
            Hidden::left_open("<details>\nx"),
            Some("a `<details>` block")
        );
        assert_eq!(
            Hidden::left_open("<!-- x -->\n<details>\n</details>\n"),
            None
        );
    }

    #[test]
    fn a_fence_is_one_only_as_commonmark_reads_it() {
        assert_eq!(hidden("```lang`x\nafter"), vec![false, false]);
        assert_eq!(hidden("~~~lang`x\nafter"), vec![true, true]);
        assert_eq!(hidden("``\nafter"), vec![false, false]);
        assert_eq!(Hidden::left_open("```\nx\n```sh"), Some("a code fence"));
        assert_eq!(Hidden::left_open("```\nx\n``` "), None);
    }

    #[test]
    fn a_block_closes_only_on_its_own_marker_at_least_as_long() {
        assert_eq!(
            hidden("```text\n~~~\n```\ntail"),
            vec![true, true, true, false]
        );
        assert_eq!(
            hidden("````md\n```sh\n````\ntail"),
            vec![true, true, true, false]
        );
        assert_eq!(hidden("````\n```\ntail"), vec![true, true, true]);
        assert_eq!(Hidden::left_open("````\n```\ntail"), Some("a code fence"));
        assert_eq!(hidden("````\n````\ntail"), vec![true, true, false]);
    }

    #[test]
    fn an_unclosed_fence_runs_to_the_end() {
        assert_eq!(hidden("```\nafter\nmore"), vec![true, true, true]);
        assert_eq!(Hidden::left_open("```\nafter\nmore"), Some("a code fence"));
    }
}
