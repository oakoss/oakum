//! `oakum tag-drift`: manifest above the highest reachable tag.

#![allow(clippy::disallowed_methods)]

mod support;

use std::fs;
use std::path::Path;

use support::fixture::{git, git_repo, oakum, Fixture};

fn temp_git_repo(label: &str) -> Fixture {
    git_repo("tag-drift", label)
}

fn cargo_package(root: &Path, name: &str, version: &str) {
    fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2021\"\n\n[workspace]\n"
        ),
    )
    .expect("Cargo.toml");
    fs::create_dir_all(root.join("src")).expect("src");
    fs::write(root.join("src/lib.rs"), "").expect("lib.rs");
}

fn commit(root: &Path, message: &str) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "--no-verify", "-m", message]);
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
    let (ok, stdout, stderr) = drift(&dest);
    assert!(!ok, "shallow clone must not look like never-released");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("shallow"), "{stderr}");
}
