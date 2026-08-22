//! `oakum tag-drift`: manifest above the highest reachable tag.

#![allow(clippy::disallowed_methods)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oakum"))
}

fn git(root: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
        .args(args)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .status()
        .expect("git");
    assert!(status.success(), "git {args:?} failed");
}

fn temp_git_repo(label: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("oakum-tag-drift-{label}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp repo");
    git(&dir, &["init", "-b", "main"]);
    git(&dir, &["config", "user.email", "test@example.com"]);
    git(&dir, &["config", "user.name", "Test"]);
    let hooks = dir.join("no-hooks");
    fs::create_dir(&hooks).expect("no-hooks");
    git(&dir, &["config", "core.hooksPath", "no-hooks"]);
    dir
}

fn cargo_package(root: &std::path::Path, name: &str, version: &str) {
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

fn commit(root: &std::path::Path, message: &str) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "--no-verify", "-m", message]);
}

fn drift(root: &std::path::Path) -> (bool, String, String) {
    let out = bin()
        .arg("tag-drift")
        .current_dir(root)
        .output()
        .expect("oakum");
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
        let out = bin()
            .arg("tag-drift")
            .current_dir(root.join("src"))
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
    let dest = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("oakum-tag-drift-shallow-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dest);
    let status = Command::new("git")
        .args([
            "-c",
            "protocol.file.allow=always",
            "clone",
            "--depth=1",
            "--no-local",
            src.to_str().expect("utf-8 path"),
            dest.to_str().expect("utf-8 dest"),
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .status()
        .expect("git clone");
    assert!(status.success(), "git clone --depth=1 failed");
    let (ok, stdout, stderr) = drift(&dest);
    assert!(!ok, "shallow clone must not look like never-released");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("shallow"), "{stderr}");
}
