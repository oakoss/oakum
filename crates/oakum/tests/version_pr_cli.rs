//! `oakum ci version-pr` opens or updates the version pull request.

#![allow(clippy::disallowed_methods)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use httpmock::prelude::*;
use serde_json::json;

fn bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_oakum"));
    cmd.env_remove("GITHUB_GRAPHQL_URL");
    cmd
}

fn temp_repo(label: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("oakum-version-pr-{label}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp repo");
    fs::create_dir(dir.join(".git")).expect("fixture .git");
    dir
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

fn write_config(root: &std::path::Path) {
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        "tool-version = \"0.0.0\"\n",
    )
    .expect("config");
}

fn write_patch_changeset(root: &std::path::Path, name: &str) {
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/one.md"),
        format!("---\n{name}: patch\n---\n\npatch {name}\n"),
    )
    .expect("changeset");
}

fn git(root: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .args(["-c", "user.email=oakum@test", "-c", "user.name=oakum"])
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|err| panic!("git {args:?}: {err}"))
}

fn commit_head(root: &std::path::Path) -> String {
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
    assert!(
        root.join(".changeset/one.md").exists(),
        "working tree stays local"
    );
    assert!(!root.join("CHANGELOG.md").exists());
    let toml = fs::read_to_string(root.join("Cargo.toml")).expect("Cargo.toml");
    assert!(
        toml.contains("version = \"0.1.0\""),
        "working tree stays local: {toml}"
    );
}

fn mock_version_commit(server: &MockServer) {
    mock_version_commit_at(server, "/graphql");
}

fn mock_version_commit_at(server: &MockServer, graphql: &str) {
    server.mock(|when, then| {
        when.method(POST)
            .path(graphql)
            .body_includes("createCommitOnBranch")
            .body_includes(r#""deletions":[{"path":".changeset/one.md"}]"#)
            .body_includes("Cargo.toml")
            .body_includes("CHANGELOG.md")
            .body_includes(r#""headline":"Version Packages""#);
        then.status(200).json_body(json!({
            "data": { "createCommitOnBranch": { "commit": { "oid": "abc123" } } }
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
    cargo_package(&root, "demo");
    write_config(&root);
    let output = bin()
        .current_dir(&root)
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
    assert!(stdout.contains("nothing to version"), "{stdout}");
    assert!(!root.join("CHANGELOG.md").exists());
}

#[test]
fn missing_token_is_an_error_when_there_is_a_plan() {
    let root = temp_repo("no-token");
    cargo_package(&root, "demo");
    write_config(&root);
    write_patch_changeset(&root, "demo");
    let output = bin()
        .current_dir(&root)
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

#[test]
fn creates_a_pull_request_through_the_github_api() {
    let root = temp_repo("create");
    cargo_package(&root, "demo");
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
    server.mock(|when, then| {
        when.method(POST)
            .path("/repos/oakoss/oakum/git/refs")
            .body_includes(&sha);
        then.status(201).json_body(json!({
            "ref": "refs/heads/oakum/version-packages",
            "object": { "sha": sha }
        }));
    });
    mock_version_commit_at(&server, "/custom-graphql");
    let derived = server.mock(|when, then| {
        when.method(POST).path("/graphql");
        then.status(404).body("derived");
    });
    mock_open_pulls(&server, json!([]));
    let created = server.mock(|when, then| {
        when.method(POST)
            .path("/repos/oakoss/oakum/pulls")
            .body_includes("\"title\":\"Version Packages\"")
            .body_includes("## Release plan")
            .body_includes("| demo (`cargo`)")
            .body_includes("Generated by oakum 0.0.0.");
        then.status(201).json_body(json!({
            "number": 7,
            "html_url": "https://github.com/oakoss/oakum/pull/7"
        }));
    });

    let output = bin()
        .current_dir(&root)
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

#[test]
fn updates_an_existing_pull_request() {
    let root = temp_repo("update");
    cargo_package(&root, "demo");
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
    server.mock(|when, then| {
        when.method(PATCH)
            .path("/repos/oakoss/oakum/git/refs/heads/oakum%2Fversion-packages")
            .body_includes(&sha);
        then.status(200)
            .json_body(json!({ "object": { "sha": sha } }));
    });
    mock_version_commit(&server);
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
            .body_includes("\"title\":\"Version Packages\"")
            .body_includes("## Release plan")
            .body_includes("Generated by oakum 0.0.0.");
        then.status(200).json_body(json!({
            "number": 7,
            "html_url": "https://github.com/oakoss/oakum/pull/7"
        }));
    });

    let output = bin()
        .current_dir(&root)
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

#[test]
fn five_hundred_is_unverified() {
    let root = temp_repo("unverified");
    cargo_package(&root, "demo");
    write_config(&root);
    write_patch_changeset(&root, "demo");

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/repos/oakoss/oakum");
        then.status(502).body("bad gateway");
    });

    let output = bin()
        .current_dir(&root)
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
        "error: unverified: GitHub /repos/oakoss/oakum returned 502 Bad Gateway",
        "{stderr}"
    );
    assert_tree_local(&root);
}

#[test]
fn mismatched_default_head_is_an_error() {
    let root = temp_repo("head-mismatch");
    cargo_package(&root, "demo");
    write_config(&root);
    write_patch_changeset(&root, "demo");
    let _sha = commit_head(&root);

    let server = MockServer::start();
    mock_default_head(&server, "ffffffffffffffffffffffffffffffffffffffff");
    let reset = server.mock(|when, then| {
        when.method(POST).path("/repos/oakoss/oakum/git/refs");
        then.status(201).json_body(json!({
            "ref": "refs/heads/oakum/version-packages",
            "object": { "sha": "ffffffffffffffffffffffffffffffffffffffff" }
        }));
    });

    let output = bin()
        .current_dir(&root)
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
        stderr.contains("is not `main` at `ffffffffffffffffffffffffffffffffffffffff`"),
        "{stderr}"
    );
    reset.assert_calls(0);
    assert_tree_local(&root);
}

#[test]
fn bad_commit_template_does_not_reset_the_branch() {
    let root = temp_repo("bad-template");
    cargo_package(&root, "demo");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        "tool-version = \"0.0.0\"\ncommit-message = { file = \"notes.md\" }\n",
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

    let output = bin()
        .current_dir(&root)
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
    cargo_package(&root, "demo");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        "tool-version = \"0.0.0\"\ntitle = { file = \"title.md\" }\n",
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

    let output = bin()
        .current_dir(&root)
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
    cargo_package(&root, "demo");
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
        then.status(200).json_body(json!({
            "data": { "createCommitOnBranch": { "commit": { "oid": "abc123" } } }
        }));
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

    let output = bin()
        .current_dir(&root)
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
    cargo_package(&root, "demo");
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
    server.mock(|when, then| {
        when.method(POST).path("/repos/oakoss/oakum/git/refs");
        then.status(201).json_body(json!({
            "ref": "refs/heads/oakum/version-packages",
            "object": { "sha": sha }
        }));
    });
    mock_version_commit(&server);
    mock_open_pulls(&server, json!([]));
    let created = server.mock(|when, then| {
        when.method(POST).path("/repos/oakoss/oakum/pulls");
        then.status(201).json_body(json!({
            "number": 7,
            "html_url": "https://github.com/oakoss/oakum/pull/7"
        }));
    });

    let output = bin()
        .current_dir(&root)
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
