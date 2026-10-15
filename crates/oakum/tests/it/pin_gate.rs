//! The write gate across the writers: a repository with no oakum install pin
//! is refused as `unverified`, naming the cheapest pin for it (ADR-0038).

use crate::support;

use std::fs;

use support::fixture::{
    cargo_package, commit, git, git_repo, git_stdout, oakum, Fixture, BINARY_VERSION,
};

const WRITERS: [&[&str]; 4] = [
    &["add", "--packages", "demo:patch", "--message", "x"],
    &["generate", "--from", "HEAD~1"],
    &["version"],
    &["ci", "version-pr"],
];

/// Committed, so a refusal that wrote anything shows up in `git status`. A
/// pending bump file and a scoped `fix` commit give each writer something to
/// write.
fn unpinned(label: &str, npm: bool) -> Fixture {
    let root = git_repo("pin-gate", label);
    if npm {
        fs::write(
            root.join("package.json"),
            "{\"name\": \"demo\", \"version\": \"0.1.0\"}\n",
        )
        .expect("package.json");
    } else {
        cargo_package(&root, "demo", "0.1.0");
    }
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    // A `tool-version` naming this binary must not stand in for the pin.
    fs::write(
        root.join(".changeset/_config.toml"),
        format!("tool-version = \"{BINARY_VERSION}\"\n"),
    )
    .expect("config");
    fs::write(
        root.join(".changeset/one.md"),
        "---\ndemo: patch\n---\n\npatch demo\n",
    )
    .expect("bump file");
    commit(&root, "chore: init");
    fs::write(root.join("notes.txt"), "x\n").expect("notes");
    commit(&root, "fix(demo): x");
    root
}

/// Every writer runs before any verdict, so one failure does not hide another.
fn writers_that_did_not_refuse(root: &Fixture, pin: &str) -> Vec<String> {
    let mut failures = Vec::new();
    for args in WRITERS {
        let output = oakum(root)
            .args(args)
            .env_remove("GITHUB_TOKEN")
            .env_remove("GH_TOKEN")
            .output()
            .expect("oakum");
        let stderr = String::from_utf8_lossy(&output.stderr);
        let refused = output.status.code() == Some(2)
            && stderr.contains("no oakum install pin")
            && stderr.contains(pin);
        git(root, &["add", "-A"]);
        let wrote = git_stdout(root, &["status", "--porcelain"]);
        if !refused || !wrote.is_empty() {
            failures.push(format!(
                "{args:?}: exit {:?}, wrote {wrote:?}: {stderr}",
                output.status.code()
            ));
            git(root, &["reset", "--hard", "--quiet"]);
        }
    }
    failures
}

/// A pin elsewhere must not let a dangling `.github/workflows` read as empty:
/// the workflows it hides could pin another version. A composite action holds
/// the pin rather than `.mise.toml`, which a mise shim in the fixture would read.
#[cfg(unix)]
#[test]
fn a_dangling_workflows_link_beside_a_pin_refuses_add() {
    let root = unpinned("dangling-workflows", false);
    fs::create_dir_all(root.join(".github/actions/setup")).expect("action dir");
    fs::write(
        root.join(".github/actions/setup/action.yml"),
        format!(
            "runs:\n  using: composite\n  steps:\n    - run: cargo binstall --no-confirm oakum@{BINARY_VERSION}\n      shell: bash\n"
        ),
    )
    .expect("action");
    std::os::unix::fs::symlink("nowhere", root.join(".github/workflows")).expect("dangling");
    let output = oakum(&root).args(WRITERS[0]).output().expect("oakum add");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("`.github/workflows` is a symlink to a missing target"),
        "{stderr}"
    );
    assert!(
        fs::read_dir(root.join(".changeset"))
            .expect("changeset")
            .count()
            == 2,
        "no bump file written"
    );
}

#[test]
fn no_pin_refuses_writers_naming_the_cheapest_pin() {
    let cargo = unpinned("cargo", false);
    let mut failures = writers_that_did_not_refuse(
        &cargo,
        &format!(
            "add `\"github:oakoss/oakum\" = \"{BINARY_VERSION}\"` under `[tools]` in `.mise.toml`"
        ),
    );
    let npm = unpinned("npm", true);
    failures.extend(writers_that_did_not_refuse(
        &npm,
        &format!("`pnpm add -D --allow-build=@oakoss/oakum @oakoss/oakum@{BINARY_VERSION}`"),
    ));
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
