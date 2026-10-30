//! `oakum ci version-pr` opens or updates the version pull request.

use crate::support;

use std::fs;
use std::path::Path;
use std::process::Command;

use httpmock::prelude::*;
use serde_json::json;
use support::fixture::{cargo_package, git_output, oakum, plain_repo, Fixture};

fn bin(root: &Path) -> Command {
    let mut cmd = oakum(root);
    cmd.env_remove("GITHUB_GRAPHQL_URL");
    cmd
}

fn temp_repo(label: &str) -> Fixture {
    let root = plain_repo("version-pr", label);
    fs::create_dir(root.join(".git")).expect("fixture .git");
    support::fixture::pin_oakum(&root);
    root
}

fn write_config(root: &Path) {
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(root.join(".changeset/_config.toml"), "").expect("config");
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

fn commit_head(root: &Path) -> String {
    let _ = fs::remove_dir_all(root.join(".git"));
    for args in [&["init"][..], &["add", "-A"], &["commit", "-m", "fixture"]] {
        let output = git(root, args);
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let output = git(root, &["rev-parse", "HEAD"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("HEAD utf-8")
        .trim()
        .to_owned()
}

fn assert_tree_local(root: &std::path::Path) {
    assert_tree_local_at(root, "0.1.0");
}

fn assert_tree_local_at(root: &std::path::Path, version: &str) {
    assert!(
        root.join(".changeset/one.md").exists(),
        "working tree stays local"
    );
    assert!(!root.join("CHANGELOG.md").exists());
    let toml = fs::read_to_string(root.join("Cargo.toml")).expect("Cargo.toml");
    assert!(
        toml.contains(&format!("version = \"{version}\"")),
        "working tree stays local: {toml}"
    );
}

const VERSION_COMMIT_OID: &str = "abc123";

fn mock_replace_branch_commit(server: &MockServer, parent_sha: &str) {
    server.mock(|when, then| {
        when.method(GET)
            .path(format!("/repos/oakoss/oakum/git/commits/{parent_sha}"));
        then.status(200)
            .json_body(json!({ "tree": { "sha": "basetree" } }));
    });
    for blob in ["blob1", "blob2", "blob3", "blob4", "blob5"] {
        server.mock(|when, then| {
            when.method(POST).path("/repos/oakoss/oakum/git/blobs");
            then.status(201).json_body(json!({ "sha": blob }));
        });
    }
    server.mock(|when, then| {
        when.method(POST).path("/repos/oakoss/oakum/git/trees");
        then.status(201).json_body(json!({ "sha": "newtree" }));
    });
    server.mock(|when, then| {
        when.method(POST).path("/repos/oakoss/oakum/git/commits");
        then.status(201)
            .json_body(json!({ "sha": VERSION_COMMIT_OID }));
    });
}

fn mock_create_version_branch_ref(server: &MockServer) {
    server.mock(|when, then| {
        when.method(POST)
            .path("/repos/oakoss/oakum/git/refs")
            .body_includes(VERSION_COMMIT_OID);
        then.status(201).json_body(json!({
            "ref": "refs/heads/oakum/version-packages",
            "object": { "sha": VERSION_COMMIT_OID }
        }));
    });
}

fn mock_update_version_branch_ref(server: &MockServer) {
    server.mock(|when, then| {
        when.method(PATCH)
            .path("/repos/oakoss/oakum/git/refs/heads/oakum%2Fversion-packages")
            .body_includes(VERSION_COMMIT_OID)
            .body_includes("\"force\":true");
        then.status(200).json_body(json!({
            "object": { "sha": VERSION_COMMIT_OID }
        }));
    });
}

fn mock_open_pulls(server: &MockServer, body: serde_json::Value) {
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/pulls")
            .query_param("head", "oakoss:oakum/version-packages")
            .query_param("state", "open")
            .query_param("per_page", "100");
        then.status(200).json_body(body);
    });
}

fn mock_closed_pulls(server: &MockServer, body: serde_json::Value) {
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/pulls")
            .query_param("head", "oakoss:oakum/version-packages")
            .query_param("state", "closed")
            .query_param("per_page", "100");
        then.status(200).json_body(body);
    });
}

fn mock_default_head(server: &MockServer, sha: &str) {
    server.mock(|when, then| {
        when.method(GET).path("/repos/oakoss/oakum");
        then.status(200)
            .json_body(json!({ "default_branch": "main" }));
    });
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/git/ref/heads/main");
        then.status(200)
            .json_body(json!({ "object": { "sha": sha } }));
    });
}

#[test]
fn empty_plan_prints_nothing_to_version() {
    let root = temp_repo("empty");
    cargo_package(&root, "demo", "0.1.0");
    write_config(&root);
    let output = bin(&root)
        .args(["ci", "version-pr"])
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci version-pr");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "nothing to version", "{stdout}");
    assert!(!root.join("CHANGELOG.md").exists());
}

fn write_none_changeset(root: &Path) {
    fs::write(
        root.join(".changeset/tests-only.md"),
        "---\ndemo: none\n---\n\nTests only.\n",
    )
    .expect("none bump file");
}

