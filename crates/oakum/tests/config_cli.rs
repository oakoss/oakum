//! `_config.toml` at the CLI: schema refusals and missing-file defaults.

#![allow(clippy::disallowed_methods)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oakum"))
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

fn temp_repo(label: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("oakum-config-{label}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp repo");
    fs::create_dir(dir.join(".git")).expect("fixture .git");
    dir
}

fn write_config(root: &std::path::Path, body: &str) {
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(root.join(".changeset/_config.toml"), body).expect("config");
}

fn add_demo(root: &std::path::Path) -> std::process::Output {
    bin()
        .current_dir(root)
        .args([
            "add",
            "--packages",
            "demo:patch",
            "--message",
            "x",
            "--name",
            "cfg",
        ])
        .output()
        .expect("oakum add")
}

#[test]
fn unknown_config_key_refuses() {
    let root = temp_repo("unknown");
    cargo_package(&root, "demo");
    write_config(&root, "tool-version = \"0.0.0\"\ngit-user = \"bot\"\n");

    let output = add_demo(&root);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("git-user") && err.contains("not a valid oakum config"),
        "stderr: {err}"
    );
}

#[test]
fn snake_case_key_refuses() {
    let root = temp_repo("snake");
    cargo_package(&root, "demo");
    write_config(&root, "tool-version = \"0.0.0\"\nchange_files = false\n");

    let output = add_demo(&root);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("change_files"), "stderr: {err}");
}

#[test]
fn known_preference_keys_load() {
    let root = temp_repo("known");
    cargo_package(&root, "demo");
    write_config(
        &root,
        r#"
tool-version = "0.0.0"
versioning = "zero-major"
pr-status = "both"
tag-format = "v{{version}}"
commit-message = "chore: release {{version}}"
title = "Release"
template = "keep"

[packages.demo]
versioning = "semver"
resolves-dependencies-at = "build"
"#,
    );

    let output = add_demo(&root);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(root.join(".changeset/cfg.md").is_file());
}

#[test]
fn missing_tool_version_refuses() {
    let root = temp_repo("no-tool-version");
    cargo_package(&root, "demo");
    write_config(&root, "change-files = true\n");

    let output = add_demo(&root);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("tool-version") && err.contains("not a valid oakum config"),
        "stderr: {err}"
    );
}

#[test]
fn invalid_enum_value_refuses() {
    let root = temp_repo("bad-enum");
    cargo_package(&root, "demo");
    write_config(&root, "tool-version = \"0.0.0\"\npr-status = \"checks\"\n");

    let output = add_demo(&root);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("not a valid oakum config")
            && (err.contains("pr-status") || err.contains("checks")),
        "stderr: {err}"
    );
}

#[test]
fn unknown_package_key_refuses() {
    let root = temp_repo("pkg-unknown");
    cargo_package(&root, "demo");
    write_config(
        &root,
        "tool-version = \"0.0.0\"\n\n[packages.demo]\npublish = true\n",
    );

    let output = add_demo(&root);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("publish") && err.contains("not a valid oakum config"),
        "stderr: {err}"
    );
}

#[test]
fn missing_config_file_still_adds() {
    let root = temp_repo("no-config");
    cargo_package(&root, "demo");
    let output = add_demo(&root);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
