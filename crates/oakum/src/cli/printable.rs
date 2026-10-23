//! What oakum lets reach a terminal. Bump files, the paths naming them, git
//! config and a package manager's error text can all arrive from someone else,
//! so anything that could rewrite what the terminal shows is escaped rather
//! than sent on. `is_control` alone is not enough — a bidi override reverses
//! the rendering of everything after it and is not a control character.

use std::borrow::Cow;
use std::fmt;
use std::io::{self, Write};

/// One value inside a message, newline included: a path holding a newline
/// would otherwise split one list entry across two lines.
pub(super) struct Printable<'a>(pub(super) &'a str);

impl fmt::Display for Printable<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        escape_into(f, self.0, false)
    }
}

/// A whole message on its way to stdout or stderr. Its own newlines are
/// oakum's, so they pass; everything else follows [`Printable`].
pub(super) fn for_terminal(text: &str) -> Cow<'_, str> {
    if text.chars().all(|ch| shown_as_written(ch, true)) {
        return Cow::Borrowed(text);
    }
    let mut escaped = String::with_capacity(text.len() + 8);
    // Writing to a String cannot fail.
    let _ = escape_into(&mut escaped, text, true);
    Cow::Owned(escaped)
}

pub(super) fn say(mut out: impl Write, line: &str) -> io::Result<()> {
    writeln!(out, "{}", for_terminal(line))
}

pub(super) fn prompt(mut out: impl Write, text: &str) -> io::Result<()> {
    out.write_all(for_terminal(text).as_bytes())?;
    out.flush()
}

fn escape_into(out: &mut impl fmt::Write, text: &str, newline: bool) -> fmt::Result {
    for ch in text.chars() {
        if shown_as_written(ch, newline) {
            out.write_char(ch)?;
        } else {
            write!(out, "{}", ch.escape_debug())?;
        }
    }
    Ok(())
}

fn shown_as_written(ch: char, newline: bool) -> bool {
    match ch {
        '\n' => newline,
        // Neither moves a cursor nor hides text, and the joiners are
        // orthographically required in Persian, Urdu and Indic scripts and in
        // emoji sequences.
        '\t' | '"' | '\'' | '\\' | '\u{200C}' | '\u{200D}' => true,
        _ => printable_after_a_base(ch),
    }
}

/// Rust's own printability table, which escapes Cc, Cf, Cs, Co, Cn, Zl and Zp:
/// every hand-written range list tried here missed a carrier, tag characters
/// and U+2028 among them. `str::escape_debug` escapes a combining mark only at
/// the start of a string, so asking after a base character keeps Vietnamese,
/// Hebrew and Devanagari marks readable.
fn printable_after_a_base(ch: char) -> bool {
    let mut pair = String::with_capacity(5);
    pair.push('a');
    pair.push(ch);
    pair.escape_debug().count() == 2
}

#[cfg(test)]
mod tests {
    use super::{for_terminal, prompt, say, Printable};

    #[test]
    fn a_said_line_is_escaped_and_ends_in_a_newline() {
        let mut wrote = Vec::new();
        say(&mut wrote, "a\u{1b}[2J\nb").unwrap();
        assert_eq!(wrote, b"a\\u{1b}[2J\nb\n");
        let mut wrote = Vec::new();
        prompt(&mut wrote, "pick \u{202e}x: ").unwrap();
        assert_eq!(wrote, b"pick \\u{202e}x: ");
    }

    /// The carriers a hand-written range list kept missing, and the two that
    /// rewrite a line.
    #[test]
    fn invisible_carriers_are_escaped_and_ordinary_text_is_not() {
        for hidden in [
            '\u{1b}',
            '\r',
            '\u{202E}',
            '\u{200B}',
            '\u{E0041}',
            '\u{2060}',
            '\u{2028}',
            '\u{00AD}',
            '\u{FEFF}',
        ] {
            for rendered in [
                Printable(&hidden.to_string()).to_string(),
                for_terminal(&hidden.to_string()).into_owned(),
            ] {
                assert!(
                    !rendered.contains(hidden),
                    "U+{:04X} reached raw",
                    hidden as u32
                );
            }
        }
        // A reader has to recognize the file, so ordinary script stays readable.
        for shown in [
            "中",
            "👩\u{200D}💻",
            "é",
            "e\u{301}",
            "\u{301}",
            "mi\u{200C}khaham",
            "\t",
            "\"'\\",
        ] {
            assert_eq!(Printable(shown).to_string(), shown);
            assert_eq!(for_terminal(shown), shown);
        }
    }

    /// The message's own line breaks are oakum's; a value's are not.
    #[test]
    fn a_newline_passes_in_a_message_and_not_in_a_value() {
        assert_eq!(for_terminal("a\nb"), "a\nb");
        assert_eq!(Printable("a\nb").to_string(), "a\\nb");
    }

    /// An escaped value is all ASCII, so the message around it leaves it alone.
    #[test]
    fn an_escaped_value_is_not_escaped_twice() {
        let value = Printable("z\u{1b}[2Jz\u{202e}.md").to_string();
        assert_eq!(for_terminal(&value), value);
    }
}
