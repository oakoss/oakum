//! `CommonMark` primitives shared by the changelog reader and writer.

/// Fence state as `CommonMark` defines it: a run of three or more backticks
/// or tildes opens a block (a backtick opener's info string cannot itself
/// contain a backtick); only the same character in a run at least as long,
/// followed by nothing but whitespace, closes it, so a `~~~` line or a
/// fence with an info string inside a backtick block is content. Four or
/// more columns of leading space (a tab counts four) make an indented code
/// line, not a fence. A fence left open runs to the end of the text, as it
/// would render.
#[derive(Default)]
pub(super) struct Fence {
    open: Option<(char, usize)>,
}

impl Fence {
    /// Records `line` and reports whether it belongs to a fenced block: the
    /// fence lines themselves and everything between them.
    pub(super) fn observe(&mut self, line: &str) -> bool {
        match self.open {
            None => match fence_opener(line) {
                Some(opened) => {
                    self.open = Some(opened);
                    true
                }
                None => false,
            },
            Some((open_char, open_len)) => {
                if fence_closer(line).is_some_and(|(ch, len)| ch == open_char && len >= open_len) {
                    self.open = None;
                }
                true
            }
        }
    }
}

/// Lines no version heading can be on: fenced code, an HTML comment, or a
/// `<details>` block, each of which hides or collapses a `## 0.1.1` inside it.
/// `CommonMark` ends a `<details>` HTML block at the first blank line and
/// renders what follows inside the open element, so this ends it at the
/// matching `</details>` instead; nested blocks count.
#[derive(Default)]
pub(super) struct Hidden {
    fence: Fence,
    comment: bool,
    details: usize,
}

impl Hidden {
    /// The HTML block `text` leaves open and its opening line, 1-based. It
    /// hides every heading after it, where an older entry's section may be.
    pub(super) fn open_html(text: &str) -> Option<(&'static str, usize)> {
        let mut hidden = Self::default();
        let mut opened_at = 0;
        for (at, line) in text.lines().enumerate() {
            let was_open = hidden.html_open();
            hidden.observe(line.trim_end_matches('\r'));
            if !was_open && hidden.html_open() {
                opened_at = at + 1;
            }
        }
        if hidden.comment {
            Some(("an HTML comment", opened_at))
        } else if hidden.details > 0 {
            Some(("a `<details>` block", opened_at))
        } else {
            None
        }
    }

    /// Records `line` and reports whether it is hidden.
    pub(super) fn observe(&mut self, line: &str) -> bool {
        if self.comment {
            self.comment = !line.contains("-->");
            return true;
        }
        if self.fence.observe(line) {
            return true;
        }
        let (columns, rest) = indent_columns(line);
        if columns < 4 {
            if rest.starts_with("<!--") {
                self.comment = !rest.contains("-->");
                return true;
            }
            self.details += details_openers(rest);
        }
        let hidden = self.details > 0;
        self.details = self.details.saturating_sub(details_closers(line));
        hidden
    }

    const fn html_open(&self) -> bool {
        self.comment || self.details > 0
    }

    /// What `text` leaves open to swallow whatever follows it.
    pub(super) fn left_open(text: &str) -> Option<&'static str> {
        let mut hidden = Self::default();
        for line in text.lines() {
            hidden.observe(line.trim_end_matches('\r'));
        }
        if hidden.fence.open.is_some() {
            Some("a code fence")
        } else if hidden.comment {
            Some("an HTML comment")
        } else if hidden.details > 0 {
            Some("a `<details>` block")
        } else {
            None
        }
    }
}

/// HTML tag names ignore ASCII case.
fn is_details_opener(rest: &str) -> bool {
    rest.get(..8)
        .is_some_and(|tag| tag.eq_ignore_ascii_case("<details"))
        && rest[8..].starts_with(['>', ' ', '\t'])
        || rest.eq_ignore_ascii_case("<details")
}

/// `</details>` tags on `line`, in any case and with space before the `>`.
fn details_closers(line: &str) -> usize {
    if !line.contains("</") {
        return 0;
    }
    let lower = line.to_ascii_lowercase();
    lower
        .match_indices("</details")
        .filter(|(at, tag)| lower[at + tag.len()..].trim_start().starts_with('>'))
        .count()
}

