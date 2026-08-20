// `crates/plan-no-std` is what makes ADR-0024 a build failure rather than a
// promise. Its failure mode is silence — the crate attribute, the module
// declaration, or the workspace membership can all go away and it compiles an
// empty crate, exits 0, and proves nothing while the workspace stays green.
//
// Same shape as the disarmed-denylist tests in `io_boundary.rs`, and the same
// answer: assert the probe is still armed. This checks the wiring only,
// never that `plan` is clean — that is the probe's own job, and it does it at
// compile time.
//
// Substring scans are not enough on their own. Commenting a line out, or a doc
// comment that merely quotes it, satisfies `contains` while disabling the thing
// it names, so everything here reads comment-stripped source.
#![allow(clippy::disallowed_methods)]

mod support;

const PROBE: &str = "crates/plan-no-std";
/// Spelled out rather than split from `PROBE`, so a directory rename that leaves
/// the package name behind cannot slip through.
const PROBE_PACKAGE: &str = "plan-no-std";
const PLAN_ROOT: &str = "crates/oakum/src/plan/mod.rs";
/// The shape `plan` must not revert to: a single file, which the `#[path]` would
/// mount just as happily while covering nothing beside it.
const PLAN_SINGLE_FILE: &str = "crates/oakum/src/plan.rs";

/// Every line the probe needs, and nothing else. Equality rather than
/// containment: an added `#[cfg(any())]` disarms it just as thoroughly as a
/// deletion, and only a whole-file comparison sees both.
const PROBE_CODE: [&str; 4] = [
    "#![no_std]",
    "extern crate alloc;",
    "#[path = \"../../oakum/src/plan/mod.rs\"]",
    "pub mod plan;",
];

fn read(relative: &str) -> String {
    std::fs::read_to_string(support::workspace_root().join(relative))
        .unwrap_or_else(|e| panic!("{relative} should be readable: {e}"))
}

#[test]
fn the_probe_is_exactly_the_lines_that_arm_it() {
    let code = support::code_lines(&read(&format!("{PROBE}/src/lib.rs")), "//");

    assert_eq!(
        code, PROBE_CODE,
        "the probe's code no longer matches what arms it. Changing it is fine; \
         changing it without updating PROBE_CODE is how it goes quiet."
    );
}

/// The `#[path]` is checked against the real module rather than trusted: if
/// `plan` reverted to a single file, the probe would compile that file alone and
/// every submodule would drop out of coverage silently.
#[test]
fn the_probe_compiles_the_whole_plan_module() {
    let declared = PROBE_CODE[2]
        .split_once("#[path = \"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(path, _)| path)
        .expect("PROBE_CODE should carry a #[path]");

    // Each side canonicalizes or panics. Comparing two `Option`s lets a state
    // where neither path exists — a rename that updated the constants and the
    // `#[path]` together but never moved the directory — pass as `None == None`.
    let root = support::workspace_root();
    let resolved = root
        .join(PROBE)
        .join("src")
        .join(declared)
        .canonicalize()
        .unwrap_or_else(|e| {
            panic!("the probe's #[path] names {declared}, which does not exist: {e}")
        });
    let plan = root
        .join(PLAN_ROOT)
        .canonicalize()
        .unwrap_or_else(|e| panic!("{PLAN_ROOT} does not exist, so the probe covers nothing: {e}"));
    assert_eq!(
        resolved, plan,
        "the probe compiles {declared}, not {PLAN_ROOT}"
    );
    assert!(
        !root.join(PLAN_SINGLE_FILE).exists(),
        "{PLAN_SINGLE_FILE} is back, so `plan` is a single file and the probe \
         covers only that file rather than every module beside {PLAN_ROOT}"
    );
}

/// The probe compiles whatever `PLAN_ROOT` names, whether or not `oakum` still
/// mounts it. Drop the declaration — or gate it behind `#[cfg(any())]`, or move
/// the real planner elsewhere and leave a stub — and the probe holds a module the
/// crate no longer ships to a standard nothing depends on.
///
/// Resolved by the compiler rather than scanned for: an attribute, a block
/// comment, or a re-declaration inside a dead module all leave `pub mod plan;`
/// in `crates/oakum/src/lib.rs` intact while `oakum` stops containing the
/// module. Only path resolution sees that.
#[expect(
    unused_imports,
    reason = "importing it is the assertion; nothing here needs to use it"
)]
use oakum::plan;

