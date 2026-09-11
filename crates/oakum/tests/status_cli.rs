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

#[test]
fn no_config_says_defaults_are_in_effect_and_still_reports() {
    let root = temp_repo("no-config");
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
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(
            "`.changeset/_config.toml` not found; defaults in effect (run `oakum init` or `oakum migrate`)"
        ),
        "{stderr}"
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["packages"][0]["name"], "demo");
}

#[test]
fn a_malformed_bump_file_fails_status_by_name() {
    let root = temp_repo("malformed");
    cargo_package(&root, "demo", "0.1.0");
    write_patch_changeset(&root);
    fs::write(root.join(".changeset/bad.md"), "not a bump file\n").expect("bad");

    let output = oakum(&root)
        .args(["status", "--json"])
        .output()
        .expect("run");
    assert!(
        !output.status.success(),
        "a malformed bump file must not report an empty plan; stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("`bad.md` is not a bump file"), "{stderr}");
    assert!(output.stdout.is_empty(), "no JSON beside a refusal");
}

#[test]
fn a_present_config_gets_no_defaults_note() {
    let root = temp_repo("config-present");
    cargo_package(&root, "demo", "0.1.0");
    write_patch_changeset(&root);
    fs::write(root.join(".changeset/_config.toml"), versioned("")).expect("config");

    let output = oakum(&root)
        .args(["status", "--json"])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("defaults in effect"),
        "a present config is not reported as absent: {stderr}"
    );
}

/// A real repository, unlike [`temp_repo`]'s fake `.git`: the coverage look and
/// `--from` both need git to answer.
fn temp_git_repo(label: &str) -> Fixture {
    support::fixture::git_repo("status", label)
}

/// `check` refuses on this config; `status` prints `No packages planned.` and
/// exits 0, the same sentence it prints while waiting for a bump file
/// (`okm-404.23`). `status` reports rather than gates, so the exit code stays 0
/// and the summary carries the difference.
#[test]
fn a_config_that_manages_nothing_says_so_rather_than_planning_nothing() {
    let root = temp_git_repo("manages-nothing-summary");
    fs::write(
        root.join("package.json"),
        "{\n  \"name\": \"demo\",\n  \"version\": \"0.1.0\",\n  \"private\": true\n}\n",
    )
    .expect("package.json");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(root.join(".changeset/_config.toml"), versioned("")).expect("config");
    support::fixture::commit(&root, "init");

    let output = oakum(&root).args(["status"]).output().expect("run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "status reports, it does not gate: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("No packages planned."), "{stdout}");
    assert!(
        stdout.contains("manages no package on either axis"),
        "and says the plan can never be non-empty: {stdout}"
    );
}

/// The JSON half of the same fact: a dashboard reading `packages: []` cannot
/// otherwise tell a quiet repository from one that can never release.
#[test]
fn a_config_that_manages_nothing_carries_the_fact_in_json() {
    let root = temp_git_repo("manages-nothing-json");
    fs::write(
        root.join("package.json"),
        "{\n  \"name\": \"demo\",\n  \"version\": \"0.1.0\",\n  \"private\": true\n}\n",
    )
    .expect("package.json");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(root.join(".changeset/_config.toml"), versioned("")).expect("config");
    support::fixture::commit(&root, "init");

    let output = oakum(&root)
        .args(["status", "--json"])
        .output()
        .expect("run");
    assert!(output.status.success());
    let json: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(json["manages_nothing"], Value::Bool(true));
    assert_eq!(json["packages"], Value::Array(vec![]));
    assert_eq!(json["schema_version"], 1, "additive, not a new shape");
}

/// An empty `uncovered` means the look ran and found nothing, which holds only
/// while `status` runs the coverage look itself rather than deferring it to
/// `check`.
#[test]
fn a_changed_package_with_no_bump_file_is_reported_as_uncovered() {
    let root = temp_git_repo("uncovered-real");
    cargo_package(&root, "demo", "0.1.0");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(root.join(".changeset/_config.toml"), versioned("")).expect("config");
    support::fixture::commit(&root, "init");
    fs::write(root.join("src/lib.rs"), "// changed\n").expect("change");
    support::fixture::commit(&root, "touch demo");

    let output = oakum(&root)
        .args(["status", "--json", "--from", "HEAD~1"])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(json["coverage_checked"], Value::Bool(true));
    assert_eq!(
        json["uncovered"][0]["name"],
        Value::String(String::from("demo"))
    );
}