/// A bump file that releases nothing is consumed by the next real release, so
/// a plan that would only delete it opens no pull request and writes no
/// commit.
#[test]
fn a_plan_that_only_consumes_none_bump_files_opens_nothing() {
    let root = temp_repo("deletes-only");
    cargo_package(&root, "demo", "0.1.0");
    write_config(&root);
    write_none_changeset(&root);
    commit_head(&root);
    let server = MockServer::start();
    mock_open_pulls(&server, json!([]));
    let committed = server.mock(|when, then| {
        when.method(POST).path("/repos/oakoss/oakum/git/commits");
        then.status(500).body("must not commit");
    });
    let output = bin(&root)
        .args(["ci", "version-pr"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci version-pr");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("nothing to version: 1 pending bump file(s) release nothing"),
        "{stdout}"
    );
    committed.assert_calls(0);
    assert!(root.join(".changeset/tests-only.md").exists());
}

/// A version pull request an earlier plan opened is refreshed when the plan
/// becomes delete-only, so it stops offering a release whose intent left the
/// default branch.
#[test]
fn a_delete_only_plan_refreshes_an_open_version_pull_request() {
    let root = temp_repo("deletes-only-open");
    cargo_package(&root, "demo", "0.1.0");
    write_config(&root);
    write_none_changeset(&root);
    let sha = commit_head(&root);
    let server = MockServer::start();
    mock_default_head(&server, &sha);
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/git/ref/heads/oakum%2Fversion-packages");
        then.status(200)
            .json_body(json!({ "object": { "sha": "old" } }));
    });
    mock_replace_branch_commit(&server, &sha);
    mock_update_version_branch_ref(&server);
    let open_lookups = server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/pulls")
            .query_param("state", "open");
        then.status(200).json_body(json!([{
            "number": 7,
            "html_url": "https://github.com/oakoss/oakum/pull/7"
        }]));
    });
    let updated = server.mock(|when, then| {
        when.method(PATCH)
            .path("/repos/oakoss/oakum/pulls/7")
            .body_includes("No packages planned");
        then.status(200).json_body(json!({
            "number": 7,
            "html_url": "https://github.com/oakoss/oakum/pull/7"
        }));
    });
    let closed = server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/pulls")
            .query_param("state", "closed");
        then.status(200).json_body(json!([]));
    });
    let output = bin(&root)
        .args(["ci", "version-pr"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci version-pr");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    updated.assert();
    // The pull request found once is the one refreshed; no second lookup.
    open_lookups.assert_calls(1);
    closed.assert_calls(0);
}

#[test]
fn no_config_refuses_before_any_write_or_token_check() {
    let root = temp_repo("no-config");
    cargo_package(&root, "demo", "0.1.0");
    write_patch_changeset(&root, "demo");
    let output = bin(&root)
        .args(["ci", "version-pr"])
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci version-pr");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(
            "unverified: `.changeset/_config.toml` not found; run `oakum init` or `oakum migrate`"
        ),
        "{stderr}"
    );
    assert!(
        !stderr.contains("GITHUB_TOKEN"),
        "the config refusal comes before the token check: {stderr}"
    );
    assert_tree_local(&root);
}

#[test]
fn missing_token_is_an_error_when_there_is_a_plan() {
    let root = temp_repo("no-token");
    cargo_package(&root, "demo", "0.1.0");
    write_config(&root);
    write_patch_changeset(&root, "demo");
    let output = bin(&root)
        .args(["ci", "version-pr"])
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci version-pr");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("GITHUB_TOKEN") && stderr.contains("GH_TOKEN"),
        "{stderr}"
    );
    assert_tree_local(&root);
}

/// A `version` that stopped mid-consume leaves bump files already applied;
/// `ci version-pr` plans through the same refusal before it asks for a token.
#[test]
fn an_interrupted_consume_refuses_before_any_token_is_asked_for() {
    let root = temp_repo("interrupted-consume");
    cargo_package(&root, "demo", "0.1.0");
    write_config(&root);
    write_patch_changeset(&root, "demo");
    std::fs::write(root.join(".changeset/.b.md.oakum-consume.1.2.0"), "").expect("stray");
    let output = bin(&root)
        .args(["ci", "version-pr"])
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci version-pr");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("did not finish"), "{stderr}");
    assert!(!stderr.contains("GITHUB_TOKEN"), "{stderr}");
}

/// A version pull request would bump past a hand-set version no tag records,
/// so `ci version-pr` plans through `version`'s refusal before it asks for a
/// token.
#[test]
fn a_hand_set_untagged_version_refuses_before_any_token_is_asked_for() {
    let root = support::fixture::git_repo("version-pr", "hand-set-untagged");
    cargo_package(&root, "demo", "0.1.1");
    write_config(&root);
    support::fixture::pin_oakum(&root);
    support::fixture::commit(&root, "init");
    write_patch_changeset(&root, "demo");
    let output = bin(&root)
        .args(["ci", "version-pr"])
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci version-pr");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("tag the version you meant"), "{stderr}");
    assert!(!stderr.contains("GITHUB_TOKEN"), "{stderr}");
    let manifest = fs::read_to_string(root.join("Cargo.toml")).expect("manifest");
    assert!(manifest.contains("version = \"0.1.1\""), "{manifest}");
}