/// The `<details>` openers chained at the start of `rest`, as in
/// `<details><details>`; one named later on the line, in a summary or a code
/// span, is text.
fn details_openers(mut rest: &str) -> usize {
    let mut count = 0;
    while is_details_opener(rest) {
        count += 1;
        let Some(end) = rest.find('>') else {
            break;
        };
        rest = rest[end + 1..].trim_start();
    }
    count
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

/// The marker run of a fence line, or `None` for indented or non-fence text.
fn fence_run(line: &str) -> Option<(char, usize, &str)> {
    let (columns, rest) = indent_columns(line);
    if columns >= 4 {
        return None;
    }
    let ch = rest.chars().next()?;
    if ch != '`' && ch != '~' {
        return None;
    }
    let run = rest.chars().take_while(|c| *c == ch).count();
    (run >= 3).then(|| (ch, run, &rest[run..]))
}

fn fence_opener(line: &str) -> Option<(char, usize)> {
    let (ch, run, info) = fence_run(line)?;
    if ch == '`' && info.contains('`') {
        return None;
    }
    Some((ch, run))
}

fn fence_closer(line: &str) -> Option<(char, usize)> {
    let (ch, run, rest) = fence_run(line)?;
    rest.trim().is_empty().then_some((ch, run))
}

#[cfg(test)]
mod tests {
    use super::{fence_closer, fence_opener, fence_run, Fence, Hidden};

    fn fenced(text: &str) -> Vec<bool> {
        let mut fence = Fence::default();
        text.lines().map(|line| fence.observe(line)).collect()
    }

    fn hidden(text: &str) -> Vec<bool> {
        let mut hidden = Hidden::default();
        text.lines().map(|line| hidden.observe(line)).collect()
    }

    #[test]
    fn a_comment_hides_through_its_closing_line() {
        assert_eq!(hidden("<!-- ## 0.1.1 -->\nafter"), vec![true, false]);
        assert_eq!(
            hidden("<!--\n## 0.1.1\n```\n-->\nafter"),
            vec![true, true, true, true, false]
        );
        assert_eq!(hidden("    <!--\nafter"), vec![false, false]);
        assert_eq!(hidden("text <!--\nafter"), vec![false, false]);
        assert_eq!(hidden("<!-->\nafter"), vec![true, false]);
        assert_eq!(hidden("<!--->\nafter"), vec![true, false]);
    }

    #[test]
    fn details_tags_ignore_case_and_space_before_the_bracket() {
        assert_eq!(
            hidden("<DETAILS>\n## 0.1.1\n</Details >\nafter"),
            vec![true, true, true, false]
        );
        assert_eq!(
            hidden("<details\n</details\t>\nafter"),
            vec![true, true, false]
        );
        assert_eq!(hidden("    <details>\n## 0.1.1"), vec![false, false]);
    }

    #[test]
    fn open_html_names_the_outermost_block_and_its_line() {
        assert_eq!(
            Hidden::open_html("## b\n<details>\n<details>\n</details>\n## a"),
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
            hidden("<details>\n<summary>old</summary>\n\n## 0.1.1\n\n</details>\nafter"),
            vec![true, true, true, true, true, true, false]
        );
        assert_eq!(
            hidden("<details open>\n<details>\n</details>\n## 0.1.1\n</details>\nafter"),
            vec![true, true, true, true, true, false]
        );
        assert_eq!(
            hidden("<details><summary>x</summary>y</details>\nafter"),
            vec![true, false]
        );
        assert_eq!(hidden("<detailsx>\nafter"), vec![false, false]);
        assert_eq!(
            hidden("<details><details>\n</details>\n## 0.3.0\n</details>\nafter"),
            vec![true, true, true, true, false]
        );
        assert_eq!(
            hidden("<details><summary>`<details>` parser</summary>\n</details>\nafter"),
            vec![true, true, false]
        );
    }

    #[test]
    fn a_close_inside_a_fence_or_a_comment_opener_inside_one_is_content() {
        assert_eq!(
            hidden("<details>\n```\n</details>\n```\n</details>\nafter"),
            vec![true, true, true, true, true, false]
        );
        assert_eq!(
            hidden("```\n<!--\n```\nafter"),
            vec![true, true, true, false]
        );
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
    fn fence_run_needs_three_markers_under_four_columns() {
        assert_eq!(fence_run("```"), Some(('`', 3, "")));
        assert_eq!(fence_run("````md"), Some(('`', 4, "md")));
        assert_eq!(fence_run("   ~~~ x"), Some(('~', 3, " x")));
        assert_eq!(fence_run("``"), None);
        assert_eq!(fence_run("    ```"), None);
        assert_eq!(fence_run("\t```"), None);
        assert_eq!(fence_run("text"), None);
        assert_eq!(fence_run(""), None);
    }

    #[test]
    fn a_backtick_opener_rejects_a_backtick_in_its_info_string() {
        assert_eq!(fence_opener("```lang`x"), None);
        assert_eq!(fence_opener("~~~lang`x"), Some(('~', 3)));
        assert_eq!(fence_opener("```sh"), Some(('`', 3)));
        assert_eq!(fenced("```lang`x\nafter"), vec![false, false]);
    }

    #[test]
    fn a_closer_carries_nothing_but_whitespace() {
        assert_eq!(fence_closer("```sh"), None);
        assert_eq!(fence_closer("``` "), Some(('`', 3)));
        assert_eq!(fence_closer("~~~~"), Some(('~', 4)));
    }

    #[test]
    fn a_block_closes_only_on_its_own_marker_at_least_as_long() {
        assert_eq!(
            fenced("```text\n~~~\n```\ntail"),
            vec![true, true, true, false]
        );
        assert_eq!(
            fenced("````md\n```sh\n````\ntail"),
            vec![true, true, true, false]
        );
        assert_eq!(
            fenced("```text\n```sh\n```\ntail"),
            vec![true, true, true, false]
        );
        assert_eq!(fenced("````\n```\ntail"), vec![true, true, true]);
        assert_eq!(fenced("````\n````\ntail"), vec![true, true, false]);
    }

    #[test]
    fn an_unclosed_fence_runs_to_the_end() {
        assert_eq!(fenced("```\nafter\nmore"), vec![true, true, true]);
        assert_eq!(fenced("    ```\nafter"), vec![false, false]);
    }
}
