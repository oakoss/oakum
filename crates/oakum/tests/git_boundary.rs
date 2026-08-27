// A detector for the obvious spelling. Not the enforcement — the distinction
// `clippy.toml` draws about itself, and for the same reason.
//
// ADR-0002's rule is that git I/O stays in the binary: `cli/tags.rs` says so in
// its own header, and `oakum::tags` parses names without talking to git. Within
// the binary, `cli::git` is the only module that builds a git child.
//
// Three mechanisms cover parts of that, and it is worth knowing which:
//
// - Clippy's denylist stays armed for the library, so a `Command::new` in
//   `src/tags.rs` fails on its own — measured. What is disarmed is the `cli`
//   tree, by the crate-level `allow` in `src/main.rs`, which is deliberate: CLI
//   I/O has to stay off ADR-0002's second-marker trigger. Re-arming per module
//   with `#[deny]` was measured and does not work — it re-arms the whole
//   denylist, and `tags.rs::run` then fails on a legitimate `env::current_dir`.
// - The compiler covers the child builder, since `cli/git/env.rs` became a
//   private child of `cli/git`: `super::git::env::remote_command` from `tags.rs`
//   is `E0603`, and `pub(super) use env::remote_command;` in `git/mod.rs` is
//   `E0364`.
// - This file covers what is left: a module outside `cli/git` naming the process
//   type in the ordinary way.
//
// It catches accident, not evasion, and the difference was measured rather than
// assumed. Four of these remain open, each verified to compile with every gate
// green: `#[rustfmt::skip]` with spaced path segments, a `type` alias re-exported
// from an exempt module, `include!`, and `#[cfg_attr(all(), path = …)]`. Two
// earlier holes were closed rather than conceded, because both were reachable by
// accident: a `//` inside a string literal, and a `'"'` character literal — the
// tree has three of those, and one sat above 110 positions that no mechanism
// covered.
//
// A lexer this shallow cannot decide "does this file construct a process"; only
// a real parse can. Rules that chased one spelling of an evasion were dropped
// rather than grown, because each pinned a proof-of-concept and fell to the next
// reviewer. Deliberate evasion is left to review, which is what the module
// boundary and these headers exist to prompt.
#![allow(clippy::disallowed_methods)]

mod support;

use std::path::{Path, PathBuf};

/// The one module tree permitted to build a git child.
const GIT_MODULE: &str = "cli/git/";

/// Permitted to build some other child. `discover` asks the package managers
/// what the workspace contains (ADR-0002 records that choice); `cli/config` and
/// `cli/detect_tools` spawn only from test code. All four are asserted below to
/// never name git.
///
/// Keyed on the path from `src`, not the basename: `config.rs` alone also
/// matches the library's own `src/config.rs`, which never asked for an
/// exemption, and a new `cli/cargo.rs` would be born with one.
const OTHER_SPAWNERS: [&str; 4] = [
    "discover/cargo.rs",
    "discover/pnpm.rs",
    "cli/config.rs",
    "cli/detect_tools.rs",
];

/// Every ordinary spelling that reaches the type, anchored after `std::` so a
/// nested use tree cannot hide the path — `use std::{process::Command as Proc}`
/// contains neither `use std::process` nor `Command::new(`, and was measurably
/// missed by an earlier draft.
///
/// `process::` and not `process`, so `preprocess` and `process_commits` are not
/// accused of anything. Every needle is qualified for the same reason: a bare
/// `Command::new(` would reject `clap::Command::new`, and it is redundant —
/// each spelling below is already caught by the path or the import.
const PROCESS_NEEDLES: [&str; 4] = [
    "process::Command",
    "process::{",
    "process as ",
    "process::*",
];

fn crate_src() -> PathBuf {
    support::workspace_root().join("crates/oakum/src")
}

fn sources() -> Vec<(PathBuf, String)> {
    fn walk(dir: &Path, found: &mut Vec<(PathBuf, String)>) {
        for entry in std::fs::read_dir(dir).expect("src dir should be readable") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let text = std::fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("{} should be readable: {e}", path.display()));
                found.push((path, text));
            }
        }
    }

    let mut found = Vec::new();
    walk(&crate_src(), &mut found);
    for required in ["tags.rs", "release.rs", "github.rs", "pnpm.rs"] {
        assert!(
            found
                .iter()
                .any(|(path, _)| path.file_name().is_some_and(|name| name == required)),
            "the scan missed {required}; it covered {} files",
            found.len()
        );
    }
    found
}

