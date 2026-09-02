//! `oakum status --json` and the built-in summary render (okm-1q3).

#![allow(clippy::disallowed_methods)]

mod support;

use std::fs;
use std::process::Command;

use support::fixture::{cargo_package, oakum, plain_repo, Fixture};

use serde_json::Value;

/// A config whose `tool-version` always matches the binary under test. This
/// command is not behind the ADR-0007 gate; deriving the version keeps the
/// fixtures uniform with the suites that are.
fn versioned(rest: &str) -> String {
    format!("tool-version = \"{}\"\n{}", env!("CARGO_PKG_VERSION"), rest)
}

fn temp_repo(label: &str) -> Fixture {
    let root = plain_repo("status", label);
    fs::create_dir(root.join(".git")).expect("fixture .git");
    root
}

fn write_patch_changeset(root: &std::path::Path) {
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/one.md"),
        "---\ndemo: patch\n---\n\npatch demo\n",
    )
    .expect("changeset");
}

#[test]
fn json_emits_schema_version_one_and_planned_package() {
    let root = temp_repo("json");
    cargo_package(&root, "demo", "0.1.0");
    write_patch_changeset(&root);

    let output = oakum(&root)
        .args(["status", "--json"])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["target"], "status");
    assert_eq!(value["packages"][0]["name"], "demo");
    assert_eq!(value["packages"][0]["ecosystem"], "cargo");
    assert_eq!(value["packages"][0]["from"], "0.1.0");
    assert_eq!(value["packages"][0]["to"], "0.1.1");
    assert_eq!(value["packages"][0]["bump"], "patch");
    assert_eq!(value["packages"][0]["source"]["kind"], "intent");
    assert!(value["uncovered"].as_array().expect("uncovered").is_empty());
}

#[test]
fn summary_template_lists_the_planned_bump() {
    let root = temp_repo("summary");
    cargo_package(&root, "demo", "0.1.0");
    write_patch_changeset(&root);

    let output = oakum(&root)
        .args(["status", "--template", "summary"])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("## Release plan"), "{stdout}");
    assert!(stdout.contains("demo"), "{stdout}");
    assert!(stdout.contains("0.1.0"), "{stdout}");
    assert!(stdout.contains("0.1.1"), "{stdout}");
    assert!(stdout.contains("patch"), "{stdout}");
    assert!(stdout.contains("intent"), "{stdout}");
    assert!(
        !stdout.contains("No uncovered packages."),
        "empty uncovered is not a completed coverage check, got: {stdout}"
    );
}

#[test]
fn default_render_matches_summary_template() {
    let root = temp_repo("default");
    cargo_package(&root, "demo", "0.1.0");
    write_patch_changeset(&root);

    let default = oakum(&root).arg("status").output().expect("run");
    let named = oakum(&root)
        .args(["status", "--template", "summary"])
        .output()
        .expect("run");
    assert!(default.status.success());
    assert!(named.status.success());
    assert_eq!(default.stdout, named.stdout);
}

#[test]
fn unknown_template_fails() {
    let root = temp_repo("unknown");
    cargo_package(&root, "demo", "0.1.0");

    let output = oakum(&root)
        .args(["status", "--template", "slack"])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("unknown template"), "{err}");
    assert!(err.contains("summary"), "{err}");
}

#[test]
fn json_and_template_conflict() {
    let output = Command::new(env!("CARGO_BIN_EXE_oakum"))
        .args(["status", "--json", "--template", "summary"])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("cannot be used with") || err.contains("conflict"),
        "stderr should report a flag conflict, got: {err}"
    );
}

#[test]
fn empty_plan_is_still_schema_version_one() {
    let root = temp_repo("empty");
    cargo_package(&root, "demo", "0.1.0");

    let output = oakum(&root)
        .args(["status", "--json"])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).expect("json");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["target"], "status");
    assert!(value["packages"].as_array().expect("packages").is_empty());
    assert!(value["uncovered"].as_array().expect("uncovered").is_empty());
}

#[test]
fn empty_plan_summary_has_no_table() {
    let root = temp_repo("empty-summary");
    cargo_package(&root, "demo", "0.1.0");

    let output = oakum(&root)
        .args(["status", "--template", "summary"])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("## Release plan"), "{stdout}");
    assert!(stdout.contains("No packages planned."), "{stdout}");
    assert!(
        !stdout.contains("| Package |"),
        "empty plan must not print the table header, got: {stdout}"
    );
}

#[test]
fn semver_policy_takes_pre_1_major_to_1_0_0() {
    let root = temp_repo("semver");
    cargo_package(&root, "demo", "0.1.0");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        versioned("versioning = \"semver\"\n"),
    )
    .expect("config");
    fs::write(
        root.join(".changeset/one.md"),
        "---\ndemo: major\n---\n\nbreaking\n",
    )
    .expect("changeset");

    let output = oakum(&root)
        .args(["status", "--json"])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).expect("json");
    assert_eq!(value["packages"][0]["from"], "0.1.0");
    assert_eq!(value["packages"][0]["to"], "1.0.0");
    assert_eq!(value["packages"][0]["bump"], "major");
}

#[test]
fn mismatched_tool_version_still_emits_status() {
    let root = temp_repo("toolver");
    cargo_package(&root, "demo", "0.1.0");
    write_patch_changeset(&root);
    fs::write(
        root.join(".changeset/_config.toml"),
        "tool-version = \"9.9.9\"\n",
    )
    .expect("config");

    let output = oakum(&root)
        .args(["status", "--json"])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(!err.contains("upgrade"), "{err}");
    let value: Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).expect("json");
    assert_eq!(value["packages"][0]["name"], "demo");
    assert_eq!(value["packages"][0]["to"], "0.1.1");
}
