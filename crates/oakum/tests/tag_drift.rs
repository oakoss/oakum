//! `oakum tag-drift`: manifest above the highest reachable tag.

#![allow(clippy::disallowed_methods)]

mod support;

use std::path::Path;

use support::fixture::oakum_exit;
use support::fixture::{
    cargo_package, commit, git, git_repo, oakum, versioned, write_config, write_install_pin,
    Fixture,
};
#[cfg(unix)]
use support::fixture::{path_prefixed_by, path_shim};

fn temp_git_repo(label: &str) -> Fixture {
    git_repo("tag-drift", label)
}

fn drift(root: &Path) -> (bool, String, String) {
    let out = oakum(root).arg("tag-drift").output().expect("oakum");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn matching_manifest_is_clean() {
    let root = temp_git_repo("match");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    let (ok, stdout, stderr) = drift(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn manifest_above_tag_is_drift() {
    let root = temp_git_repo("above");
    cargo_package(&root, "demo", "0.2.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    let (ok, stdout, stderr) = drift(&root);
    assert!(!ok, "expected drift");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("0.2.0"), "{stderr}");
    assert!(stderr.contains("0.1.0"), "{stderr}");
    assert!(stderr.contains("demo"), "{stderr}");
}

/// The same block `check` prints for the same state: the summary first, the
/// detail indented beneath it.
#[test]
fn drift_is_one_block_summary_first() {
    let root = temp_git_repo("block");
    cargo_package(&root, "demo", "0.2.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    let (_, _, stderr) = drift(&root);
    let lines: Vec<&str> = stderr.lines().collect();
    assert_eq!(
        lines[0], "error: 1 package(s) bumped without a tag",
        "{stderr}"
    );
    assert!(
        lines[1].starts_with("  demo (cargo): manifest 0.2.0 is above tagged 0.1.0"),
        "{stderr}"
    );
    assert_eq!(lines.len(), 2, "{stderr}");
}

/// The pin is a look here too, after the tags: a stale pin refuses, and a
/// git that cannot run is what a reader meets first.
#[test]
fn a_stale_install_pin_refuses_after_the_tags_answer() {
    let root = temp_git_repo("stale-pin");
    cargo_package(&root, "demo", "0.2.0");
    write_config(&root, &versioned(""));
    write_install_pin(&root, "99999.0.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    let (code, _, stderr) = oakum_exit(&root, &["tag-drift"]);
    assert_eq!(code, Some(1), "the drift finding decides: {stderr}");
    assert_eq!(
        stderr.lines().next(),
        Some("error: 1 package(s) bumped without a tag"),
        "{stderr}"
    );
    assert!(
        stderr.contains("also unverified: install pin is 99999.0.0"),
        "the stale pin is still reported, subordinate: {stderr}"
    );
    assert_eq!(
        stderr.lines().count(),
        3,
        "the verdict, its one detail line, and a bare `also`: {stderr}"
    );
}

/// The same tie-break as `check`: with git wholly unusable, that is what a
/// reader meets first, and the stale pin follows as `also`.
#[cfg(unix)]
#[test]
fn a_git_that_cannot_run_outranks_a_stale_install_pin() {
    let root = temp_git_repo("dead-git-and-pin");
    cargo_package(&root, "demo", "0.1.0");
    write_config(&root, &versioned(""));
    write_install_pin(&root, "99999.0.0");
    commit(&root, "init");
    let shim_dir = path_shim(
        &root,
        "git",
        "#!/bin/sh\necho 'fatal: git is not working today' >&2\nexit 128\n",
    );
    let path = path_prefixed_by(&shim_dir);
    let out = oakum(&root)
        .args(["tag-drift"])
        .env("PATH", &path)
        .output()
        .expect("oakum");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    let first = stderr.lines().next().unwrap_or_default();
    assert!(
        first.starts_with("unverified: ") && first.contains("git is not working today"),
        "the dead git decides: {stderr}"
    );
    assert!(
        stderr.contains("also unverified: install pin is 99999.0.0"),
        "{stderr}"
    );
}

#[test]
fn later_untagged_bump_is_drift() {
    let root = temp_git_repo("later");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    cargo_package(&root, "demo", "0.2.0");
    commit(&root, "bump");
    let (ok, stdout, stderr) = drift(&root);
    assert!(!ok, "expected drift after untagged bump");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("0.2.0"), "{stderr}");
    assert!(stderr.contains("0.1.0"), "{stderr}");
    assert!(stderr.contains("demo"), "{stderr}");
}

#[test]
fn two_tagged_releases_matching_latest_is_clean() {
    let root = temp_git_repo("two");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    cargo_package(&root, "demo", "0.2.0");
    commit(&root, "release");
    git(&root, &["tag", "v0.2.0"]);
    let (ok, stdout, stderr) = drift(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn leftover_tag_is_unverified() {
    let root = temp_git_repo("leftover");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "other-v1.0.0"]);
    let (ok, stdout, stderr) = drift(&root);
    assert!(!ok, "leftover must not look clean");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("other-v1.0.0"), "{stderr}");
}

#[test]
fn leftover_after_a_real_tag_is_unverified() {
    let root = temp_git_repo("mixed-leftover");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    git(&root, &["tag", "other-v1.0.0"]);
    let (ok, stdout, stderr) = drift(&root);
    assert!(!ok, "leftover next to a real tag must not look clean");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("other-v1.0.0"), "{stderr}");
}

#[test]
fn tag_on_another_branch_does_not_hide_drift() {
    let root = temp_git_repo("other-branch");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    git(&root, &["checkout", "-b", "release"]);
    cargo_package(&root, "demo", "0.2.0");
    commit(&root, "release");
    git(&root, &["tag", "v0.2.0"]);
    git(&root, &["checkout", "main"]);
    cargo_package(&root, "demo", "0.2.0");
    commit(&root, "bump");
    let (ok, stdout, stderr) = drift(&root);
    assert!(!ok, "other-branch tag must not hide HEAD drift");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("0.1.0"), "{stderr}");
}

#[test]
fn from_a_subdirectory_still_discovers() {
    let root = temp_git_repo("subdir");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    let (ok, stdout, stderr) = {
        let out = oakum(&root.join("src"))
            .arg("tag-drift")
            .output()
            .expect("oakum");
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn no_tags_is_not_drift() {
    let root = temp_git_repo("bootstrap");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    let (ok, stdout, stderr) = drift(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn shallow_clone_is_unverified() {
    let src = temp_git_repo("shallow-src");
    cargo_package(&src, "demo", "0.1.0");
    commit(&src, "init");
    git(&src, &["tag", "v0.1.0"]);
    cargo_package(&src, "demo", "0.2.0");
    commit(&src, "later");
    let dest_name = "shallow";
    let mut parts = Path::new(dest_name).components();
    assert!(
        matches!(parts.next(), Some(std::path::Component::Normal(_))) && parts.next().is_none(),
        "clone dest name must be one path segment inside the container, got {dest_name:?}"
    );
    let dest = src.container().join(dest_name);
    git(
        &src,
        &[
            "-c",
            "protocol.file.allow=always",
            "clone",
            "--depth=1",
            "--no-local",
            src.to_str().expect("utf-8 path"),
            dest.to_str().expect("utf-8 dest"),
        ],
    );
    let (code, stdout, stderr) = oakum_exit(&dest, &["tag-drift"]);
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("shallow"), "{stderr}");
    assert_eq!(code, Some(2), "unverified is exit 2: {stderr}");
}