/// The path from `src`, with forward slashes so the constants read the same on
/// every platform.
fn relative(path: &Path) -> String {
    path.strip_prefix(crate_src())
        .unwrap_or(path)
        .display()
        .to_string()
        .replace('\\', "/")
}

/// Only code names a type. Line comments end at the newline and count only
/// outside a string; block comments are removed whole; a string literal's
/// contents are blanked, so a diagnostic or doc quoting the rule does not fail
/// the gate it describes; and a character literal is consumed whole, because an
/// unpaired `'"'` would otherwise read as a string opener and blind everything
/// after it.
///
/// Raw identifiers name the same item — `std::process::r#Command` compiles and
/// survives `cargo fmt` — so the prefix goes before matching.
fn code_only(text: &str) -> String {
    without_raw_prefixes(&scrub(text, false))
}

/// Comments dropped, literals kept: the exemptions are asserted on what a module
/// names, and `"git"` is a literal.
fn code_and_literals(text: &str) -> String {
    scrub(text, true)
}

fn scrub(text: &str, keep_literals: bool) -> String {
    enum State {
        Code,
        Line,
        Block,
        Str,
    }

    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut state = State::Code;
    let mut escaped = false;
    let mut at = 0;
    while at < chars.len() {
        let ch = chars[at];
        let next = chars.get(at + 1).copied();
        match state {
            State::Code => {
                if ch == '/' && next == Some('/') {
                    state = State::Line;
                    at += 2;
                    continue;
                }
                if ch == '/' && next == Some('*') {
                    state = State::Block;
                    at += 2;
                    continue;
                }
                if ch == '\'' {
                    if let Some(len) = char_literal_len(&chars[at..]) {
                        at += len;
                        continue;
                    }
                    // A lifetime, which carries no quote to pair.
                }
                if ch == '"' {
                    state = State::Str;
                    escaped = false;
                }
                out.push(ch);
            }
            State::Line => {
                if ch == '\n' {
                    state = State::Code;
                    out.push(ch);
                }
            }
            State::Block => {
                if ch == '*' && next == Some('/') {
                    state = State::Code;
                    at += 2;
                    continue;
                }
                if ch == '\n' {
                    out.push(ch);
                }
            }
            State::Str => {
                if keep_literals || ch == '"' {
                    out.push(ch);
                }
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    state = State::Code;
                }
            }
        }
        at += 1;
    }
    out
}

/// A character literal holds an unpaired `"` often enough to matter — the tree
/// has `'"'` in three files — and treating it as a string opener blinds the
/// scanner from there to the next quote, across newlines. `'a` is a lifetime and
/// carries no closing quote, so it is left alone.
fn char_literal_len(chars: &[char]) -> Option<usize> {
    if chars.get(1) == Some(&'\\') {
        // `'\''`, `'\n'`, `'\u{1f600}'` — scan a bounded window for the close.
        return (3..=12)
            .find(|at| chars.get(*at) == Some(&'\''))
            .map(|at| at + 1);
    }
    (chars.get(2) == Some(&'\'')).then_some(3)
}