/// The wire, not just `pr_title`: a configured `commit-message` moves the commit
/// and leaves the title on its own default. Every other success-path test
/// configures none, so headline and title are the same string there and passing
/// one where the other belongs goes unnoticed. Conventional on purpose, so a
/// mirror that fires only on a message that already parses is caught here too.
#[test]
fn a_configured_commit_message_does_not_reach_the_pull_request_title() {
    let root = temp_repo("commit-message-not-title");
    cargo_package(&root, "demo", "0.1.0");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        "commit-message = \"chore(release): v9.9.9\"\n",
    )
    .expect("config");
    write_patch_changeset(&root, "demo");

    let sha = commit_head(&root);
    let server = MockServer::start();
    mock_default_head(&server, &sha);
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/git/ref/heads/oakum%2Fversion-packages");
        then.status(404).body("missing");
    });
    mock_replace_branch_commit(&server, &sha);
    mock_create_version_branch_ref(&server);
    server.mock(|when, then| {
        when.method(POST).path("/graphql");
        then.status(404).body("derived");
    });
    mock_open_pulls(&server, json!([]));
    mock_closed_pulls(&server, json!([]));
    let created = server.mock(|when, then| {
        when.method(POST)
            .path("/repos/oakoss/oakum/pulls")
            .body_includes("\"title\":\"chore(release): version packages\"");
        then.status(201).json_body(json!({
            "number": 7,
            "html_url": "https://github.com/oakoss/oakum/pull/7"
        }));
    });

    let output = bin(&root)
        .args(["ci", "version-pr"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci version-pr");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    created.assert();
}

#[test]
fn creates_a_pull_request_through_the_github_api() {
    let root = temp_repo("create");
    cargo_package(&root, "demo", "0.1.0");
    write_config(&root);
    write_patch_changeset(&root, "demo");

    let sha = commit_head(&root);
    let server = MockServer::start();
    mock_default_head(&server, &sha);
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/git/ref/heads/oakum%2Fversion-packages");
        then.status(404).body("missing");
    });
    mock_replace_branch_commit(&server, &sha);
    mock_create_version_branch_ref(&server);
    let derived = server.mock(|when, then| {
        when.method(POST).path("/graphql");
        then.status(404).body("derived");
    });
    mock_open_pulls(&server, json!([]));
    mock_closed_pulls(&server, json!([]));
    let created = server.mock(|when, then| {
        when.method(POST)
            .path("/repos/oakoss/oakum/pulls")
            // Spelled out, not `VERSION_HEADLINE`: this and its `update` sibling
            // are the only things pinning the literal, and referring to the
            // constant would compare it with itself.
            .body_includes("\"title\":\"chore(release): version packages\"")
            .body_includes("## Release plan")
            .body_includes("| demo (`cargo`)")
            .body_includes(format!("Generated by oakum {}.", env!("CARGO_PKG_VERSION")));
        then.status(201).json_body(json!({
            "number": 7,
            "html_url": "https://github.com/oakoss/oakum/pull/7"
        }));
    });

    let output = bin(&root)
        .args(["ci", "version-pr"])
        .env("GITHUB_API_URL", server.base_url())
        .env(
            "GITHUB_GRAPHQL_URL",
            format!("{}/custom-graphql", server.base_url()),
        )
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci version-pr");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("https://github.com/oakoss/oakum/pull/7"),
        "{stdout}"
    );
    created.assert();
    derived.assert_calls(0);
    assert_tree_local(&root);
}

