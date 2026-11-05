//! `oakum check` is the shared readiness path (ADR-0020).

mod changelog;
mod coverage;
mod deadline;
mod install_pin;
mod intent;
mod management;
mod remote;
mod report;
mod schema;
mod strict;
mod tag_store;
mod tags;
mod transport;

use crate::support;

use std::fs;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Command;

use httpmock::prelude::*;
use support::fixture::oakum_exit;
use support::fixture::{
    cargo_package, commit, git, git_repo, head_sha, oakum, oakum_output, sibling, Fixture,
};
#[cfg(unix)]
use support::fixture::{deadline_turn, output_within};
use support::repo_files::{write_config, write_install_pin, BINARY_VERSION, TOOL_VERSION_WARNING};
#[cfg(unix)]
use support::tools::{
    ambient_tool, install_executable, path_prefixed_by, path_shim, recording_fake_ssh,
};

/// A pin that can never equal the binary's version, so a fixture that pins it
/// beside `BINARY_VERSION` stays a disagreement at every release. Measured:
/// literal `1.2.3` collided with the binary's version once it reached 1.2.3.
const MISMATCHED_PIN: &str = "99999.0.0";

fn temp_git_repo(label: &str) -> Fixture {
    git_repo("check", label)
}

/// A package a registry would refuse, which is what makes `private-packages`
/// decide whether oakum manages it.
fn private_npm_package(root: &Path, name: &str, version: &str) {
    std::fs::write(
        root.join("package.json"),
        format!(
            "{{\n  \"name\": \"{name}\",\n  \"version\": \"{version}\",\n  \"private\": true\n}}\n"
        ),
    )
    .expect("package.json");
}

fn write_workflow_pin(root: &Path, name: &str, body: &str) {
    fs::create_dir_all(root.join(".github/workflows")).expect("workflows");
    fs::write(root.join(".github/workflows").join(name), body).expect("workflow");
}

/// A second workflow pin at [`MISMATCHED_PIN`], beside the `release.yml` one
/// [`write_install_pin`] writes, so the pins disagree.
fn write_stale_pin(root: &Path) {
    write_workflow_pin(
        root,
        "ci.yml",
        &format!("run: cargo binstall --no-confirm oakum@{MISMATCHED_PIN}\n"),
    );
}

fn write_pinned_config(root: &Path, version: &str, extra: &str) {
    write_config(root, extra);
    write_install_pin(root, version);
}

fn check(root: &Path) -> (bool, String, String) {
    checked(root, &["check"])
}

/// `check` names what it covers on every run that gets as far as reading a
/// config (`okm-404.11`), before any look runs, and accounts for the coverage
/// look after. These fixtures assert on what a run says *besides* those, so
/// both are stripped here and asserted on directly by the tests about them.
fn checked(root: &Path, args: &[&str]) -> (bool, String, String) {
    let (ok, stdout, stderr) = oakum_output(root, args);
    (
        ok,
        without_the_coverage(&without_the_scope(&stdout)),
        stderr,
    )
}

fn without_the_coverage(stdout: &str) -> String {
    stdout
        .lines()
        .filter(|line| !line.starts_with("coverage: "))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The report is the first two lines; what follows is the test's business.
///
/// The count is asserted rather than tolerated: an `is_empty()` escape hatch
/// here let every downstream `stdout.is_empty()` pass on a run that printed no
/// report at all, which is ~79 assertions contributing nothing (measured).
fn without_the_scope(stdout: &str) -> String {
    let rest: Vec<&str> = stdout
        .lines()
        .skip_while(|line| line.starts_with("check: "))
        .collect();
    assert_eq!(
        stdout.lines().count() - rest.len(),
        2,
        "a run that reaches a look opens with the two-line report; got: {stdout}"
    );
    rest.join("\n")
}

/// A run that refuses before the config is read has nothing to report yet, so
/// it prints no report — the other half of the same claim, and the half
/// [`without_the_scope`] cannot make.
fn refused_before_the_report(root: &Path, args: &[&str]) -> (bool, String, String) {
    let (ok, stdout, stderr) = oakum_output(root, args);
    assert!(
        !stdout.starts_with("check: "),
        "nothing is known to report before the config is read: {stdout}"
    );
    (ok, stdout, stderr)
}

fn cargo_members(root: &Path, members: &[(&str, &str)]) {
    let listed: Vec<String> = members
        .iter()
        .map(|(name, _)| format!("\"{name}\""))
        .collect();
    fs::write(
        root.join("Cargo.toml"),
        format!(
            "[workspace]\nresolver = \"2\"\nmembers = [{}]\n",
            listed.join(", ")
        ),
    )
    .expect("workspace");
    for (name, version) in members {
        let dir = root.join(name);
        fs::create_dir_all(dir.join("src")).expect("src");
        fs::write(
            dir.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2021\"\n"),
        )
        .expect("member Cargo.toml");
        fs::write(dir.join("src/lib.rs"), "").expect("lib.rs");
    }
}

