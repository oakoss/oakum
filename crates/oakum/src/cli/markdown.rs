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

/// The marker run of a fence line, or `None` for indented or non-fence text.
fn fence_run(line: &str) -> Option<(char, usize, &str)> {
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
    use super::{fence_closer, fence_opener, fence_run, Fence};

    fn fenced(text: &str) -> Vec<bool> {
        let mut fence = Fence::default();
        text.lines().map(|line| fence.observe(line)).collect()
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
