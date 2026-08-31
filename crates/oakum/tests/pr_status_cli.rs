//! `oakum ci pr-status` (`okm-961`).

#![allow(clippy::disallowed_methods)]

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use httpmock::prelude::*;
use serde_json::json;
use support::fixture::{git_output, oakum, plain_repo, Fixture};

/// A config whose `tool-version` always matches the binary under test. This
/// command is not behind the ADR-0007 gate; deriving the version keeps the
/// fixtures uniform with the suites that are.
fn versioned(rest: &str) -> String {
    format!("tool-version = \"{}\"\n{}", env!("CARGO_PKG_VERSION"), rest)
}

fn bin(root: &Path) -> Command {
    let mut cmd = oakum(root);
    cmd.env_remove("GITHUB_GRAPHQL_URL");
    cmd
}

fn temp_repo(label: &str) -> Fixture {
    plain_repo("pr-status", label)
}

fn cargo_package(root: &Path, name: &str) {
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

fn write_config(root: &Path, extra: &str) {
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(root.join(".changeset/_config.toml"), versioned(extra)).expect("config");
}

fn write_patch_changeset(root: &Path, name: &str) {
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/one.md"),
        format!("---\n{name}: patch\n---\n\npatch {name}\n"),
    )
    .expect("changeset");
}

fn git(root: &Path, args: &[&str]) -> std::process::Output {
    git_output(root, args)
}

fn commit(root: &Path, message: &str) {
    let add = git(root, &["add", "-A"]);
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );
    let committed = git(root, &["commit", "-m", message]);
    assert!(
        committed.status.success(),
        "{}",
        String::from_utf8_lossy(&committed.stderr)
    );
}