/// A workflow's stderr is read by nobody, so the version pull request names
/// a catalog alias the run wrote out, as stderr does once the branch lands.
#[test]
fn a_catalog_alias_written_out_is_named_in_the_pull_request() {
    let root = temp_repo("anchor");
    fs::write(
        root.join("package.json"),
        "{\n  \"name\": \"root\",\n  \"private\": true\n}\n",
    )
    .expect("root manifest");
    fs::write(
        root.join("pnpm-workspace.yaml"),
        "packages:\n  - 'packages/*'\ncatalogs:\n  spare:\n    z: &caret ^0.1.0\ncatalog:\n  a: *caret\n",
    )
    .expect("workspace yaml");
    for (name, deps) in [
        ("a", ""),
        (
            "c",
            ",\n  \"dependencies\": {\n    \"a\": \"catalog:\"\n  }",
        ),
    ] {
        fs::create_dir_all(root.join("packages").join(name)).expect("member dir");
        fs::write(
            root.join("packages").join(name).join("package.json"),
            format!("{{\n  \"name\": \"{name}\",\n  \"version\": \"0.1.0\"{deps}\n}}\n"),
        )
        .expect("member manifest");
    }
    write_config(&root);
    write_patch_changeset(&root, "a");

    let sha = commit_head(&root);
    let server = MockServer::start();
    mock_default_head(&server, &sha);
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/git/ref/heads/oakum%2Fversion-packages");
        then.status(404).body("missing");
    });
    mock_replace_branch_commit(&server, &sha);
    mock_create_version_branch_ref(&server);
    mock_open_pulls(&server, json!([]));
    mock_closed_pulls(&server, json!([]));
    let created = server.mock(|when, then| {
        when.method(POST)
            .path("/repos/oakoss/oakum/pulls")
            .body_includes("### Catalog entries sharing a YAML anchor")
            .body_includes("- pnpm-workspace.yaml: `catalog/a` was the alias `*caret`");
        then.status(201).json_body(json!({
            "number": 7,
            "html_url": "https://github.com/oakoss/oakum/pull/7"
        }));
    });

    let output = bin(&root)
        .args(["ci", "version-pr"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci version-pr");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "stderr: {stderr}");
    assert!(
        stderr.contains("pnpm-workspace.yaml: `catalog/a` was the alias `*caret`"),
        "{stderr}"
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "https://github.com/oakoss/oakum/pull/7\n"
    );
    created.assert();
}

/// The pull request is open once GitHub says so. A workflow doing
/// `URL=$(oakum ci version-pr)` must not get an empty URL and a green step,
/// so the run exits unverified and stderr names the URL that exists.
#[test]
fn a_pull_request_url_nobody_can_receive_is_not_success() {
    let root = temp_repo("dead-stdout");
    cargo_package(&root, "demo", "0.1.0");
    write_config(&root);
    write_patch_changeset(&root, "demo");

    let sha = commit_head(&root);
    let server = MockServer::start();
    mock_default_head(&server, &sha);
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/git/ref/heads/oakum%2Fversion-packages");
        then.status(404).body("missing");
    });
    mock_replace_branch_commit(&server, &sha);
    mock_create_version_branch_ref(&server);
    server.mock(|when, then| {
        when.method(POST).path("/graphql");
        then.status(404).body("derived");
    });
    mock_open_pulls(&server, json!([]));
    mock_closed_pulls(&server, json!([]));
    let created = server.mock(|when, then| {
        when.method(POST).path("/repos/oakoss/oakum/pulls");
        then.status(201).json_body(json!({
            "number": 7,
            "html_url": "https://github.com/oakoss/oakum/pull/7"
        }));
    });

    let writer = support::dead_stdout();
    let output = bin(&root)
        .args(["ci", "version-pr"])
        .env("GITHUB_API_URL", server.base_url())
        .env(
            "GITHUB_GRAPHQL_URL",
            format!("{}/custom-graphql", server.base_url()),
        )
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env_remove("GH_TOKEN")
        .stdout(writer)
        .output()
        .expect("oakum ci version-pr");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains(
            "version pull request https://github.com/oakoss/oakum/pull/7 is open, but its URL could not be delivered"
        ),
        "{stderr}"
    );
    created.assert();
    assert_tree_local(&root);
}

