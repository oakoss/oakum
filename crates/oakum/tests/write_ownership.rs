//! Write ownership for shipped commands (okm-aib, ADR-0003).

#![allow(clippy::disallowed_methods)]

mod support;

use std::fs;
use std::path::PathBuf;

use support::fixture::{git, git_repo, oakum, plain_repo, Fixture};
use support::repo_state::RepoState;

const BINARY_VERSION: &str = env!("CARGO_PKG_VERSION");

fn versioned(rest: &str) -> String {
    format!("tool-version = \"{BINARY_VERSION}\"\n{rest}")
}

fn cargo_package(root: &std::path::Path, name: &str) {
    fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n"
        ),
    )
    .expect("Cargo.toml");
    fs::create_dir_all(root.join("src")).expect("src");
    fs::write(root.join("src/lib.rs"), "").expect("lib.rs");
}

fn git_repo_with_package(label: &str) -> Fixture {
    let root = git_repo("write-ownership", label);
    cargo_package(&root, "demo");
    git(&root, &["add", "."]);
    git(&root, &["commit", "--no-verify", "-m", "chore: initial"]);
    root
}

fn fake_git_repo(label: &str) -> Fixture {
    let root = plain_repo("write-ownership", label);
    fs::create_dir(root.join(".git")).expect(".git");
    root
}

fn write_config(root: &std::path::Path, body: &str) {
    fs::create_dir_all(root.join(".changeset")).expect("changeset dir");
    fs::write(root.join(".changeset/_config.toml"), body).expect("config");
}

fn write_patch_changeset(root: &std::path::Path) {
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/one.md"),
        "---\ndemo: patch\n---\n\npatch demo\n",
    )
    .expect("changeset");
}

fn run_ok(root: &Fixture, args: &[&str]) {
    let output = oakum(root).args(args).output().expect("oakum");
    assert!(
        output.status.success(),
        "oakum {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn check_leaves_repository_state_unchanged() {
    let root = git_repo_with_package("check");
    git(&root, &["tag", "v0.1.0"]);
    let before = RepoState::capture(&root);
    run_ok(&root, &["check"]);
    RepoState::assert_unchanged(&before, &root, "check");
}

#[test]
fn status_leaves_repository_state_unchanged() {
    let root = fake_git_repo("status");
    cargo_package(&root, "demo");
    write_patch_changeset(&root);
    let before = RepoState::capture(&root);
    run_ok(&root, &["status", "--json"]);
    RepoState::assert_unchanged(&before, &root, "status --json");
}

#[test]
fn hidden_read_commands_leave_repository_state_unchanged() {
    let root = git_repo_with_package("hidden-read");
    write_config(
        &root,
        &versioned("change-files = true\nconventional-commits = true\n"),
    );
    write_patch_changeset(&root);
    git(&root, &["add", "."]);
    git(&root, &["commit", "--no-verify", "-m", "chore: config"]);

    let before = RepoState::capture(&root);
    run_ok(&root, &["plan-intent", "--from", "HEAD~1"]);
    run_ok(&root, &["detect-release-tools"]);
    run_ok(&root, &["reachable-tags"]);
    RepoState::assert_unchanged(&before, &root, "hidden read commands");
}

#[test]
fn generate_dry_run_leaves_repository_state_unchanged() {
    let root = git_repo_with_package("generate-dry");
    write_config(
        &root,
        &versioned("change-files = true\nconventional-commits = true\n"),
    );
    git(&root, &["add", "."]);
    git(&root, &["commit", "--no-verify", "-m", "chore: config"]);
    let base = support::fixture::git_stdout(&root, &["rev-parse", "HEAD"]);

    fs::write(root.join("src/lib.rs"), "// change\n").expect("edit");
    git(&root, &["add", "."]);
    git(&root, &["commit", "--no-verify", "-m", "fix(demo): bug"]);

    let before = RepoState::capture(&root);
    run_ok(
        &root,
        &[
            "generate",
            "--from",
            &base,
            "--dry-run",
            "--name",
            "ignored",
        ],
    );
    RepoState::assert_unchanged(&before, &root, "generate --dry-run");
}

#[test]
fn add_writes_only_the_named_bump_file() {
    let root = fake_git_repo("add");
    cargo_package(&root, "demo");
    let before = RepoState::capture(&root);

    run_ok(
        &root,
        &[
            "add",
            "--packages",
            "demo:minor",
            "--message",
            "Adds the add command.",
            "--name",
            "adds-add",
        ],
    );

    RepoState::assert_only_new_files(
        &before,
        &root,
        &[PathBuf::from(".changeset/adds-add.md")],
        "add",
    );
}

#[test]
fn generate_writes_only_the_named_bump_file() {
    let root = git_repo_with_package("generate-write");
    write_config(
        &root,
        &versioned("change-files = true\nconventional-commits = true\n"),
    );
    git(&root, &["add", "."]);
    git(&root, &["commit", "--no-verify", "-m", "chore: config"]);
    let base = support::fixture::git_stdout(&root, &["rev-parse", "HEAD"]);

    fs::write(root.join("src/lib.rs"), "// change\n").expect("edit");
    git(&root, &["add", "."]);
    git(&root, &["commit", "--no-verify", "-m", "fix(demo): bug"]);

    let before = RepoState::capture(&root);
    run_ok(
        &root,
        &["generate", "--from", &base, "--name", "from-commits"],
    );
    RepoState::assert_only_new_files(
        &before,
        &root,
        &[PathBuf::from(".changeset/from-commits.md")],
        "generate",
    );
}