/// A tree git cannot diff is not a tree with nothing uncovered. `status`
/// reports the plan either way — it is not a gate — but it does not claim a
/// look it could not make.
#[test]
fn a_tree_git_cannot_diff_says_the_coverage_look_did_not_run() {
    let root = temp_repo("coverage-unchecked");
    cargo_package(&root, "demo", "0.1.0");
    write_patch_changeset(&root);
    fs::write(root.join(".changeset/_config.toml"), versioned("")).expect("config");

    let output = oakum(&root)
        .args(["status", "--json"])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "a failed look does not fail the report: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("coverage not checked"), "{stderr}");
    let json: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(json["coverage_checked"], Value::Bool(false));
    assert_eq!(json["uncovered"], Value::Array(vec![]));
}

/// `alpha` publishable, `beta` not, and `beta`'s source changed in HEAD. The
/// shape okm-404.26 records, built once because two tests turn on what the
/// config says about `beta`.
fn mixed_workspace_with_beta_changed(label: &str, config_extra: &str) -> Fixture {
    let root = temp_git_repo(label);
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
        .expect("member");
        fs::write(path.join("src/lib.rs"), "").expect("lib.rs");
    }
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        versioned(config_extra),
    )
    .expect("config");
    support::fixture::commit(&root, "init");
    fs::write(root.join("beta/src/lib.rs"), "// changed\n").expect("change");
    support::fixture::commit(&root, "touch beta");
    root
}

/// The report `check` deliberately does not make. A private member changed and
/// no bump file could ever cover it, which is worth seeing in the command whose
/// job is showing you things — and worth not failing a gate over (`okm-404.26`).
#[test]
fn a_changed_package_the_config_cannot_version_is_reported() {
    let root = mixed_workspace_with_beta_changed("unmanaged-reported", "");

    let output = oakum(&root)
        .args(["status", "--json", "--from", "HEAD~1"])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "status reports, it does not gate: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(
        json["unmanaged"][0]["name"],
        Value::String(String::from("beta"))
    );
    assert_eq!(
        json["uncovered"],
        Value::Array(vec![]),
        "beta is not uncovered — it can never be covered"
    );

    let rendered = oakum(&root)
        .args(["status", "--from", "HEAD~1"])
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&rendered.stdout);
    assert!(
        stdout.contains("Changed but not version-managed: beta (`cargo`)"),
        "and the render says so too, since stderr does not travel: {stdout}"
    );
}

/// A shallow clone resolves the default base to HEAD itself, so the diff comes
/// back empty and every changed package looks covered. `actions/checkout`
/// clones that way by default, which makes it the common CI shape.
#[test]
fn a_shallow_clone_does_not_report_a_clean_coverage_look() {
    let src = temp_git_repo("shallow-src");
    cargo_package(&src, "demo", "0.1.0");
    fs::create_dir_all(src.join(".changeset")).expect("changeset");
    fs::write(src.join(".changeset/_config.toml"), versioned("")).expect("config");
    support::fixture::commit(&src, "one");
    fs::write(src.join("src/lib.rs"), "// changed\n").expect("change");
    support::fixture::commit(&src, "two");

    let dest = support::fixture::sibling(&src, "shallow");
    support::fixture::git(
        &src,
        &[
            "-c",
            "protocol.file.allow=always",
            "clone",
            "--depth=1",
            "--no-local",
            src.to_str().expect("utf-8"),
            dest.to_str().expect("utf-8"),
        ],
    );

    let output = oakum(&dest)
        .args(["status", "--json"])
        .output()
        .expect("run");
    assert!(output.status.success());
    let json: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(
        json["coverage_checked"],
        Value::Bool(false),
        "a truncated history is not a clean look: {json}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("shallow clone"), "{stderr}");
}

/// An exclusion is a decision someone wrote down, so the report must not accuse
/// them of an omission. Nothing else pins this on the reporting path:
/// substituting `Unmanaged` for `Excluded` left the whole suite green.
#[test]
fn a_changed_package_removed_by_exclude_is_not_reported_as_unmanaged() {
    let root = mixed_workspace_with_beta_changed("unmanaged-excluded", "exclude = [\"beta\"]\n");

    let output = oakum(&root)
        .args(["status", "--json", "--from", "HEAD~1"])
        .output()
        .expect("run");
    assert!(output.status.success());
    let json: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(
        json["unmanaged"],
        Value::Array(vec![]),
        "an exclusion is not an omission: {json}"
    );
    assert_eq!(
        json["coverage_checked"],
        Value::Bool(true),
        "and the look still ran: {json}"
    );

    let rendered = oakum(&root)
        .args(["status", "--from", "HEAD~1"])
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&rendered.stdout);
    assert!(
        !stdout.contains("Changed but not version-managed"),
        "{stdout}"
    );
}