/// Drives `ci version-pr` against an already-open version pull request. The
/// verdict prose itself is a unit test on `author_note`; what needs a server is
/// which response the author is read from.
fn author_note_on_refresh(author: &serde_json::Value) -> String {
    let root = temp_repo("author-note");
    cargo_package(&root, "demo", "0.1.0");
    write_config(&root);
    write_patch_changeset(&root, "demo");

    let sha = commit_head(&root);
    let server = MockServer::start();
    mock_default_head(&server, &sha);
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/git/ref/heads/oakum%2Fversion-packages");
        then.status(200)
            .json_body(json!({ "object": { "sha": "old" } }));
    });
    mock_replace_branch_commit(&server, &sha);
    mock_update_version_branch_ref(&server);
    mock_open_pulls(
        &server,
        json!([{ "number": 7, "html_url": "https://github.com/oakoss/oakum/pull/7" }]),
    );
    server.mock(|when, then| {
        when.method(PATCH).path("/repos/oakoss/oakum/pulls/7");
        then.status(200).json_body(json!({
            "number": 7,
            "html_url": "https://github.com/oakoss/oakum/pull/7",
            "user": author.clone(),
        }));
    });

    let output = bin(&root)
        .args(["ci", "version-pr"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci version-pr");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.lines().count(),
        1,
        "stdout stays the one URL a caller captures: {stdout}"
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The refresh path reads the PATCH response, not the listing that found the
/// pull request — a listing carries no author, so reusing it would report every
/// refresh as unknown.
#[test]
fn refreshing_an_existing_version_pr_reads_the_author_from_the_update() {
    let stderr = author_note_on_refresh(&json!({ "login": "oakoss[bot]", "type": "Bot" }));
    assert!(
        stderr.contains("version pull request author: oakoss[bot] (Bot)"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("not reported by GitHub"),
        "the PATCH response carries the author: {stderr}"
    );
}

#[test]
fn self_host_version_pr_leaves_tool_version_out_of_the_commit() {
    let root = temp_repo("self-host-pr");
    // The Cargo member named `oakum` is this repository's pin, so it sits at
    // the binary's version, tagged so the bump past it is not hand-set.
    fs::remove_dir_all(root.join(".github")).expect("drop the workflow pin");
    cargo_package(&root, "oakum", support::fixture::BINARY_VERSION);
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        format!("tool-version = \"{}\"\n", support::fixture::BINARY_VERSION),
    )
    .expect("config");
    write_patch_changeset(&root, "oakum");

    let sha = commit_head(&root);
    let tag = format!("v{}", support::fixture::BINARY_VERSION);
    assert!(git(&root, &["tag", &tag]).status.success(), "tag");
    let server = MockServer::start();
    mock_default_head(&server, &sha);
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/git/ref/heads/oakum%2Fversion-packages");
        then.status(404).body("missing");
    });
    // httpmock serves the first registered match, so these see each tree
    // before the catch-all in `mock_replace_branch_commit`.
    let config_tree = server.mock(|when, then| {
        when.method(POST)
            .path("/repos/oakoss/oakum/git/trees")
            .body_includes("_config.toml");
        then.status(201).json_body(json!({ "sha": "newtree" }));
    });
    let manifest_tree = server.mock(|when, then| {
        when.method(POST)
            .path("/repos/oakoss/oakum/git/trees")
            .body_includes("Cargo.toml");
        then.status(201).json_body(json!({ "sha": "newtree" }));
    });
    mock_replace_branch_commit(&server, &sha);
    mock_create_version_branch_ref(&server);
    let committed = server.mock(|when, then| {
        when.method(POST)
            .path("/graphql")
            .body_includes("createCommitOnBranch");
        then.status(404).body("graphql unused");
    });
    mock_open_pulls(&server, json!([]));
    mock_closed_pulls(&server, json!([]));
    server.mock(|when, then| {
        when.method(POST).path("/repos/oakoss/oakum/pulls");
        then.status(201).json_body(json!({
            "number": 8,
            "html_url": "https://github.com/oakoss/oakum/pull/8"
        }));
    });

    let output = bin(&root)
        .args(["ci", "version-pr"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .output()
        .expect("oakum ci version-pr");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    committed.assert_calls(0);
    config_tree.assert_calls(0);
    manifest_tree.assert_calls(1);
    assert_tree_local_at(&root, support::fixture::BINARY_VERSION);
}

/// The stamp names the binary that ran, whatever `tool-version` says.
#[test]
fn stamps_the_binary_not_tool_version() {
    let root = temp_repo("stamp");
    cargo_package(&root, "demo", "0.1.0");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        "tool-version = \"0.0.1\"\n",
    )
    .expect("config");
    write_patch_changeset(&root, "demo");

    let sha = commit_head(&root);
    let server = MockServer::start();
    mock_default_head(&server, &sha);
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/git/ref/heads/oakum%2Fversion-packages");
        then.status(404).body("missing");
    });
    mock_replace_branch_commit(&server, &sha);
    mock_create_version_branch_ref(&server);
    mock_open_pulls(&server, json!([]));
    mock_closed_pulls(&server, json!([]));
    let created = server.mock(|when, then| {
        when.method(POST)
            .path("/repos/oakoss/oakum/pulls")
            .body_includes(format!("Generated by oakum {}.", env!("CARGO_PKG_VERSION")));
        then.status(201).json_body(json!({
            "number": 7,
            "html_url": "https://github.com/oakoss/oakum/pull/7"
        }));
    });

    let output = bin(&root)
        .args(["ci", "version-pr"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci version-pr");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    created.assert();
}

#[test]
fn updates_an_existing_pull_request() {
    let root = temp_repo("update");
    cargo_package(&root, "demo", "0.1.0");
    write_config(&root);
    write_patch_changeset(&root, "demo");

    let sha = commit_head(&root);
    let server = MockServer::start();
    mock_default_head(&server, &sha);
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/git/ref/heads/oakum%2Fversion-packages");
        then.status(200)
            .json_body(json!({ "object": { "sha": "old" } }));
    });
    mock_replace_branch_commit(&server, &sha);
    mock_update_version_branch_ref(&server);
    mock_open_pulls(
        &server,
        json!([{
            "number": 7,
            "html_url": "https://github.com/oakoss/oakum/pull/7"
        }]),
    );
    let updated = server.mock(|when, then| {
        when.method(PATCH)
            .path("/repos/oakoss/oakum/pulls/7")
            .body_includes("\"title\":\"chore(release): version packages\"")
            .body_includes("\"state\":\"open\"")
            .body_includes("## Release plan")
            .body_includes("<details open>\\n<summary>demo 0.1.1 (cargo)</summary>")
            .body_includes("patch demo\\n\\n</details>")
            .body_includes(format!("Generated by oakum {}.", env!("CARGO_PKG_VERSION")));
        then.status(200).json_body(json!({
            "number": 7,
            "html_url": "https://github.com/oakoss/oakum/pull/7"
        }));
    });

    let output = bin(&root)
        .args(["ci", "version-pr"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .output()
        .expect("oakum ci version-pr");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("https://github.com/oakoss/oakum/pull/7"),
        "{stdout}"
    );
    updated.assert();
    assert_tree_local(&root);
}