/// Parsed, not scanned: a `members` entry commented out leaves text a `contains`
/// still matches while `--workspace` builds nothing.
///
/// `exclude` is deliberately not checked. Measured on cargo 1.97.1, 2026-08-20:
/// a literal `members` entry outranks an `exclude` naming the same path, so the
/// two together still build the probe and refusing that pairing would fail on a
/// manifest that works. `exclude` bites only against a glob `members` list,
/// which `support::workspace` refuses before any test here runs.
#[test]
fn the_probe_is_a_workspace_member() {
    let (root, members) = support::workspace();
    assert!(
        members.contains(&root.join(PROBE)),
        "{PROBE} is not a workspace member, so --workspace never builds it"
    );

    let probe: toml::Value = toml::from_str(&read(&format!("{PROBE}/Cargo.toml")))
        .expect("the probe manifest should parse");
    assert_eq!(
        probe["package"]["name"].as_str(),
        Some(PROBE_PACKAGE),
        "PROBE_PACKAGE no longer names the probe's package, so every check keyed \
         on it — here and the --exclude in .mise.toml — guards a string that \
         appears nowhere"
    );
    // `[lib] path` redirects the target away from the file PROBE_CODE audits,
    // which leaves every assertion above passing against source cargo no longer
    // compiles.
    if let Some(declared) = probe.get("lib").and_then(|lib| lib.get("path")) {
        let declared = declared
            .as_str()
            .unwrap_or_else(|| panic!("{PROBE}'s [lib] path is not a string: {declared:?}"));
        let target = root.join(PROBE).join(declared);
        let audited = root
            .join(PROBE)
            .join("src/lib.rs")
            .canonicalize()
            .unwrap_or_else(|e| panic!("{PROBE}/src/lib.rs should exist: {e}"));
        // Compared after canonicalizing, so `./src/lib.rs` reads as the same file
        // cargo compiles rather than as a redirect.
        assert_eq!(
            target.canonicalize().ok(),
            Some(audited),
            "{PROBE} points its lib target at {declared}, so PROBE_CODE audits a \
             file that is no longer compiled"
        );
    }
}

/// `mise run check` is the arming invocation: `--workspace` reaches the probe and
/// `--all-targets` builds its test target, which is what holds `plan`'s own
/// `#[cfg(test)]` module to `no_std` despite the probe's `[lib] test = false`.
///
/// `mise run test`'s `--doc` pass compiles the probe without arming it, so this
/// asserts the flags rather than the count.
#[test]
fn the_task_that_builds_the_probe_asks_for_every_target_in_the_workspace() {
    let mise: toml::Value = toml::from_str(&read(".mise.toml")).expect(".mise.toml should parse");

    let task = "check";
    let command = "cargo clippy";
    let lines = support::task_commands(&mise, task);

    // Every matching line, not the first: a task that runs a command twice —
    // one broad pass and one narrowed — would hide the narrowed one.
    let invocations: Vec<&&str> = lines.iter().filter(|line| line.contains(command)).collect();
    assert!(
        !invocations.is_empty(),
        "[tasks.{task}] no longer runs `{command}`"
    );

    for invocation in invocations {
        // `--all-targets` is the one that arms the probe; from the virtual
        // root `--workspace` only restates a selection cargo already makes,
        // and is pinned so the invocation cannot come to depend on the
        // directory mise happens to run it from.
        assert!(
            invocation.contains("--all-targets"),
            "[tasks.{task}] runs `{command}` without --all-targets, so it no \
             longer builds {PROBE}'s test target and `plan`'s own `#[cfg(test)]` \
             module escapes `no_std`:\n{invocation}"
        );
        assert!(
            invocation.contains("--workspace"),
            "[tasks.{task}] runs `{command}` without --workspace, so its selection \
             depends on the working directory:\n{invocation}"
        );
        // `--exclude` after `--workspace` drops the probe. Tokenised and split
        // on `=`, because `--exclude=plan-no-std` and a double space both
        // evade a substring match while disarming just as completely.
        // `-p`/`--package` cannot disarm while `--workspace` is present —
        // that flag wins — so they are refused only to keep the invocation
        // one obvious shape.
        for arg in invocation.split_whitespace() {
            let flag = arg.split('=').next().unwrap_or(arg);
            assert!(
                !matches!(flag, "--exclude" | "-p" | "--package"),
                "[tasks.{task}] narrows the selection with `{arg}`, so {PROBE} is \
                 never built:\n{invocation}"
            );
        }
    }
}

/// `code_lines` is the one thing `PROBE_CODE`'s equality check reads through, so a
/// stripper that mangles either direction turns that comparison into a check on
/// the wrong text. Pin both directions here.
#[test]
fn the_stripper_sees_through_comments_in_both_directions() {
    let probe = "//! Once declared `#![no_std]`.\n\
                 #![no_std]\n\
                 // pub mod plan;\n\
                 pub mod plan; // mounted\n";

    assert_eq!(
        support::code_lines(probe, "//"),
        ["#![no_std]", "pub mod plan;"],
        "the stripper kept a comment or ate real code"
    );
}

/// The direction the stripper does not handle. `PROBE_CODE`'s whole-file
/// equality fails closed on a block comment without help, so this refusal is
/// what keeps the helper safe for a caller that checks containment instead.
#[test]
#[should_panic(expected = "block comment")]
fn the_stripper_refuses_source_it_cannot_strip() {
    support::code_lines("/*\npub mod plan;\n*/\n", "//");
}