fn init_git(root: &Path) {
    let output = git(root, &["init"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn event_path(root: &Path, number: u64) -> PathBuf {
    let path = root.join("event.json");
    fs::write(
        &path,
        format!(r#"{{"pull_request":{{"number":{number}}}}}"#),
    )
    .expect("event");
    path
}

fn planned_repo(label: &str) -> Fixture {
    let root = temp_repo(label);
    cargo_package(&root, "demo");
    write_config(&root, "");
    write_patch_changeset(&root, "demo");
    init_git(&root);
    commit(&root, "init");
    fs::write(root.join("src/lib.rs"), "// changed\n").expect("edit");
    commit(&root, "feat: bump");
    root
}

#[test]
fn none_writes_nothing() {
    let root = planned_repo("none");
    write_config(&root, "pr-status = \"none\"\n");

    let server = MockServer::start();
    let listed = server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/issues/4/comments");
        then.status(200).json_body(json!([]));
    });

    let summary = root.join("summary.md");
    let output = bin(&root)
        .args(["ci", "pr-status", "--from", "HEAD~1"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_EVENT_PATH", event_path(&root, 4))
        .env("GITHUB_STEP_SUMMARY", &summary)
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci pr-status");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    listed.assert();
    assert!(!summary.exists());
}

#[test]
fn none_deletes_a_leftover_bot_comment() {
    let root = planned_repo("none-stale");
    write_config(&root, "pr-status = \"none\"\n");

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/issues/4/comments");
        then.status(200).json_body(json!([
            {
                "id": 7,
                "body": "<!-- oakum:pr-plan -->\nold plan",
                "user": { "login": "github-actions[bot]" }
            }
        ]));
    });
    let deleted = server.mock(|when, then| {
        when.method(DELETE)
            .path("/repos/oakoss/oakum/issues/comments/7");
        then.status(204).body("");
    });

    let summary = root.join("summary.md");
    let output = bin(&root)
        .args(["ci", "pr-status", "--from", "HEAD~1"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_EVENT_PATH", event_path(&root, 4))
        .env("GITHUB_STEP_SUMMARY", &summary)
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci pr-status");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    deleted.assert();
    assert!(!summary.exists());
}

#[test]
fn no_opinion_skips_comment_and_summary() {
    let root = temp_repo("silent");
    cargo_package(&root, "demo");
    write_config(&root, "");
    init_git(&root);
    commit(&root, "init");
    fs::write(root.join("README.md"), "docs\n").expect("readme");
    commit(&root, "docs: note");

    let server = MockServer::start();
    let listed = server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/issues/4/comments");
        then.status(200).json_body(json!([]));
    });
    let posted = server.mock(|when, then| {
        when.method(POST)
            .path("/repos/oakoss/oakum/issues/4/comments");
        then.status(201).json_body(json!({ "id": 1 }));
    });
    let summary = root.join("summary.md");
    let output = bin(&root)
        .args(["ci", "pr-status", "--from", "HEAD"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_EVENT_PATH", event_path(&root, 4))
        .env("GITHUB_STEP_SUMMARY", &summary)
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci pr-status");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    listed.assert();
    posted.assert_calls(0);
    assert!(!summary.exists());
}

#[test]
fn no_opinion_deletes_a_leftover_bot_comment() {
    let root = temp_repo("stale");
    cargo_package(&root, "demo");
    write_config(&root, "");
    init_git(&root);
    commit(&root, "init");
    fs::write(root.join("README.md"), "docs\n").expect("readme");
    commit(&root, "docs: note");

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/issues/4/comments");
        then.status(200).json_body(json!([
            {
                "id": 7,
                "body": "<!-- oakum:pr-plan -->\nold plan",
                "user": { "login": "github-actions[bot]" }
            }
        ]));
    });
    let deleted = server.mock(|when, then| {
        when.method(DELETE)
            .path("/repos/oakoss/oakum/issues/comments/7");
        then.status(204).body("");
    });

    let summary = root.join("summary.md");
    let output = bin(&root)
        .args(["ci", "pr-status", "--from", "HEAD"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_EVENT_PATH", event_path(&root, 4))
        .env("GITHUB_STEP_SUMMARY", &summary)
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci pr-status");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    deleted.assert();
    assert!(!summary.exists());
}

#[test]
fn posts_comment_and_writes_summary() {
    let root = planned_repo("both");

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/issues/4/comments");
        then.status(200).json_body(json!([]));
    });
    let created = server.mock(|when, then| {
        when.method(POST)
            .path("/repos/oakoss/oakum/issues/4/comments")
            .body_includes("<!-- oakum:pr-plan -->")
            .body_includes("These packages will release")
            .body_includes("`demo`");
        then.status(201).json_body(json!({ "id": 11 }));
    });

    let summary = root.join("summary.md");
    let output = bin(&root)
        .args(["ci", "pr-status", "--from", "HEAD~1"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_EVENT_PATH", event_path(&root, 4))
        .env("GITHUB_STEP_SUMMARY", &summary)
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci pr-status");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    created.assert();
    let summary_text = fs::read_to_string(&summary).expect("summary");
    assert!(summary_text.contains("## Release plan"), "{summary_text}");
    assert!(summary_text.contains("demo"), "{summary_text}");
}

#[test]
fn forbidden_comment_writes_summary_and_exits_zero() {
    let root = planned_repo("fork");
    write_config(&root, "pr-status = \"comment\"\n");

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/issues/4/comments");
        then.status(403).body("read-only token");
    });

    let summary = root.join("summary.md");
    let output = bin(&root)
        .args(["ci", "pr-status", "--from", "HEAD~1"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_EVENT_PATH", event_path(&root, 4))
        .env("GITHUB_STEP_SUMMARY", &summary)
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci pr-status");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no write permission (fork pull request)"),
        "{stderr}"
    );
    let summary_text = fs::read_to_string(&summary).expect("summary");
    assert!(summary_text.contains("## Release plan"), "{summary_text}");
}

#[test]
fn missing_token_degrades_to_summary() {
    let root = planned_repo("no-token");
    let summary = root.join("summary.md");
    let output = bin(&root)
        .args(["ci", "pr-status", "--from", "HEAD~1"])
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_EVENT_PATH", event_path(&root, 4))
        .env("GITHUB_STEP_SUMMARY", &summary)
        .output()
        .expect("oakum ci pr-status");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("GITHUB_TOKEN is unset"), "{stderr}");
    let summary_text = fs::read_to_string(&summary).expect("summary");
    assert!(summary_text.contains("demo"), "{summary_text}");
}

#[test]
fn summary_channel_does_not_call_github() {
    let root = planned_repo("summary-only");
    write_config(&root, "pr-status = \"summary\"\n");

    let server = MockServer::start();
    let hit = server.mock(|when, then| {
        when.any_request();
        then.status(500).body("summary must not call GitHub");
    });

    let summary = root.join("summary.md");
    let output = bin(&root)
        .args(["ci", "pr-status", "--from", "HEAD~1"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_EVENT_PATH", event_path(&root, 4))
        .env("GITHUB_STEP_SUMMARY", &summary)
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci pr-status");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    hit.assert_calls(0);
    let summary_text = fs::read_to_string(&summary).expect("summary");
    assert!(summary_text.contains("## Release plan"), "{summary_text}");
    assert!(summary_text.contains("demo"), "{summary_text}");
}

#[test]
fn comment_channel_does_not_write_a_summary_file() {
    let root = planned_repo("comment-only");
    write_config(&root, "pr-status = \"comment\"\n");

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/issues/4/comments");
        then.status(200).json_body(json!([]));
    });
    let created = server.mock(|when, then| {
        when.method(POST)
            .path("/repos/oakoss/oakum/issues/4/comments")
            .body_includes("<!-- oakum:pr-plan -->");
        then.status(201).json_body(json!({ "id": 11 }));
    });

    let summary = root.join("summary.md");
    let output = bin(&root)
        .args(["ci", "pr-status", "--from", "HEAD~1"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_EVENT_PATH", event_path(&root, 4))
        .env("GITHUB_STEP_SUMMARY", &summary)
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci pr-status");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    created.assert();
    assert!(!summary.exists());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("## Release plan"), "{stdout}");
}

#[test]
fn missing_pull_number_degrades_to_summary() {
    let root = planned_repo("no-pr");
    let server = MockServer::start();
    let hit = server.mock(|when, then| {
        when.any_request();
        then.status(500)
            .body("not a pull request must not call GitHub");
    });
    let summary = root.join("summary.md");
    let output = bin(&root)
        .args(["ci", "pr-status", "--from", "HEAD~1"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_EVENT_PATH")
        .env_remove("GITHUB_REF")
        .env("GITHUB_STEP_SUMMARY", &summary)
        .output()
        .expect("oakum ci pr-status");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    hit.assert_calls(0);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not a pull request"), "{stderr}");
    let summary_text = fs::read_to_string(&summary).expect("summary");
    assert!(summary_text.contains("demo"), "{summary_text}");
}

#[test]
fn an_issue_event_without_pull_request_degrades() {
    let root = planned_repo("issue-event");
    let event = root.join("event.json");
    fs::write(&event, r#"{"issue":{"number":4}}"#).expect("event");

    let server = MockServer::start();
    let hit = server.mock(|when, then| {
        when.any_request();
        then.status(500)
            .body("ordinary issue must not get a plan comment");
    });
    let summary = root.join("summary.md");
    let output = bin(&root)
        .args(["ci", "pr-status", "--from", "HEAD~1"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_EVENT_PATH", &event)
        .env_remove("GITHUB_REF")
        .env_remove("GH_TOKEN")
        .env("GITHUB_STEP_SUMMARY", &summary)
        .output()
        .expect("oakum ci pr-status");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    hit.assert_calls(0);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not a pull request"), "{stderr}");
    let summary_text = fs::read_to_string(&summary).expect("summary");
    assert!(summary_text.contains("demo"), "{summary_text}");
}

#[test]
fn an_issue_event_with_pull_request_posts() {
    let root = planned_repo("issue-pr");
    let event = root.join("event.json");
    fs::write(&event, r#"{"issue":{"number":4,"pull_request":{}}}"#).expect("event");

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/issues/4/comments");
        then.status(200).json_body(json!([]));
    });
    let created = server.mock(|when, then| {
        when.method(POST)
            .path("/repos/oakoss/oakum/issues/4/comments")
            .body_includes("<!-- oakum:pr-plan -->");
        then.status(201).json_body(json!({ "id": 11 }));
    });

    let output = bin(&root)
        .args(["ci", "pr-status", "--from", "HEAD~1"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_EVENT_PATH", &event)
        .env_remove("GITHUB_REF")
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci pr-status");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    created.assert();
}

#[test]
fn a_null_issue_pull_request_does_not_post() {
    let root = planned_repo("issue-null");
    let event = root.join("event.json");
    fs::write(&event, r#"{"issue":{"number":4,"pull_request":null}}"#).expect("event");

    let server = MockServer::start();
    let hit = server.mock(|when, then| {
        when.any_request();
        then.status(500)
            .body("null pull_request must not get a plan comment");
    });
    let summary = root.join("summary.md");
    let output = bin(&root)
        .args(["ci", "pr-status", "--from", "HEAD~1"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_EVENT_PATH", &event)
        .env_remove("GITHUB_REF")
        .env_remove("GH_TOKEN")
        .env("GITHUB_STEP_SUMMARY", &summary)
        .output()
        .expect("oakum ci pr-status");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    hit.assert_calls(0);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not a pull request"), "{stderr}");
}

#[test]
fn pull_number_from_github_ref_posts() {
    let root = planned_repo("ref-pr");

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/issues/4/comments");
        then.status(200).json_body(json!([]));
    });
    let created = server.mock(|when, then| {
        when.method(POST)
            .path("/repos/oakoss/oakum/issues/4/comments")
            .body_includes("<!-- oakum:pr-plan -->");
        then.status(201).json_body(json!({ "id": 11 }));
    });

    let output = bin(&root)
        .args(["ci", "pr-status", "--from", "HEAD~1"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_REF", "refs/pull/4/merge")
        .env_remove("GITHUB_EVENT_PATH")
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci pr-status");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    created.assert();
}

#[test]
fn gh_token_without_github_token_does_not_post() {
    let root = planned_repo("gh-token-only");
    let server = MockServer::start();
    let hit = server.mock(|when, then| {
        when.any_request();
        then.status(500).body("GH_TOKEN must not post a comment");
    });
    let summary = root.join("summary.md");
    let output = bin(&root)
        .args(["ci", "pr-status", "--from", "HEAD~1"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GH_TOKEN", "pat")
        .env_remove("GITHUB_TOKEN")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_EVENT_PATH", event_path(&root, 4))
        .env("GITHUB_STEP_SUMMARY", &summary)
        .output()
        .expect("oakum ci pr-status");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    hit.assert_calls(0);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("GITHUB_TOKEN is unset"), "{stderr}");
    let summary_text = fs::read_to_string(&summary).expect("summary");
    assert!(summary_text.contains("demo"), "{summary_text}");
}

#[test]
fn uncovered_only_names_the_package_in_comment_and_summary() {
    let root = temp_repo("uncovered");
    cargo_package(&root, "demo");
    write_config(&root, "");
    init_git(&root);
    commit(&root, "init");
    fs::write(root.join("src/lib.rs"), "// changed\n").expect("edit");
    commit(&root, "feat: touch");

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/issues/4/comments");
        then.status(200).json_body(json!([]));
    });
    let created = server.mock(|when, then| {
        when.method(POST)
            .path("/repos/oakoss/oakum/issues/4/comments")
            .body_includes("<!-- oakum:pr-plan -->")
            .body_includes("Uncovered")
            .body_includes("`demo` (cargo) changed with no bump file");
        then.status(201).json_body(json!({ "id": 12 }));
    });

    let summary = root.join("summary.md");
    let output = bin(&root)
        .args(["ci", "pr-status", "--from", "HEAD~1"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_EVENT_PATH", event_path(&root, 4))
        .env("GITHUB_STEP_SUMMARY", &summary)
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci pr-status");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    created.assert();
    let summary_text = fs::read_to_string(&summary).expect("summary");
    assert!(
        summary_text.contains("No packages planned."),
        "{summary_text}"
    );
    assert!(summary_text.contains("Uncovered"), "{summary_text}");
    assert!(summary_text.contains("demo"), "{summary_text}");
}

#[test]
fn unauthorized_comment_is_not_a_fork_403() {
    let root = planned_repo("unauth");
    write_config(&root, "pr-status = \"comment\"\n");

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/issues/4/comments");
        then.status(401)
            .body("GitHub /repos/oakoss/oakum/issues/4/comments returned 403");
    });

    let summary = root.join("summary.md");
    let output = bin(&root)
        .args(["ci", "pr-status", "--from", "HEAD~1"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_EVENT_PATH", event_path(&root, 4))
        .env("GITHUB_STEP_SUMMARY", &summary)
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci pr-status");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("did not accept the comment"), "{stderr}");
    assert!(!stderr.contains("fork pull request"), "{stderr}");
    let summary_text = fs::read_to_string(&summary).expect("summary");
    assert!(summary_text.contains("## Release plan"), "{summary_text}");
}

#[test]
fn both_forbidden_writes_the_summary_once() {
    let root = planned_repo("both-fork");

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/issues/4/comments");
        then.status(403).body("read-only token");
    });

    let summary = root.join("summary.md");
    let output = bin(&root)
        .args(["ci", "pr-status", "--from", "HEAD~1"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_EVENT_PATH", event_path(&root, 4))
        .env("GITHUB_STEP_SUMMARY", &summary)
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci pr-status");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let summary_text = fs::read_to_string(&summary).expect("summary");
    assert_eq!(
        summary_text.matches("## Release plan").count(),
        1,
        "{summary_text}"
    );
}

#[test]
fn check_with_a_token_does_not_call_github() {
    let root = planned_repo("check-token");
    fs::create_dir_all(root.join(".github/workflows")).expect("workflows");
    fs::write(
        root.join(".github/workflows/release.yml"),
        format!(
            "run: cargo binstall --no-confirm oakum@{}\n",
            env!("CARGO_PKG_VERSION")
        ),
    )
    .expect("pin");
    git(&root, &["add", "-A"]);
    commit(&root, "chore: pin");

    let server = MockServer::start();
    let hit = server.mock(|when, then| {
        when.any_request();
        then.status(500).body("check must not call GitHub");
    });

    let summary = root.join("summary.md");
    let output = bin(&root)
        .args(["check", "--from", "HEAD~1"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GH_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_EVENT_PATH", event_path(&root, 4))
        .env("GITHUB_REF", "refs/pull/4/merge")
        .env("GITHUB_STEP_SUMMARY", &summary)
        .output()
        .expect("oakum check");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    hit.assert_calls(0);
    assert!(!summary.exists());
}

#[test]
fn emit_comment_writes_the_sticky_body_and_skips_github() {
    let root = planned_repo("emit-comment");
    write_config(&root, "pr-status = \"comment\"\n");

    let server = MockServer::start();
    let hit = server.mock(|when, then| {
        when.any_request();
        then.status(500).body("emit-comment must not call GitHub");
    });

    let out = root.join("comment-out");
    let output = bin(&root)
        .args([
            "ci",
            "pr-status",
            "--from",
            "HEAD~1",
            "--emit-comment",
            out.to_str().expect("utf-8 path"),
        ])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_EVENT_PATH", event_path(&root, 4))
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci pr-status");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    hit.assert_calls(0);
    let body = fs::read_to_string(out.join("oakum-pr-comment.md")).expect("emitted comment");
    assert!(
        body.contains("<!-- oakum:pr-plan -->"),
        "missing sticky marker: {body}"
    );
    assert!(body.contains("demo"), "missing package plan: {body}");
    assert!(
        body.ends_with('\n'),
        "emitted comment must end with a newline"
    );
}

#[test]
fn emit_comment_refuses_pr_status_none() {
    let root = planned_repo("emit-none");
    write_config(&root, "pr-status = \"none\"\n");

    let server = MockServer::start();
    let hit = server.mock(|when, then| {
        when.any_request();
        then.status(500).body("emit+none must not call GitHub");
    });

    let out = root.join("comment-out");
    let output = bin(&root)
        .args([
            "ci",
            "pr-status",
            "--from",
            "HEAD~1",
            "--emit-comment",
            out.to_str().expect("utf-8 path"),
        ])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_EVENT_PATH", event_path(&root, 4))
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci pr-status");
    assert!(
        !output.status.success(),
        "expected failure, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("pr-status=none refuses --emit-comment"),
        "{stderr}"
    );
    hit.assert_calls(0);
    assert!(!out.join("oakum-pr-comment.md").exists());
}

#[test]
fn emit_comment_with_no_opinion_skips_github_cleanup() {
    let root = temp_repo("emit-silent");
    cargo_package(&root, "demo");
    write_config(&root, "pr-status = \"comment\"\n");
    init_git(&root);
    commit(&root, "init");
    fs::write(root.join("README.md"), "docs\n").expect("readme");
    commit(&root, "docs: note");

    let server = MockServer::start();
    let hit = server.mock(|when, then| {
        when.any_request();
        then.status(500)
            .body("emit+no-opinion must not call GitHub");
    });

    let out = root.join("comment-out");
    let output = bin(&root)
        .args([
            "ci",
            "pr-status",
            "--from",
            "HEAD",
            "--emit-comment",
            out.to_str().expect("utf-8 path"),
        ])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_EVENT_PATH", event_path(&root, 4))
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci pr-status");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    hit.assert_calls(0);
    assert!(!out.join("oakum-pr-comment.md").exists());
}

#[test]
fn emit_comment_with_no_opinion_removes_a_stale_artifact() {
    let root = temp_repo("emit-stale");
    cargo_package(&root, "demo");
    write_config(&root, "pr-status = \"comment\"\n");
    init_git(&root);
    commit(&root, "init");
    fs::write(root.join("README.md"), "docs\n").expect("readme");
    commit(&root, "docs: note");

    let out = root.join("comment-out");
    fs::create_dir_all(&out).expect("emit dir");
    fs::write(
        out.join("oakum-pr-comment.md"),
        "<!-- oakum:pr-plan -->\nstale\n",
    )
    .expect("stale artifact");

    let server = MockServer::start();
    let hit = server.mock(|when, then| {
        when.any_request();
        then.status(500)
            .body("emit+stale-clear must not call GitHub");
    });

    let output = bin(&root)
        .args([
            "ci",
            "pr-status",
            "--from",
            "HEAD",
            "--emit-comment",
            out.to_str().expect("utf-8 path"),
        ])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_EVENT_PATH", event_path(&root, 4))
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci pr-status");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    hit.assert_calls(0);
    assert!(
        !out.join("oakum-pr-comment.md").exists(),
        "stale emit artifact must be removed"
    );
}