/// The body quotes the section the branch commits: `version` on a twin
/// fixture writes the same plan to disk, and its section must appear verbatim.
#[test]
fn the_body_quotes_the_changelog_section_the_branch_writes() {
    let setup = |label| {
        let root = temp_repo(label);
        cargo_package(&root, "demo", "0.1.0");
        write_config(&root);
        fs::write(
            root.join(".changeset/one.md"),
            "---\ndemo: patch\n---\n\nFirst line of the note.\n\n- one\n- two\n",
        )
        .expect("bump file");
        root
    };
    let twin = setup("quote-twin");
    commit_head(&twin);
    let output = bin(&twin).arg("version").output().expect("oakum version");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let changelog = fs::read_to_string(twin.join("CHANGELOG.md")).expect("changelog");
    let section: Vec<&str> = changelog
        .lines()
        .skip_while(|line| !line.starts_with("## "))
        .skip(1)
        .take_while(|line| !line.starts_with("Generated by oakum"))
        .collect();
    let section = section.join("\n");
    let section = section.trim_matches('\n');
    assert!(section.contains("- two"), "{changelog}");
    let quoted = serde_json::to_string(&format!(
        "<summary>demo 0.1.1 (cargo)</summary>\n\n{section}\n\n</details>"
    ))
    .expect("json");
    let quoted = quoted.trim_matches('"').to_owned();

    let root = setup("quote");
    let sha = commit_head(&root);
    let server = MockServer::start();
    mock_default_head(&server, &sha);
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/git/ref/heads/oakum%2Fversion-packages");
        then.status(200)
            .json_body(json!({ "object": { "sha": "old" } }));
    });
    mock_replace_branch_commit(&server, &sha);
    mock_update_version_branch_ref(&server);
    mock_open_pulls(
        &server,
        json!([{ "number": 7, "html_url": "https://github.com/oakoss/oakum/pull/7" }]),
    );
    let updated = server.mock(|when, then| {
        when.method(PATCH)
            .path("/repos/oakoss/oakum/pulls/7")
            .body_includes(quoted.clone());
        then.status(200).json_body(json!({
            "number": 7,
            "html_url": "https://github.com/oakoss/oakum/pull/7"
        }));
    });
    let output = bin(&root)
        .args(["ci", "version-pr"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci version-pr");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    updated.assert();
}

/// GitHub refuses an over-limit body only after the branch commit is pushed,
/// so the run refuses it before any write.
#[test]
fn a_body_over_githubs_limit_stops_before_any_write() {
    let root = temp_repo("over-limit");
    let name = "a".repeat(66_000);
    cargo_package(&root, &name, "0.1.0");
    write_config(&root);
    write_patch_changeset(&root, &name);
    let sha = commit_head(&root);
    let server = MockServer::start();
    mock_default_head(&server, &sha);
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/git/ref/heads/oakum%2Fversion-packages");
        then.status(404).body("missing");
    });
    mock_open_pulls(&server, json!([]));
    mock_closed_pulls(&server, json!([]));
    let writes = server.mock(|when, then| {
        when.is_true(|request| request.method().as_str() != "GET");
        then.status(500).body("must not write");
    });
    let output = bin(&root)
        .args(["ci", "version-pr"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci version-pr");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("over GitHub's"), "{stderr}");
    assert!(stderr.contains("nothing was pushed"), "{stderr}");
    writes.assert_calls(0);
}

#[test]
fn reopens_a_closed_version_pull_request() {
    let root = temp_repo("reopen-closed");
    cargo_package(&root, "demo", "0.1.0");
    write_config(&root);
    write_patch_changeset(&root, "demo");

    let sha = commit_head(&root);
    let server = MockServer::start();
    mock_default_head(&server, &sha);
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/git/ref/heads/oakum%2Fversion-packages");
        then.status(200)
            .json_body(json!({ "object": { "sha": "old" } }));
    });
    mock_replace_branch_commit(&server, &sha);
    mock_update_version_branch_ref(&server);
    mock_open_pulls(&server, json!([]));
    mock_closed_pulls(
        &server,
        json!([{
            "number": 145,
            "html_url": "https://github.com/oakoss/oakum/pull/145"
        }]),
    );
    let updated = server.mock(|when, then| {
        when.method(PATCH)
            .path("/repos/oakoss/oakum/pulls/145")
            .body_includes("\"state\":\"open\"");
        then.status(200).json_body(json!({
            "number": 145,
            "html_url": "https://github.com/oakoss/oakum/pull/145"
        }));
    });

    let output = bin(&root)
        .args(["ci", "version-pr"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .output()
        .expect("oakum ci version-pr");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("https://github.com/oakoss/oakum/pull/145"),
        "{stdout}"
    );
    updated.assert();
    assert_tree_local(&root);
}

#[test]
fn creates_a_pull_when_only_a_merged_version_pr_exists() {
    let root = temp_repo("merged-closed");
    cargo_package(&root, "demo", "0.1.0");
    write_config(&root);
    write_patch_changeset(&root, "demo");

    let sha = commit_head(&root);
    let server = MockServer::start();
    mock_default_head(&server, &sha);
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/git/ref/heads/oakum%2Fversion-packages");
        then.status(200)
            .json_body(json!({ "object": { "sha": "old" } }));
    });
    mock_replace_branch_commit(&server, &sha);
    mock_update_version_branch_ref(&server);
    mock_open_pulls(&server, json!([]));
    mock_closed_pulls(
        &server,
        json!([{
            "number": 145,
            "html_url": "https://github.com/oakoss/oakum/pull/145",
            "merged_at": "2026-09-01T18:00:00Z"
        }]),
    );
    let created = server.mock(|when, then| {
        when.method(POST).path("/repos/oakoss/oakum/pulls");
        then.status(201).json_body(json!({
            "number": 146,
            "html_url": "https://github.com/oakoss/oakum/pull/146"
        }));
    });
    let updated = server.mock(|when, then| {
        when.method(PATCH).path("/repos/oakoss/oakum/pulls/145");
        then.status(422).body("merged");
    });

    let output = bin(&root)
        .args(["ci", "version-pr"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .output()
        .expect("oakum ci version-pr");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("https://github.com/oakoss/oakum/pull/146"),
        "{stdout}"
    );
    created.assert();
    updated.assert_calls(0);
    assert_tree_local(&root);
}