fn write_members(root: &Path, members: &[(&str, &str)]) {
    let names = members
        .iter()
        .map(|(name, _)| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(", ");
    fs::write(
        root.join("Cargo.toml"),
        format!("[workspace]\nresolver = \"2\"\nmembers = [{names}]\n"),
    )
    .expect("workspace Cargo.toml");
    for (name, version) in members {
        fs::create_dir_all(root.join(name).join("src")).expect("member src");
        fs::write(
            root.join(name).join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2021\"\n"),
        )
        .expect("member Cargo.toml");
        fs::write(root.join(name).join("src/lib.rs"), "").expect("member lib.rs");
    }
}

/// The workspace okm-404.26 records: `alpha` publishable, `beta` not, and a
/// config stating neither `private-packages` nor `include`/`exclude`. The
/// config-level management gate passes here because `alpha` is managed, which
/// is why `beta` needed a look of its own.
fn mixed_workspace(label: &str, config_extra: &str) -> Fixture {
    let root = temp_git_repo(label);
    write_pinned_config(&root, BINARY_VERSION, config_extra);
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"alpha\", \"beta\"]\n",
    )
    .expect("workspace");
    for (name, extra) in [("alpha", ""), ("beta", "publish = false\n")] {
        let path = root.join(name);
        fs::create_dir_all(path.join("src")).expect("src");
        fs::write(
            path.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{extra}"
            ),
        )
        .expect("member Cargo.toml");
        fs::write(path.join("src/lib.rs"), "").expect("lib.rs");
    }
    commit(&root, "init");
    root
}

fn repo_with_followup_change(label: &str) -> Fixture {
    let root = temp_git_repo(label);
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::write(root.join("src/lib.rs"), "// changed\n").expect("edit");
    commit(&root, "fix: touch demo");
    root
}

fn clone_of(origin: &Fixture, label: &str) -> PathBuf {
    let dest = sibling(origin, label);
    let src_s = origin.to_str().expect("utf-8 path");
    let dest_s = dest.to_str().expect("utf-8 dest");
    // Remote-lookback needs a normal clone (hardlinks), not `--no-local`.
    git(origin, &["clone", src_s, dest_s]);
    dest
}

fn tagged_cargo(label: &str, versions: &[&str]) -> Fixture {
    let root = temp_git_repo(label);
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", versions[0]);
    commit(&root, "init");
    git(
        &root,
        &["tag", "-a", &format!("v{}", versions[0]), "-m", versions[0]],
    );
    for version in &versions[1..] {
        cargo_package(&root, "demo", version);
        commit(&root, version);
        git(&root, &["tag", "-a", &format!("v{version}"), "-m", version]);
    }
    root
}

/// A `git` that passes everything through except one subcommand, so a single
/// operation can be made to fail while the rest of the run proceeds normally.
#[cfg(unix)]
fn git_shim(root: &Fixture, matches: &str, script: &str) -> PathBuf {
    path_shim(
        root,
        "git",
        format!(
            "#!/bin/sh\ncase \"$*\" in\n  {matches}) {script} ;;\nesac\nexec {} \"$@\"\n",
            ambient_tool("git").display()
        ),
    )
}

#[cfg(unix)]
fn with_shim(root: &Path, dir: &Path, args: &[&str]) -> (bool, String) {
    let out = oakum(root)
        .args(args)
        .env("PATH", path_prefixed_by(dir))
        .output()
        .expect("oakum");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}
