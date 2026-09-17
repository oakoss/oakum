//! `oakum ci pr-status` (`okm-961`).

#![allow(clippy::disallowed_methods)]

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use httpmock::prelude::*;
use serde_json::json;
use support::fixture::{
    cargo_package, commit, git, oakum, plain_repo, sibling, versioned, Fixture,
};

fn bin(root: &Path) -> Command {
    let mut cmd = oakum(root);
    cmd.env_remove("GITHUB_GRAPHQL_URL");
    // PR CI sets these for the version-packages branch; tests supply their own.
    cmd.env_remove("GITHUB_HEAD_REF");
    cmd.env_remove("GITHUB_EVENT_PATH");
    cmd
}

fn temp_repo(label: &str) -> Fixture {
    plain_repo("pr-status", label)
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

fn init_git(root: &Path) {
    git(root, &["init"]);
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

/// The payload a `pull_request` event carries for a version PR: the branch,
/// the head repository, and both actors. `event_path` is enough for a
/// contributor PR; a version PR is identified by all four, never the name.
fn version_pr_event_path(
    root: &Path,
    number: u64,
    head_repo: &str,
    user: &str,
    sender: &str,
) -> PathBuf {
    let path = root.join("event.json");
    fs::write(
        &path,
        format!(
            r#"{{"pull_request":{{"number":{number},"head":{{"ref":"oakum/version-packages","repo":{{"full_name":"{head_repo}"}}}},"user":{{"type":"{user}"}}}},"sender":{{"type":"{sender}"}}}}"#
        ),
    )
    .expect("event");
    path
}

/// A run that is not the version PR despite carrying its branch name posts
/// the coverage comment like any contributor PR. `head_repo` and `sender`
/// are the two terms a spoofer controls.
fn a_lookalike_still_gets_a_coverage_comment(
    label: &str,
    head_repo: &str,
    user: &str,
    sender: &str,
) {
    let root = planned_repo(label);
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
    let output = bin(&root)
        .args(["ci", "pr-status", "--from", "HEAD~1"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_HEAD_REF", "oakum/version-packages")
        .env(
            "GITHUB_EVENT_PATH",
            version_pr_event_path(&root, 4, head_repo, user, sender),
        )
        .env("GITHUB_STEP_SUMMARY", root.join("summary.md"))
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci pr-status");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    listed.assert();
    posted.assert();
}

/// With `GITHUB_HEAD_REF` absent — a caller outside Actions, or an event other
/// than `pull_request`/`pull_request_target` — the payload's `head.ref` is the
/// only branch gate.
#[test]
fn a_contributor_branch_in_the_payload_alone_still_gets_a_coverage_comment() {
    let root = planned_repo("version-pr-payload-branch");
    let event = root.join("event.json");
    fs::write(
        &event,
        r#"{"pull_request":{"number":4,"head":{"ref":"feature","repo":{"full_name":"oakoss/oakum"}},"user":{"type":"Bot"}},"sender":{"type":"Bot"}}"#,
    )
    .expect("event");
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
    let output = bin(&root)
        .args(["ci", "pr-status", "--from", "HEAD~1"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env_remove("GITHUB_HEAD_REF")
        .env("GITHUB_EVENT_PATH", &event)
        .env("GITHUB_STEP_SUMMARY", root.join("summary.md"))
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci pr-status");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    listed.assert();
    posted.assert();
}

/// A version-shaped payload behind a `GITHUB_HEAD_REF` that names another
/// branch is a contributor pull request: the branch is a precondition, and
/// the payload is not consulted.
#[test]
fn a_contributor_head_ref_decides_before_the_payload_is_read() {
    let root = planned_repo("version-pr-head-ref");
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
    let output = bin(&root)
        .args(["ci", "pr-status", "--from", "HEAD~1"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_HEAD_REF", "feature")
        .env(
            "GITHUB_EVENT_PATH",
            version_pr_event_path(&root, 4, "oakoss/oakum", "Bot", "Bot"),
        )
        .env("GITHUB_STEP_SUMMARY", root.join("summary.md"))
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci pr-status");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    listed.assert();
    posted.assert();
}

/// A payload that cannot be read cannot identify the version pull request,
/// so the run is treated as a contributor's and says why: the one cost is a
/// redundant comment on the real version pull request.
#[test]
fn an_unreadable_payload_is_a_contributor_pull_request_and_says_so() {
    let root = planned_repo("version-pr-unreadable-payload");
    let event = root.join("event.json");
    fs::write(&event, "{not json").expect("event");
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
    let output = bin(&root)
        .args(["ci", "pr-status", "--from", "HEAD~1"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_HEAD_REF", "oakum/version-packages")
        .env("GITHUB_REF", "refs/pull/4/merge")
        .env("GITHUB_EVENT_PATH", &event)
        .env("GITHUB_STEP_SUMMARY", root.join("summary.md"))
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci pr-status");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "stderr: {stderr}");
    assert!(
        stderr.contains("could not be read")
            && stderr.contains("treating this run as a contributor pull request"),
        "{stderr}"
    );
    listed.assert();
    posted.assert();
}

/// Without `GITHUB_REPOSITORY` the head repository cannot be compared, so a
/// version-shaped payload is treated as a contributor's and the run says why.
/// A contributor branch in the payload decides on its own and says nothing.
#[test]
fn a_missing_repository_variable_is_named_only_when_the_branch_matched() {
    for (label, head_ref, expect_line) in [
        ("version-pr-no-repo-var", "oakum/version-packages", true),
        ("contributor-no-repo-var", "feature", false),
    ] {
        let root = planned_repo(label);
        git(
            &root,
            &["remote", "add", "origin", "git@github.com:oakoss/oakum.git"],
        );
        let event = root.join("event.json");
        fs::write(
            &event,
            format!(
                r#"{{"pull_request":{{"number":4,"head":{{"ref":"{head_ref}","repo":{{"full_name":"oakoss/oakum"}}}},"user":{{"type":"Bot"}}}},"sender":{{"type":"Bot"}}}}"#
            ),
        )
        .expect("event");
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
        let output = bin(&root)
            .args(["ci", "pr-status", "--from", "HEAD~1"])
            .env("GITHUB_API_URL", server.base_url())
            .env("GITHUB_TOKEN", "token")
            .env_remove("GITHUB_REPOSITORY")
            .env_remove("GITHUB_HEAD_REF")
            .env("GITHUB_EVENT_PATH", &event)
            .env("GITHUB_STEP_SUMMARY", root.join("summary.md"))
            .env_remove("GH_TOKEN")
            .output()
            .expect("oakum ci pr-status");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{label}: stderr: {stderr}");
        assert_eq!(
            stderr.contains("GITHUB_REPOSITORY is not set"),
            expect_line,
            "{label}: {stderr}"
        );
        listed.assert();
        posted.assert();
    }
}

/// Without a payload the version pull request cannot be recognised either;
/// on the version branch the run says so, and on any other branch the branch
/// alone decides and nothing is said.
#[test]
fn a_missing_payload_is_named_only_when_the_branch_matched() {
    for (label, head_ref, expect_line) in [
        (
            "version-pr-no-payload",
            Some("oakum/version-packages"),
            true,
        ),
        ("contributor-no-payload", Some("feature"), false),
        ("outside-actions-no-payload", None, false),
    ] {
        let root = planned_repo(label);
        let summary = root.join("summary.md");
        let mut command = bin(&root);
        match head_ref {
            Some(head_ref) => command.env("GITHUB_HEAD_REF", head_ref),
            None => command.env_remove("GITHUB_HEAD_REF"),
        };
        let output = command
            .args(["ci", "pr-status", "--from", "HEAD~1"])
            .env_remove("GITHUB_TOKEN")
            .env_remove("GH_TOKEN")
            .env("GITHUB_REPOSITORY", "oakoss/oakum")
            .env_remove("GITHUB_EVENT_PATH")
            .env("GITHUB_STEP_SUMMARY", &summary)
            .output()
            .expect("oakum ci pr-status");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{label}: stderr: {stderr}");
        assert_eq!(
            stderr.contains("GITHUB_EVENT_PATH is not set"),
            expect_line,
            "{label}: {stderr}"
        );
    }
}

#[test]
fn a_fork_named_like_the_version_branch_still_gets_a_coverage_comment() {
    a_lookalike_still_gets_a_coverage_comment("version-pr-fork", "someone/oakum", "Bot", "Bot");
}

#[test]
fn a_person_pushing_the_version_branch_still_gets_a_coverage_comment() {
    a_lookalike_still_gets_a_coverage_comment(
        "version-pr-human-push",
        "oakoss/oakum",
        "Bot",
        "User",
    );
}

#[test]
fn a_person_opening_a_pr_on_the_version_branch_still_gets_a_coverage_comment() {
    a_lookalike_still_gets_a_coverage_comment(
        "version-pr-human-author",
        "oakoss/oakum",
        "User",
        "Bot",
    );
}

fn planned_repo(label: &str) -> Fixture {
    let root = temp_repo(label);
    cargo_package(&root, "demo", "0.1.0");
    write_config(&root, "");
    write_patch_changeset(&root, "demo");
    init_git(&root);
    commit(&root, "init");
    fs::write(root.join("src/lib.rs"), "// changed\n").expect("edit");
    commit(&root, "feat: bump");
    root
}

/// A change no package owns, so the plan is empty *and* nothing is uncovered —
/// the state that renders no comment at all. The member lives in a
/// subdirectory for that reason: a root package owns every path, so a
/// repository-root edit would come back uncovered instead of unowned.
fn empty_plan_repo(label: &str) -> Fixture {
    let root = temp_repo(label);
    write_config(&root, "");
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"demo\"]\n",
    )
    .expect("workspace");
    fs::create_dir_all(root.join("demo/src")).expect("member dir");
    fs::write(
        root.join("demo/Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("member manifest");
    fs::write(root.join("demo/src/lib.rs"), "").expect("member lib");
    init_git(&root);
    commit(&root, "init");
    fs::write(root.join("README.md"), "docs\n").expect("readme");
    commit(&root, "docs: note");
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
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("pr-status is set to `none`"),
        "a run that writes nothing says why: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Removing a leftover comment is best-effort — a fork's read-only token, an
/// unset one, an unreadable pull number are all expected states rather than
/// failures. They were silent, which was tolerable while the run said nothing
/// at all; beside a line announcing what was not written, silence reads as
/// "and nothing was left behind", which is the one thing it does not mean.
#[test]
fn a_leftover_comment_that_could_not_be_removed_is_not_passed_over() {
    let root = planned_repo("none-no-token");
    write_config(&root, "pr-status = \"none\"\n");

    let summary = root.join("summary.md");
    let output = bin(&root)
        .args(["ci", "pr-status", "--from", "HEAD~1"])
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_EVENT_PATH", event_path(&root, 4))
        .env("GITHUB_STEP_SUMMARY", &summary)
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci pr-status");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(stderr.contains("pr-status is set to `none`"), "{stderr}");
    assert!(
        stderr.contains("an earlier plan may still be visible on the pull request"),
        "the run says what it could not clean up: {stderr}"
    );
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
    cargo_package(&root, "demo", "0.1.0");
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
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("nothing to post"),
        "an empty plan is announced, not silent: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The finding, put as the comparison that exposes it. On 0.3.1 an empty plan
/// wrote no comment, no job summary and no line on either stream, so from the
/// workflow's side it was one of three things: nothing worth posting, a post
/// to somewhere nobody looked, or a failure whose reason was swallowed. Both
/// runs exit 0 and neither writes a summary file, so the streams are the only
/// place the difference can live — which is why asserting on one run alone
/// would not have caught it.
#[test]
fn an_empty_plan_is_told_apart_from_a_posted_one() {
    let mut said = Vec::new();
    for (label, root, posts) in [
        ("empty", empty_plan_repo("told-apart-empty"), false),
        ("planned", planned_repo("told-apart-planned"), true),
    ] {
        let server = MockServer::start();
        server.mock(|when, then| {
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
            .args(["ci", "pr-status", "--from", "HEAD~1"])
            .env("GITHUB_API_URL", server.base_url())
            .env("GITHUB_TOKEN", "token")
            .env("GITHUB_REPOSITORY", "oakoss/oakum")
            .env("GITHUB_EVENT_PATH", event_path(&root, 4))
            .env("GITHUB_STEP_SUMMARY", &summary)
            .env_remove("GH_TOKEN")
            .output()
            .expect("oakum ci pr-status");
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(output.status.success(), "{label}: {stderr}");
        posted.assert_calls(usize::from(posts));
        said.push((label, stderr));
    }
    let empty = &said[0].1;
    let planned = &said[1].1;
    assert!(
        empty.contains("nothing to post"),
        "the empty plan says why it posted nothing: {empty}"
    );
    assert_ne!(
        empty, planned,
        "a run that posted and a run that had nothing to post must not read alike"
    );
}

#[test]
fn version_packages_branch_skips_coverage_comment() {
    let root = planned_repo("version-pr-skip");

    let server = MockServer::start();
    let listed = server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/issues/4/comments");
        then.status(200).json_body(json!([
            {
                "id": 9,
                "body": "<!-- oakum:pr-plan -->\n\nUncovered:\n\n- `demo` (cargo) changed with no bump file\n",
                "user": { "login": "github-actions[bot]" }
            }
        ]));
    });
    let deleted = server.mock(|when, then| {
        when.method(DELETE)
            .path("/repos/oakoss/oakum/issues/comments/9");
        then.status(204).body("");
    });
    let posted = server.mock(|when, then| {
        when.method(POST)
            .path("/repos/oakoss/oakum/issues/4/comments");
        then.status(201).json_body(json!({ "id": 1 }));
    });

    let summary = root.join("summary.md");
    let output = bin(&root)
        .args(["ci", "pr-status", "--from", "HEAD~1"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_HEAD_REF", "oakum/version-packages")
        .env(
            "GITHUB_EVENT_PATH",
            version_pr_event_path(&root, 4, "oakoss/oakum", "Bot", "Bot"),
        )
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
    deleted.assert();
    posted.assert_calls(0);
    assert!(!summary.exists());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("version pull request"),
        "the branch that skips on purpose says so: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The version-PR path returns before the summary is rendered, so under
/// `pr-status = "summary"` it writes nothing at all. Naming only the comment
/// told an operator about a channel they never configured while the one they
/// did went unwritten and unmentioned — the same defect okm-6ozh closes, left
/// open on the path the fix touched.
#[test]
fn the_version_pull_request_names_the_channel_that_was_configured() {
    let root = planned_repo("version-pr-summary");
    write_config(&root, "pr-status = \"summary\"\n");

    let server = MockServer::start();
    let summary = root.join("summary.md");
    let output = bin(&root)
        .args(["ci", "pr-status", "--from", "HEAD~1"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_HEAD_REF", "oakum/version-packages")
        .env(
            "GITHUB_EVENT_PATH",
            version_pr_event_path(&root, 4, "oakoss/oakum", "Bot", "Bot"),
        )
        .env("GITHUB_STEP_SUMMARY", &summary)
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci pr-status");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(
        !summary.exists(),
        "the version PR writes no summary either: {stderr}"
    );
    assert!(
        stderr.contains("no job summary was written"),
        "the configured channel is the one named: {stderr}"
    );
    assert!(
        !stderr.contains("coverage comment"),
        "a channel nobody asked for is not mentioned: {stderr}"
    );
}

#[test]
fn no_opinion_deletes_a_leftover_bot_comment() {
    let root = temp_repo("stale");
    cargo_package(&root, "demo", "0.1.0");
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

/// With no token and no `GITHUB_STEP_SUMMARY`, stdout is the last channel the
/// plan can reach. Nobody receiving it means the report reached nowhere, and
/// the step must not read as ok.
#[test]
fn a_plan_nobody_can_receive_is_not_success() {
    let root = planned_repo("dead-stdout");
    let writer = support::dead_stdout();
    let output = bin(&root)
        .args(["ci", "pr-status", "--from", "HEAD~1"])
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_STEP_SUMMARY")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_EVENT_PATH", event_path(&root, 4))
        .stdout(writer)
        .output()
        .expect("oakum ci pr-status");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("the plan could not be delivered to stdout"),
        "{stderr}"
    );
}

/// The stdout fallback carries the plan as-is: one trailing newline, no
/// newline added or lost, the same bytes the job summary would receive.
#[test]
fn the_stdout_fallback_carries_the_plan_as_is() {
    let root = planned_repo("stdout-fallback");
    let output = bin(&root)
        .args(["ci", "pr-status", "--from", "HEAD~1"])
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_STEP_SUMMARY")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_EVENT_PATH", event_path(&root, 4))
        .output()
        .expect("oakum ci pr-status");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("## Release plan"), "{stdout}");
    assert!(
        stdout.ends_with('\n') && !stdout.ends_with("\n\n"),
        "written as-is, no newline added or lost: {stdout:?}"
    );
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
    assert!(
        stderr.contains("no pull request number could be read"),
        "{stderr}"
    );
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
    assert!(
        stderr.contains("no pull request number could be read"),
        "{stderr}"
    );
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
    assert!(
        stderr.contains("no pull request number could be read"),
        "{stderr}"
    );
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
    cargo_package(&root, "demo", "0.1.0");
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
    fs::create_dir_all(&out).expect("emit dir");
    fs::write(
        out.join("oakum-pr-comment.md"),
        "stale plan from yesterday\n",
    )
    .expect("stale artifact");
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
        !body.contains("stale plan from yesterday"),
        "opinion emit must overwrite a stale artifact: {body}"
    );
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
    cargo_package(&root, "demo", "0.1.0");
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
    cargo_package(&root, "demo", "0.1.0");
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

#[test]
fn no_config_emits_on_defaults_and_says_so() {
    let root = planned_repo("no-config");
    fs::remove_file(root.join(".changeset/_config.toml")).expect("drop config");
    commit(&root, "drop config");
    let server = MockServer::start();
    let out = root.join("comment-out");
    fs::create_dir_all(&out).expect("emit dir");
    let output = bin(&root)
        .args([
            "ci",
            "pr-status",
            "--from",
            "HEAD~2",
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
        "a reader keeps its defaults: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(
            "`.changeset/_config.toml` not found; defaults in effect (run `oakum init` or `oakum migrate`)"
        ),
        "{stderr}"
    );
    let body = fs::read_to_string(out.join("oakum-pr-comment.md")).expect("emitted comment");
    assert!(
        body.contains("demo"),
        "the plan still renders on defaults: {body}"
    );
}

/// A mixed workspace whose private member changed: nothing is planned and
/// nothing is uncovered, so the comment's content rests entirely on the
/// unmanaged report. Measured to matter — widening the emit gate without
/// widening the renderer posted a body that was nothing but the invisible
/// marker, and the whole suite passed byte-identically either way.
#[test]
fn a_comment_is_never_the_sticky_marker_alone() {
    let root = temp_repo("unmanaged-comment");
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
    write_config(&root, "pr-status = \"comment\"\n");
    init_git(&root);
    commit(&root, "init");
    fs::write(root.join("beta/src/lib.rs"), "// changed\n").expect("edit");
    commit(&root, "touch beta");

    let server = MockServer::start();
    let hit = server.mock(|when, then| {
        when.any_request();
        then.status(500).body("emit-comment must not call GitHub");
    });

    let out = root.join("comment-out");
    fs::create_dir_all(&out).expect("emit dir");
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
        body.trim_end() != "<!-- oakum:pr-plan -->",
        "a comment worth emitting has something in it: {body:?}"
    );
    assert!(body.contains("beta"), "{body}");
}

/// `ci pr-status` requires the coverage look, where `status` reports and
/// continues. That difference is the whole reason `CoverageMode` exists:
/// flipping the call site to `Reported` left all 2180 tests green, and the
/// mutant posted a comment claiming a plan over a history it had not read. A
/// shallow clone is the shape that reaches it — `actions/checkout` clones that
/// way by default.
#[test]
fn a_shallow_clone_refuses_the_comment_rather_than_claiming_coverage() {
    let root = planned_repo("shallow-refuses");
    let shallow = sibling(&root, "shallow");
    git(
        &root,
        &[
            "-c",
            "protocol.file.allow=always",
            "clone",
            "--depth=1",
            "--no-local",
            root.to_str().expect("utf-8"),
            shallow.to_str().expect("utf-8"),
        ],
    );
    write_config(&shallow, "pr-status = \"comment\"\n");

    let server = MockServer::start();
    let hit = server.mock(|when, then| {
        when.any_request();
        then.status(500)
            .body("a refused look must not reach GitHub");
    });
    let out = server_dir(&shallow);
    let output = bin(&shallow)
        .args([
            "ci",
            "pr-status",
            "--emit-comment",
            out.to_str().expect("utf-8 path"),
        ])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_EVENT_PATH", event_path(&shallow, 4))
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci pr-status");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "a look that did not run must not produce a comment: {stderr}"
    );
    assert!(stderr.contains("shallow clone"), "{stderr}");
    hit.assert_calls(0);
    assert!(
        !out.join("oakum-pr-comment.md").exists(),
        "no comment may be emitted from a refused look"
    );
}

/// The selection is validated before any look and before GitHub is touched:
/// a name the workspace does not have refuses the comment rather than
/// emitting one for an emptied selection.
#[test]
fn an_unknown_include_name_refuses_the_comment() {
    let root = planned_repo("unknown-include");
    write_config(&root, "include = [\"ghost\"]\n");
    let server = MockServer::start();
    let hit = server.mock(|when, then| {
        when.any_request();
        then.status(500).body("must not be called");
    });
    let output = bin(&root)
        .args(["ci", "pr-status", "--from", "HEAD~1"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_EVENT_PATH", event_path(&root, 4))
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci pr-status");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown package name in include/exclude: ghost"),
        "{stderr}"
    );
    hit.assert_calls(0);
}

/// The config is read once per run: the config that chose the channel is the
/// config that built the plan. Measured before this: two reads, the second
/// inside the state builder. `status` reads it once too.
#[test]
fn the_config_is_read_once_per_run() {
    for (label, args) in [
        ("pr-status", vec!["ci", "pr-status", "--from", "HEAD~1"]),
        ("status", vec!["status"]),
    ] {
        let root = planned_repo(&format!("config-reads-{label}"));
        let log = root.join("config-reads.log");
        let output = bin(&root)
            .args(&args)
            .env("OAKUM_TEST_CONFIG_READS", &log)
            .env_remove("GITHUB_TOKEN")
            .env_remove("GH_TOKEN")
            .env("GITHUB_REPOSITORY", "oakoss/oakum")
            .env("GITHUB_EVENT_PATH", event_path(&root, 4))
            .env("GITHUB_STEP_SUMMARY", root.join("summary.md"))
            .output()
            .expect("oakum");
        assert!(
            output.status.success(),
            "{label}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let reads = fs::read_to_string(&log).expect("the debug build logs each read");
        assert_eq!(reads.lines().count(), 1, "{label}: {reads:?}");
    }
}

fn server_dir(root: &Path) -> PathBuf {
    let dir = root.join("comment-out");
    fs::create_dir_all(&dir).expect("emit dir");
    dir
}
