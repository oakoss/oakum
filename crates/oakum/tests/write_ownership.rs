//! Write ownership for shipped commands (okm-aib, ADR-0003).

#![allow(clippy::disallowed_methods)]

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use httpmock::prelude::*;
use serde_json::json;
use support::fixture::{git, git_repo, git_stdout, oakum, plain_repo, Fixture};
use support::repo_state::RepoState;

const BINARY_VERSION: &str = env!("CARGO_PKG_VERSION");
const CHECKOUT_PIN: &str = "v9.9.9";

fn versioned(rest: &str) -> String {
    format!("tool-version = \"{BINARY_VERSION}\"\n{rest}")
}

fn mock_checkout_latest() -> MockServer {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/actions/checkout/releases/latest");
        then.status(200)
            .json_body(json!({ "tag_name": CHECKOUT_PIN }));
    });
    server
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

fn write_install_pin(root: &Path) {
    fs::create_dir_all(root.join(".github/workflows")).expect("workflows");
    fs::write(
        root.join(".github/workflows/ci.yml"),
        format!("run: cargo binstall --no-confirm oakum@{BINARY_VERSION}\n"),
    )
    .expect("workflow");
}

fn run_ok(root: &Fixture, args: &[&str]) {
    let output = oakum(root).args(args).output().expect("oakum");
    assert!(
        output.status.success(),
        "oakum {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_init(root: &Path) {
    let server = mock_checkout_latest();
    let output = oakum(root)
        .arg("init")
        .env("GITHUB_API_URL", server.base_url())
        .output()
        .expect("oakum init");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_migrate(root: &Path) {
    let server = mock_checkout_latest();
    let output = oakum(root)
        .arg("migrate")
        .env("GITHUB_API_URL", server.base_url())
        .output()
        .expect("oakum migrate");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn add_bare_origin(root: &Fixture) -> PathBuf {
    let remote = root.container().join("origin.git");
    git(root, &["init", "--bare", remote.to_str().expect("utf-8")]);
    git(
        root,
        &["remote", "add", "origin", remote.to_str().expect("utf-8")],
    );
    remote
}

#[test]
fn check_leaves_repository_state_unchanged() {
    let root = git_repo_with_package("check");
    write_config(
        &root,
        &versioned("change-files = true\nconventional-commits = true\n"),
    );
    write_install_pin(&root);
    git(&root, &["add", "."]);
    git(&root, &["commit", "--no-verify", "-m", "chore: config"]);
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

#[test]
fn init_writes_only_changeset_owned_files() {
    let root = fake_git_repo("init");
    let before = RepoState::capture(&root);
    run_init(&root);
    RepoState::assert_allowed_delta(
        &before,
        &root,
        &[
            PathBuf::from(".changeset/_config.toml"),
            PathBuf::from(".changeset/_schema.json"),
            PathBuf::from(".changeset/README.md"),
        ],
        &[],
        &[],
        &[],
        "init",
    );
}

#[test]
fn upgrade_writes_only_tool_version_and_schema() {
    let root = git_repo("write-ownership", "upgrade");
    write_config(
        &root,
        "# pinned by upgrade\ntool-version = \"999.0.0\" # note\nversioning = \"semver\"\n",
    );
    let before = RepoState::capture(&root);
    run_ok(&root, &["upgrade"]);
    RepoState::assert_allowed_delta(
        &before,
        &root,
        &[PathBuf::from(".changeset/_schema.json")],
        &[PathBuf::from(".changeset/_config.toml")],
        &[],
        &[],
        "upgrade",
    );
}

#[test]
fn migrate_writes_only_owned_paths() {
    let root = fake_git_repo("migrate");
    cargo_package(&root, "core");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\n\"core\": minor\n---\nnote\n",
    )
    .expect("bump");
    fs::write(
        root.join(".changeset/config.json"),
        r#"{"changelog": "@changesets/cli/changelog", "access": "public"}"#,
    )
    .expect("config");
    let before = RepoState::capture(&root);
    run_migrate(&root);
    RepoState::assert_allowed_delta(
        &before,
        &root,
        &[
            PathBuf::from(".changeset/_config.toml"),
            PathBuf::from(".changeset/_schema.json"),
            PathBuf::from(".changeset/README.md"),
        ],
        &[PathBuf::from(".changeset/feat.md")],
        &[],
        &[],
        "migrate",
    );
}

#[test]
fn version_writes_only_owned_paths() {
    let root = git_repo_with_package("version");
    write_config(&root, &versioned("change-files = true\n"));
    write_patch_changeset(&root);
    let before = RepoState::capture(&root);
    run_ok(&root, &["version"]);
    RepoState::assert_allowed_delta(
        &before,
        &root,
        &[PathBuf::from("CHANGELOG.md")],
        &[PathBuf::from("Cargo.toml")],
        &[PathBuf::from(".changeset/one.md")],
        &[],
        "version",
    );
}

#[test]
fn release_creates_only_the_expected_tag() {
    let root = git_repo_with_package("release");
    write_config(&root, &versioned(""));
    write_install_pin(&root);
    git(&root, &["add", "."]);
    git(&root, &["commit", "--no-verify", "-m", "chore: config"]);
    git(&root, &["tag", "v0.1.0"]);
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.1\"\nedition = \"2021\"\n\n[workspace]\n",
    )
    .expect("bump manifest");
    git(&root, &["add", "."]);
    git(&root, &["commit", "--no-verify", "-m", "chore: version"]);
    add_bare_origin(&root);

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/releases/tags/v0.1.1");
        then.status(404).body("Not Found");
    });
    server.mock(|when, then| {
        when.method(POST)
            .path("/repos/oakoss/oakum/releases")
            .body_includes("\"tag_name\":\"v0.1.1\"");
        then.status(201).json_body(json!({
            "html_url": "https://github.com/oakoss/oakum/releases/tag/v0.1.1"
        }));
    });

    let before = RepoState::capture(&root);
    let output = oakum(&root)
        .arg("release")
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .output()
        .expect("release");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    RepoState::assert_allowed_delta(
        &before,
        &root,
        &[],
        &[],
        &[],
        &["v0.1.1".to_owned()],
        "release",
    );
    assert_eq!(git_stdout(&root, &["cat-file", "-t", "v0.1.1"]), "tag");
}
