// `probes/plan-no-std` is what makes ADR-0024 a build failure rather than a
// promise. Its failure mode is silence — the crate attribute, the module
// declaration, or the workspace membership can all go away and it compiles an
// empty crate, exits 0, and proves nothing while the workspace stays green.
//
// Same shape as the disarmed-denylist tests in `tests/io_boundary.rs`, and the
// same answer: assert the probe is still armed. This checks the wiring only,
// never that `plan` is clean — that is the probe's own job, and it does it at
// compile time.
//
// Substring scans are not enough on their own. Commenting a line out, or a doc
// comment that merely quotes it, satisfies `contains` while disabling the thing
// it names, so everything here reads comment-stripped source.
#![allow(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};

const PROBE: &str = "probes/plan-no-std";
/// Spelled out rather than split from `PROBE`, so a directory rename that leaves
/// the package name behind cannot slip through.
const PROBE_PACKAGE: &str = "plan-no-std";
const PLAN_ROOT: &str = "src/plan/mod.rs";

/// Every line the probe needs, and nothing else. Equality rather than
/// containment: an added `#[cfg(any())]` disarms it just as thoroughly as a
/// deletion, and only a whole-file comparison sees both.
const PROBE_CODE: [&str; 4] = [
    "#![no_std]",
    "extern crate alloc;",
    "#[path = \"../../../src/plan/mod.rs\"]",
    "pub mod plan;",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn read(relative: &str) -> String {
    std::fs::read_to_string(repo_root().join(relative))
        .unwrap_or_else(|e| panic!("{relative} should be readable: {e}"))
}

fn code_lines(source: &str, marker: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|line| {
            let code = line.split(marker).next().unwrap_or_default().trim();
            (!code.is_empty()).then(|| code.to_string())
        })
        .collect()
}

#[test]
fn the_probe_is_exactly_the_lines_that_arm_it() {
    let code = code_lines(&read(&format!("{PROBE}/src/lib.rs")), "//");

    assert_eq!(
        code, PROBE_CODE,
        "the probe's code no longer matches what arms it. Changing it is fine; \
         changing it without updating PROBE_CODE is how it goes quiet."
    );
}

/// The `#[path]` is checked against the real module rather than trusted: if
/// `plan` reverted to a single `src/plan.rs`, the probe would compile that file
/// alone and every submodule would drop out of coverage silently.
#[test]
fn the_probe_compiles_the_whole_plan_module() {
    let declared = PROBE_CODE[2]
        .split_once("#[path = \"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(path, _)| path)
        .expect("PROBE_CODE should carry a #[path]");

    let resolved = repo_root().join(PROBE).join("src").join(declared);
    assert_eq!(
        resolved.canonicalize().ok(),
        repo_root().join(PLAN_ROOT).canonicalize().ok(),
        "the probe compiles {declared}, not {PLAN_ROOT}"
    );
    assert!(
        !repo_root().join("src/plan.rs").exists(),
        "src/plan.rs is back, so `plan` is a single file and the probe covers \
         only that file rather than every module under src/plan/"
    );
}

/// Parsed, not scanned: `members = []` beside `exclude = ["probes/plan-no-std"]`
/// contains the same text and builds nothing.
#[test]
fn the_probe_is_a_workspace_member_and_not_excluded() {
    #[derive(serde::Deserialize)]
    struct Manifest {
        workspace: Workspace,
    }

    #[derive(serde::Deserialize)]
    struct Workspace {
        #[serde(default)]
        members: Vec<String>,
        #[serde(default)]
        exclude: Vec<String>,
    }

    let manifest: Manifest = toml::from_str(&read("Cargo.toml"))
        .unwrap_or_else(|e| panic!("Cargo.toml does not match the shape this test audits: {e}"));

    assert!(
        manifest.workspace.members.iter().any(|m| m == PROBE),
        "{PROBE} is not a workspace member, so --workspace never builds it"
    );
    assert!(
        !manifest.workspace.exclude.iter().any(|m| m == PROBE),
        "{PROBE} is excluded from the workspace, so --workspace skips it"
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
}

/// These two are the arming invocations: `--workspace` reaches the probe, and
/// `--all-targets` builds its test target, which is what holds `plan`'s own
/// `#[cfg(test)]` module to `no_std` despite the probe's `[lib] test = false`.
///
/// `mise run test`'s `--doc` pass compiles the probe without arming it, so this
/// asserts the flags rather than the count.
#[test]
fn the_tasks_that_build_the_probe_ask_for_every_target_in_the_workspace() {
    let mise: toml::Value = toml::from_str(&read(".mise.toml")).expect(".mise.toml should parse");

    for (task, command) in [("check", "cargo clippy"), ("check-msrv", "cargo check")] {
        let run = &mise["tasks"][task]["run"];
        let lines: Vec<&str> = match run {
            toml::Value::String(one) => Vec::from([one.as_str()]),
            toml::Value::Array(many) => many.iter().filter_map(toml::Value::as_str).collect(),
            other => panic!("[tasks.{task}].run is neither a string nor an array: {other:?}"),
        };

        // Every matching line, not the first: a task that runs a command twice —
        // one broad pass and one narrowed — would hide the narrowed one.
        let invocations: Vec<&&str> = lines.iter().filter(|line| line.contains(command)).collect();
        assert!(
            !invocations.is_empty(),
            "[tasks.{task}] no longer runs `{command}`"
        );

        for invocation in invocations {
            for flag in ["--workspace", "--all-targets"] {
                assert!(
                    invocation.contains(flag),
                    "[tasks.{task}] runs `{command}` without {flag}, so it no longer \
                     builds {PROBE} over every target:\n{invocation}"
                );
            }
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
                    "[tasks.{task}] narrows the selection with `{arg}`, so \
                     {PROBE} is never built:\n{invocation}"
                );
            }
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
        code_lines(probe, "//"),
        ["#![no_std]", "pub mod plan;"],
        "the stripper kept a comment or ate real code"
    );
}