fn without_raw_prefixes(code: &str) -> String {
    let mut out = String::with_capacity(code.len());
    let mut rest = code;
    while let Some(at) = rest.find("r#") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        if after.starts_with('"') {
            out.push_str("r#");
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Whole files, test code included. Separating production from test code needs a
/// parser, and a marker split fails both ways: `tags.rs` carries an item-level
/// `#[cfg(test)]` at line 262 of 492 with production code after it, while
/// `config.rs` gates its tests with `#[cfg(all(test, unix))]`, which no
/// `#[cfg(test)]` split matches at all.
fn violates(path: &Path, text: &str) -> bool {
    let shown = relative(path);
    if shown.starts_with(GIT_MODULE) || OTHER_SPAWNERS.contains(&shown.as_str()) {
        return false;
    }
    let code = code_only(text);
    PROCESS_NEEDLES.iter().any(|needle| code.contains(needle))
}

#[test]
fn only_the_git_module_spawns_a_process() {
    let found: Vec<String> = sources()
        .iter()
        .filter(|(path, text)| violates(path, text))
        .map(|(path, _)| relative(path))
        .collect();
    assert!(
        found.is_empty(),
        "these modules build a child process directly instead of naming a `git::Op`: {found:?}"
    );
}

/// The exemptions are the disarm a future contributor reaches for when the rule
/// fires on them, so widening one has to be a deliberate edit in two places.
#[test]
fn the_exemptions_stay_where_they_are() {
    assert_eq!(GIT_MODULE, "cli/git/");
    assert_eq!(
        OTHER_SPAWNERS,
        [
            "discover/cargo.rs",
            "discover/pnpm.rs",
            "cli/config.rs",
            "cli/detect_tools.rs"
        ]
    );

    let sources = sources();
    let exempt: Vec<&(PathBuf, String)> = sources
        .iter()
        .filter(|(path, _)| OTHER_SPAWNERS.contains(&relative(path).as_str()))
        .collect();
    assert_eq!(
        exempt.len(),
        OTHER_SPAWNERS.len(),
        "every exemption must name exactly one real module, matched {exempt:?}"
    );
    for (path, text) in exempt {
        assert!(
            !code_and_literals(text).contains("\"git\""),
            "{} is exempted on the basis that it never spawns git, and now it names one",
            relative(path)
        );
    }
}

/// The scanner itself, over fixtures. Without this the rule can be gutted: a
/// `violates` that always returns false leaves the test green, because a healthy
/// tree has no violation to find.
#[test]
fn the_scanner_flags_a_violation_and_clears_clean_code() {
    let outside = crate_src().join("cli/tags.rs");
    let inside = crate_src().join("cli/git/mod.rs");
    for spelling in [
        "use std::process::Command;\nCommand::new(\"git\");",
        "use std::process::{Command, Stdio};\nCommand::new(\"git\");",
        "use std::process::Command as Proc;\nProc::new(\"git\");",
        "use std::process as pr;\npr::Command::new(\"git\");",
        "use std::{process::Command as Proc};\nProc::new(\"git\");",
        "use std::{process as pr};\npr::Command::new(\"git\");",
        "use ::std::process::Command as Proc;\nProc::new(\"git\");",
        "use std::process::r#Command as Proc;\nProc::new(\"git\");",
        "use std::process::*;\nCommand::new(\"git\");",
        "let out = std::process::Command::new(program).output();",
        // A `//` inside a string must not blind the rest of the line.
        "let (_sep, cmd) = (\"//\", std::process::Command::new(\"git\"));",
        "/* a block comment */\nstd::process::Command::new(\"git\");",
        // An unpaired quote in a character literal must not blind what follows.
        "fn q(c: char) -> bool { c == '\"' }\nstd::process::Command::new(\"git\");",
        "fn q(b: u8) -> bool { b == b'\"' }\nstd::process::Command::new(\"git\");",
        "fn q(c: char) -> bool { c == '\\'' }\nstd::process::Command::new(\"git\");",
    ] {
        assert!(
            violates(&outside, spelling),
            "should be flagged: {spelling}"
        );
        assert!(
            !violates(&inside, spelling),
            "the git module is exempt: {spelling}"
        );
    }

    for innocent in [
        "let id = std::process::id();",
        "std::process::exit(1);",
        "/// Never reach for std::process::Command here; name a `git::Op`.",
        "let note = 1; // Command::new( is what this forbids",
        "let url = \"https://example.invalid\";",
        "let pattern = r#\"a raw string mentioning nothing\"#;",
        "pub(crate) use preprocess::Trimmed;",
        "fn process_commits() {}",
        "let c = clap::Command::new(\"oakum\");",
        "/* std::process::Command must not be used here */",
        "let msg = \"std::process::Command is forbidden\";",
        "fn borrow<'a>(x: &'a str) -> &'a str { x }",
    ] {
        assert!(
            !violates(&outside, innocent),
            "should not be flagged: {innocent}"
        );
    }
}