#[test]
fn five_hundred_is_unverified() {
    let root = temp_repo("unverified");
    cargo_package(&root, "demo", "0.1.0");
    write_config(&root);
    write_patch_changeset(&root, "demo");

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/repos/oakoss/oakum");
        then.status(502).body("bad gateway");
    });

    let output = bin(&root)
        .args(["ci", "version-pr"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .output()
        .expect("oakum ci version-pr");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr.trim_end(),
        "unverified: GitHub /repos/oakoss/oakum returned 502 Bad Gateway",
        "{stderr}"
    );
    assert_tree_local(&root);
}

const MOVED_TIP: &str = "ffffffffffffffffffffffffffffffffffffffff";

/// Runs `ci version-pr` with `main` at `MOVED_TIP` and GitHub's compare of
/// HEAD against it answering `status` with `body`, and asserts nothing was
/// written anywhere.
fn run_with_moved_tip(label: &str, status: u16, body: &str) -> (std::process::Output, String) {
    let root = temp_repo(label);
    cargo_package(&root, "demo", "0.1.0");
    write_config(&root);
    write_patch_changeset(&root, "demo");
    let head = commit_head(&root);

    let server = MockServer::start();
    mock_default_head(&server, MOVED_TIP);
    let compare = server.mock(|when, then| {
        when.method(GET)
            .path(format!("/repos/oakoss/oakum/compare/{head}...{MOVED_TIP}"));
        then.status(status)
            .header("content-type", "application/json")
            .body(body);
    });
    let writes = [POST, PATCH, PUT, DELETE].map(|method| {
        server.mock(|when, then| {
            when.method(method);
            then.status(200).json_body(json!({}));
        })
    });

    let output = bin(&root)
        .args(["ci", "version-pr"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci version-pr");
    compare.assert();
    for write in &writes {
        write.assert_calls(0);
    }
    assert_tree_local(&root);
    (output, head)
}

#[test]
fn a_run_a_newer_push_superseded_writes_nothing_and_succeeds() {
    let (output, head) = run_with_moved_tip("superseded", 200, r#"{"status":"ahead"}"#);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.trim_end(),
        format!(
            "superseded: `main` moved on from checkout HEAD `{head}` to `{MOVED_TIP}`; \
             this run writes nothing, and the version pull request waits for a run on \
             `{MOVED_TIP}`"
        ),
    );
}

#[test]
fn a_head_the_default_branch_does_not_descend_from_is_an_error() {
    for (status, why) in [
        ("behind", "behind it"),
        ("diverged", "diverged from it"),
        ("identical", "identical by GitHub's compare"),
    ] {
        let (output, head) =
            run_with_moved_tip(status, 200, &format!(r#"{{"status":"{status}"}}"#));
        assert_eq!(output.status.code(), Some(1), "{status}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(&format!(
                "checkout HEAD `{head}` is not `main` at `{MOVED_TIP}` ({why})"
            )),
            "{stderr}"
        );
    }
}

#[test]
fn a_head_github_does_not_have_is_an_error() {
    let (output, _) = run_with_moved_tip("head-unknown", 404, r#"{"message":"Not Found"}"#);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("(GitHub's compare returned 404)"),
        "{stderr}"
    );
}

#[test]
fn a_compare_that_failed_is_unverified_not_superseded() {
    for (label, status, body) in [
        ("compare-502", 502, "bad gateway"),
        (
            "compare-403",
            403,
            r#"{"message":"API rate limit exceeded"}"#,
        ),
    ] {
        let (output, _) = run_with_moved_tip(label, status, body);
        assert_eq!(output.status.code(), Some(2), "{label}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), "", "{label}");
    }
}

/// okm-3cbq: the HEAD read adds context to a git error and keeps its class,
/// so one that never answers leaves the run unverified.
#[cfg(unix)]
#[test]
fn a_head_read_that_never_answers_is_unverified() {
    use support::fixture::{ambient_tool, deadline_turn, path_prefixed_by, path_shim};
    let root = temp_repo("head-hung");
    cargo_package(&root, "demo", "0.1.0");
    write_config(&root);
    write_patch_changeset(&root, "demo");
    let sha = commit_head(&root);
    let shim = path_shim(
        &root,
        "git",
        format!(
            "#!/bin/sh\nif [ \"$1\" = rev-parse ] && [ \"$2\" = HEAD ]; then exec sleep 60; fi\nexec {} \"$@\"\n",
            ambient_tool("git").display()
        ),
    );
    let server = MockServer::start();
    mock_default_head(&server, &sha);

    let _turn = deadline_turn();
    let output = bin(&root)
        .args(["ci", "version-pr"])
        .env("PATH", path_prefixed_by(&shim))
        .env("OAKUM_REMOTE_DEADLINE", "2")
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci version-pr");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("unverified: `oakum ci version-pr` needs a git HEAD"),
        "{stderr}"
    );
    assert!(stderr.contains("gave up after 2s"), "{stderr}");
    assert_eq!(stderr.matches("unverified: ").count(), 1, "{stderr}");
    assert_tree_local(&root);
}

#[test]
fn bad_commit_template_does_not_reset_the_branch() {
    let root = temp_repo("bad-template");
    cargo_package(&root, "demo", "0.1.0");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        "commit-message = { file = \"notes.md\" }\n",
    )
    .expect("config");
    fs::write(root.join("notes.md"), "{{").expect("broken template");
    write_patch_changeset(&root, "demo");
    let sha = commit_head(&root);

    let server = MockServer::start();
    mock_default_head(&server, &sha);
    let listed = server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/git/ref/heads/oakum%2Fversion-packages");
        then.status(404).body("missing");
    });
    let reset = server.mock(|when, then| {
        when.method(POST).path("/repos/oakoss/oakum/git/refs");
        then.status(201).json_body(json!({
            "ref": "refs/heads/oakum/version-packages",
            "object": { "sha": sha }
        }));
    });

    let output = bin(&root)
        .args(["ci", "version-pr"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci version-pr");
    assert!(!output.status.success());
    listed.assert_calls(0);
    reset.assert_calls(0);
    assert_tree_local(&root);
}

#[test]
fn blank_title_template_does_not_reset_the_branch() {
    let root = temp_repo("blank-title");
    cargo_package(&root, "demo", "0.1.0");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        "title = { file = \"title.md\" }\n",
    )
    .expect("config");
    fs::write(root.join("title.md"), "   \n").expect("blank title");
    write_patch_changeset(&root, "demo");
    let sha = commit_head(&root);

    let server = MockServer::start();
    mock_default_head(&server, &sha);
    mock_open_pulls(&server, json!([]));
    let reset = server.mock(|when, then| {
        when.method(POST).path("/repos/oakoss/oakum/git/refs");
        then.status(201).json_body(json!({
            "ref": "refs/heads/oakum/version-packages",
            "object": { "sha": sha }
        }));
    });

    let output = bin(&root)
        .args(["ci", "version-pr"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci version-pr");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("title template rendered an empty string"),
        "{stderr}"
    );
    reset.assert_calls(0);
    assert_tree_local(&root);
}

#[test]
fn two_open_pulls_is_an_error() {
    let root = temp_repo("two-pulls");
    cargo_package(&root, "demo", "0.1.0");
    write_config(&root);
    write_patch_changeset(&root, "demo");
    let sha = commit_head(&root);

    let server = MockServer::start();
    mock_default_head(&server, &sha);
    let reset = server.mock(|when, then| {
        when.method(POST).path("/repos/oakoss/oakum/git/refs");
        then.status(201).json_body(json!({
            "ref": "refs/heads/oakum/version-packages",
            "object": { "sha": sha }
        }));
    });
    let committed = server.mock(|when, then| {
        when.method(POST).path("/graphql");
        then.status(404).body("graphql unused");
    });
    mock_open_pulls(
        &server,
        json!([
            {
                "number": 7,
                "html_url": "https://github.com/oakoss/oakum/pull/7"
            },
            {
                "number": 8,
                "html_url": "https://github.com/oakoss/oakum/pull/8"
            }
        ]),
    );
    let patched = server.mock(|when, then| {
        when.method(PATCH).path("/repos/oakoss/oakum/pulls/7");
        then.status(200).json_body(json!({
            "number": 7,
            "html_url": "https://github.com/oakoss/oakum/pull/7"
        }));
    });
    let created = server.mock(|when, then| {
        when.method(POST).path("/repos/oakoss/oakum/pulls");
        then.status(201).json_body(json!({
            "number": 9,
            "html_url": "https://github.com/oakoss/oakum/pull/9"
        }));
    });

    let output = bin(&root)
        .args(["ci", "version-pr"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum ci version-pr");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("multiple open version pull requests on `oakum/version-packages` (2)"),
        "{stderr}"
    );
    reset.assert_calls(0);
    committed.assert_calls(0);
    patched.assert_calls(0);
    created.assert_calls(0);
    assert_tree_local(&root);
}

#[test]
fn empty_github_token_falls_through_to_gh_token() {
    let root = temp_repo("gh-token");
    cargo_package(&root, "demo", "0.1.0");
    write_config(&root);
    write_patch_changeset(&root, "demo");
    let sha = commit_head(&root);

    let server = MockServer::start();
    mock_default_head(&server, &sha);
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/git/ref/heads/oakum%2Fversion-packages");
        then.status(404).body("missing");
    });
    mock_replace_branch_commit(&server, &sha);
    mock_create_version_branch_ref(&server);
    mock_open_pulls(&server, json!([]));
    mock_closed_pulls(&server, json!([]));
    let created = server.mock(|when, then| {
        when.method(POST).path("/repos/oakoss/oakum/pulls");
        then.status(201).json_body(json!({
            "number": 7,
            "html_url": "https://github.com/oakoss/oakum/pull/7"
        }));
    });

    let output = bin(&root)
        .args(["ci", "version-pr"])
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_TOKEN", "")
        .env("GH_TOKEN", "token")
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .output()
        .expect("oakum ci version-pr");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("https://github.com/oakoss/oakum/pull/7"),
        "{stdout}"
    );
    created.assert();
    assert_tree_local(&root);
}
