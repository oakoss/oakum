//! `oakum release` tags and creates GitHub releases (`okm-mog`).

#![allow(clippy::disallowed_methods)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use httpmock::prelude::*;
use serde_json::json;

fn bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_oakum"));
    cmd.env_remove("GITHUB_GRAPHQL_URL");
    cmd.env_remove("GITHUB_ACTIONS");
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("GIT_CONFIG_NOSYSTEM", "1");
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null");
    cmd
}

fn git(root: &Path, args: &[&str]) {
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
        .join(format!("oakum-release-{label}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp repo");
    git(&dir, &["init", "-b", "main"]);
    let hooks = dir.join("no-hooks");
    fs::create_dir(&hooks).expect("no-hooks");
    git(&dir, &["config", "core.hooksPath", "no-hooks"]);
    dir
}

fn cargo_package(root: &Path, name: &str, version: &str) {
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

fn commit(root: &Path, message: &str) {
    git(root, &["add", "-A"]);
    git(
        root,
        &[
            "-c",
            "user.email=oakum@test",
            "-c",
            "user.name=oakum",
            "commit",
            "--no-verify",
            "-m",
            message,
        ],
    );
}

fn oakum_release(root: &Path) -> (bool, String, String) {
    let out = bin()
        .arg("release")
        .current_dir(root)
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN")
        .output()
        .expect("oakum release");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn local_tags(root: &Path) -> String {
    let out = Command::new("git")
        .args(["tag", "--list"])
        .current_dir(root)
        .output()
        .expect("git tag");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn assert_annotated(root: &Path, name: &str) {
    let out = Command::new("git")
        .args(["cat-file", "-t", name])
        .current_dir(root)
        .output()
        .expect("cat-file");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "tag", "{name}");
}

fn pending_demo(label: &str) -> PathBuf {
    let root = temp_git_repo(label);
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    cargo_package(&root, "demo", "0.1.1");
    commit(&root, "version");
    root
}

fn add_bare_origin(root: &Path) -> PathBuf {
    let remote = root.parent().expect("parent").join(format!(
        "{}-origin.git",
        root.file_name().expect("name").to_string_lossy()
    ));
    let _ = fs::remove_dir_all(&remote);
    git(root, &["init", "--bare", remote.to_str().expect("utf-8")]);
    git(
        root,
        &["remote", "add", "origin", remote.to_str().expect("utf-8")],
    );
    remote
}

#[test]
fn nothing_to_release_when_manifest_matches_tag() {
    let root = temp_git_repo("clean");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    let (ok, stdout, stderr) = oakum_release(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("nothing to release"), "{stdout}");
}

#[test]
fn leftover_tag_is_unverified_and_creates_no_tag() {
    let root = temp_git_repo("leftover");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "other-v1.0.0"]);
    let check = bin()
        .arg("check")
        .current_dir(&root)
        .output()
        .expect("check");
    let release = bin()
        .arg("release")
        .current_dir(&root)
        .env_remove("GITHUB_TOKEN")
        .env_remove("GH_TOKEN")
        .output()
        .expect("release");
    assert!(!check.status.success());
    assert!(!release.status.success());
    let check_err = String::from_utf8_lossy(&check.stderr);
    let release_err = String::from_utf8_lossy(&release.stderr);
    assert!(check_err.contains("unverified"), "{check_err}");
    assert!(release_err.contains("unverified"), "{release_err}");
    assert!(release_err.contains("other-v1.0.0"), "{release_err}");
    assert_eq!(local_tags(&root).trim(), "other-v1.0.0");
}

#[test]
fn tag_drift_fails_check_and_release_needs_a_token() {
    let root = pending_demo("drift-token");
    add_bare_origin(&root);
    let check = bin()
        .arg("check")
        .current_dir(&root)
        .output()
        .expect("check");
    assert!(!check.status.success());
    let check_err = String::from_utf8_lossy(&check.stderr);
    assert!(check_err.contains("above tagged"), "{check_err}");
    let (ok, stdout, stderr) = oakum_release(&root);
    assert!(!ok, "{stdout}{stderr}");
    assert!(
        stderr.contains("GITHUB_TOKEN") && stderr.contains("GH_TOKEN"),
        "{stderr}"
    );
    assert!(!stderr.contains("bumped without a tag"), "{stderr}");
    assert_eq!(local_tags(&root).trim(), "v0.1.0");
}

#[test]
fn skip_ci_head_creates_no_tag() {
    let root = temp_git_repo("skip");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    cargo_package(&root, "demo", "0.1.1");
    commit(&root, "version [skip ci]");
    add_bare_origin(&root);
    let server = MockServer::start();
    let lookup = mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stderr_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("skip-ci"), "{stderr}");
    assert_eq!(local_tags(&root).trim(), "v0.1.0");
    lookup.assert_calls(0);
    create.assert_calls(0);
}

#[test]
fn tool_version_mismatch_creates_no_tag() {
    let root = pending_demo("toolver");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        "tool-version = \"9.9.9\"\n",
    )
    .expect("config");
    let (ok, stdout, stderr) = oakum_release(&root);
    assert!(!ok, "{stdout}{stderr}");
    assert!(stderr.contains("upgrade"), "{stderr}");
    assert_eq!(local_tags(&root).trim(), "v0.1.0");
}

#[test]
fn creates_github_release_when_remote_tag_already_at_head() {
    let root = pending_demo("remote-at-head");
    add_bare_origin(&root);
    git(&root, &["tag", "v0.1.1"]);
    git(&root, &["push", "origin", "refs/tags/v0.1.1"]);
    git(&root, &["tag", "-d", "v0.1.1"]);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let out = release_cmd(&root, &server);
    assert!(
        out.status.success(),
        "{}{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    assert!(
        stdout_of(&out).contains("https://github.com/oakoss/oakum/releases/tag/v0.1.1"),
        "{}",
        stdout_of(&out)
    );
    create.assert();
    assert_eq!(local_tags(&root).trim(), "v0.1.0");
}

#[test]
fn remote_tag_at_other_commit_creates_no_github_release() {
    let root = temp_git_repo("remote-other");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    git(&root, &["tag", "v0.1.1"]);
    add_bare_origin(&root);
    git(&root, &["push", "origin", "refs/tags/v0.1.1"]);
    git(&root, &["tag", "-d", "v0.1.1"]);
    cargo_package(&root, "demo", "0.1.1");
    commit(&root, "version");
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("already has tag"), "{stderr}");
    create.assert_calls(0);
    assert!(
        !local_tags(&root).contains("v0.1.1"),
        "{}",
        local_tags(&root)
    );
}

#[test]
fn tags_and_creates_a_github_release() {
    let root = pending_demo("happy");
    add_bare_origin(&root);
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/releases/tags/v0.1.1");
        then.status(404).body("Not Found");
    });
    let create = server.mock(|when, then| {
        when.method(POST)
            .path("/repos/oakoss/oakum/releases")
            .body_includes("\"tag_name\":\"v0.1.1\"");
        then.status(201).json_body(json!({
            "html_url": "https://github.com/oakoss/oakum/releases/tag/v0.1.1"
        }));
    });
    let out = bin()
        .arg("release")
        .current_dir(&root)
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .output()
        .expect("release");
    assert!(
        out.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("https://github.com/oakoss/oakum/releases/tag/v0.1.1"),
        "{stdout}"
    );
    create.assert();
    assert!(
        local_tags(&root).contains("v0.1.1"),
        "{}",
        local_tags(&root)
    );
    assert_annotated(&root, "v0.1.1");
    let remote_tags = Command::new("git")
        .args(["ls-remote", "--tags", "origin"])
        .current_dir(&root)
        .output()
        .expect("ls-remote");
    let listed = String::from_utf8_lossy(&remote_tags.stdout);
    assert!(listed.contains("refs/tags/v0.1.1"), "{listed}");
}

#[cfg(unix)]
#[test]
fn honor_git_calls_repo_gpg_program() {
    use std::os::unix::fs::PermissionsExt;

    let root = pending_demo("honor-git");
    add_bare_origin(&root);
    git(&root, &["config", "user.email", "oakum@test"]);
    git(&root, &["config", "user.name", "oakum"]);
    let tools = root.parent().expect("parent").join(format!(
        "{}-sign",
        root.file_name().expect("name").to_string_lossy()
    ));
    fs::create_dir_all(&tools).expect("sign tools");
    let log = tools.join("fake-gpg.log");
    let stub = tools.join("fake-gpg");
    let editor_ran = tools.join("editor-ran");
    let editor = tools.join("fake-editor");
    fs::write(
        &stub,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$0 $*\" >> '{}'\nexit 1\n",
            log.display()
        ),
    )
    .expect("gpg stub");
    fs::write(
        &editor,
        format!(
            "#!/bin/sh\necho ran >> '{}'\nexit 1\n",
            editor_ran.display()
        ),
    )
    .expect("editor stub");
    for path in [&stub, &editor] {
        let mut perm = fs::metadata(path).expect("meta").permissions();
        perm.set_mode(0o755);
        fs::set_permissions(path, perm).expect("chmod");
    }
    git(
        &root,
        &["config", "gpg.program", stub.to_str().expect("utf-8 stub")],
    );
    git(&root, &["config", "tag.gpgsign", "true"]);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let out = bin()
        .arg("release")
        .current_dir(&root)
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GIT_EDITOR", editor.to_str().expect("utf-8 editor"))
        .output()
        .expect("release");
    let recorded = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        recorded.contains("fake-gpg"),
        "gpg log: {recorded:?} stdout={} stderr={}",
        stdout_of(&out),
        stderr_of(&out)
    );
    assert!(
        !out.status.success(),
        "stdout={} stderr={} gpg log: {recorded:?}",
        stdout_of(&out),
        stderr_of(&out)
    );
    assert!(!editor_ran.exists(), "editor ran");
    create.assert_calls(0);
    assert_eq!(local_tags(&root).trim(), "v0.1.0");
}

#[test]
fn github_release_already_exists_creates_no_tag() {
    let root = pending_demo("github-exists");
    add_bare_origin(&root);
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/releases/tags/v0.1.1");
        then.status(200).json_body(json!({
            "html_url": "https://github.com/oakoss/oakum/releases/tag/v0.1.1",
            "tag_name": "v0.1.1"
        }));
    });
    let create = mock_create(&server, "v0.1.1", 201);
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("already has a release"), "{stderr}");
    create.assert_calls(0);
    assert_eq!(local_tags(&root).trim(), "v0.1.0");
}

#[test]
fn current_tag_not_at_head_is_not_resumed() {
    let root = temp_git_repo("local-other");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    cargo_package(&root, "demo", "0.1.1");
    commit(&root, "version");
    git(&root, &["tag", "v0.1.1"]);
    fs::write(root.join("later.txt"), "moved").expect("later");
    commit(&root, "later");
    add_bare_origin(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let out = release_cmd(&root, &server);
    assert!(
        out.status.success(),
        "{}{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    assert!(
        stdout_of(&out).contains("nothing to release"),
        "{}",
        stdout_of(&out)
    );
    create.assert_calls(0);
    let before = Command::new("git")
        .args(["rev-parse", "v0.1.1^{}"])
        .current_dir(&root)
        .output()
        .expect("rev-parse");
    let head = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&root)
        .output()
        .expect("HEAD");
    assert_ne!(
        String::from_utf8_lossy(&before.stdout).trim(),
        String::from_utf8_lossy(&head.stdout).trim()
    );
}

#[test]
fn resumes_github_release_when_head_tag_already_exists() {
    let root = pending_demo("resume");
    add_bare_origin(&root);
    git(&root, &["tag", "v0.1.1"]);
    git(&root, &["push", "origin", "refs/tags/v0.1.1"]);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let out = release_cmd(&root, &server);
    let stdout = stdout_of(&out);
    assert!(out.status.success(), "{stdout}{}", stderr_of(&out));
    assert!(!stdout.contains("nothing to release"), "{stdout}");
    create.assert();
    assert!(
        stdout.contains("https://github.com/oakoss/oakum/releases/tag/v0.1.1"),
        "{stdout}"
    );
    assert!(
        local_tags(&root).contains("v0.1.1"),
        "{}",
        local_tags(&root)
    );
}

#[test]
fn resumes_github_release_when_annotated_head_tag_already_exists() {
    let root = pending_demo("resume-annotated");
    add_bare_origin(&root);
    git(
        &root,
        &[
            "-c",
            "user.email=oakum@test",
            "-c",
            "user.name=oakum",
            "tag",
            "-a",
            "v0.1.1",
            "-m",
            "v0.1.1",
        ],
    );
    git(&root, &["push", "origin", "refs/tags/v0.1.1"]);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let out = release_cmd(&root, &server);
    let stdout = stdout_of(&out);
    assert!(out.status.success(), "{stdout}{}", stderr_of(&out));
    assert!(!stdout.contains("nothing to release"), "{stdout}");
    create.assert();
    assert!(
        stdout.contains("https://github.com/oakoss/oakum/releases/tag/v0.1.1"),
        "{stdout}"
    );
    assert!(
        local_tags(&root).contains("v0.1.1"),
        "{}",
        local_tags(&root)
    );
}

#[test]
fn existing_github_release_on_head_tag_is_nothing_to_release() {
    let root = pending_demo("resume-found");
    add_bare_origin(&root);
    git(&root, &["tag", "v0.1.1"]);
    git(&root, &["push", "origin", "refs/tags/v0.1.1"]);
    let server = MockServer::start();
    let lookup = server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/releases/tags/v0.1.1");
        then.status(200).json_body(json!({
            "html_url": "https://github.com/oakoss/oakum/releases/tag/v0.1.1",
            "tag_name": "v0.1.1"
        }));
    });
    let create = mock_create(&server, "v0.1.1", 201);
    let out = release_cmd(&root, &server);
    assert!(
        out.status.success(),
        "{}{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    assert!(
        stdout_of(&out).contains("nothing to release"),
        "{}",
        stdout_of(&out)
    );
    lookup.assert();
    create.assert_calls(0);
}

#[test]
fn resume_lookup_500_is_unverified_and_creates_no_release() {
    let root = pending_demo("resume-500");
    add_bare_origin(&root);
    git(&root, &["tag", "v0.1.1"]);
    git(&root, &["push", "origin", "refs/tags/v0.1.1"]);
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/releases/tags/v0.1.1");
        then.status(500).body("boom");
    });
    let create = mock_create(&server, "v0.1.1", 201);
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("unverified"), "{stderr}");
    create.assert_calls(0);
    assert!(
        local_tags(&root).contains("v0.1.1"),
        "{}",
        local_tags(&root)
    );
}

#[test]
fn create_release_500_reports_pushed_and_keeps_the_tag() {
    let root = pending_demo("partial");
    add_bare_origin(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 500);
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("tags are not deleted"), "{stderr}");
    assert!(stderr.contains("  v0.1.1 (pushed)"), "{stderr}");
    assert!(stderr.contains("remaining:\n  v0.1.1"), "{stderr}");
    create.assert();
    assert!(
        local_tags(&root).contains("v0.1.1"),
        "{}",
        local_tags(&root)
    );
    let remote_tags = Command::new("git")
        .args(["ls-remote", "--tags", "origin"])
        .current_dir(&root)
        .output()
        .expect("ls-remote");
    let listed = String::from_utf8_lossy(&remote_tags.stdout);
    assert!(listed.contains("refs/tags/v0.1.1"), "{listed}");
}

#[test]
fn dirty_resume_creates_no_github_release() {
    let root = pending_demo("dirty-resume");
    add_bare_origin(&root);
    git(&root, &["tag", "v0.1.1"]);
    git(&root, &["push", "origin", "refs/tags/v0.1.1"]);
    fs::write(root.join("extra.txt"), "dirty").expect("dirty file");
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("dirty"), "{stderr}");
    create.assert_calls(0);
}

#[test]
fn pending_sibling_is_not_blocked_by_older_current_tag() {
    let root = temp_git_repo("sibling");
    write_workspace(&root, &[("alpha", "0.1.0"), ("beta", "0.1.0")]);
    commit(&root, "init");
    git(&root, &["tag", "alpha/v0.1.0"]);
    git(&root, &["tag", "beta/v0.1.0"]);
    write_workspace(&root, &[("alpha", "0.1.0"), ("beta", "0.1.1")]);
    commit(&root, "version");
    add_bare_origin(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "alpha%2Fv0.1.0");
    mock_lookup_empty(&server, "beta%2Fv0.1.1");
    let create_alpha = mock_create(&server, "alpha/v0.1.0", 201);
    let create_beta = mock_create(&server, "beta/v0.1.1", 201);
    let out = release_cmd(&root, &server);
    assert!(
        out.status.success(),
        "{}{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    create_alpha.assert_calls(0);
    create_beta.assert();
    assert!(
        local_tags(&root).contains("beta/v0.1.1"),
        "{}",
        local_tags(&root)
    );
}

#[test]
fn push_failure_after_tag_reports_tagged() {
    let root = pending_demo("push-fail");
    let remote = add_bare_origin(&root);
    let hook = remote.join("hooks/pre-receive");
    fs::write(&hook, "#!/bin/sh\nexit 1\n").expect("hook");
    let mut perm = fs::metadata(&hook).expect("meta").permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        perm.set_mode(0o755);
    }
    fs::set_permissions(&hook, perm).expect("chmod");
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("v0.1.1 (tagged)"), "{stderr}");
    assert!(!stderr.contains("(pushed)"), "{stderr}");
    create.assert_calls(0);
    assert!(
        local_tags(&root).contains("v0.1.1"),
        "{}",
        local_tags(&root)
    );
}

#[test]
fn pending_with_token_and_no_remote_is_unverified() {
    let root = pending_demo("no-remote");
    let server = MockServer::start();
    let create = mock_create(&server, "v0.1.1", 201);
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("no remotes"), "{stderr}");
    create.assert_calls(0);
    assert_eq!(local_tags(&root).trim(), "v0.1.0");
}

#[test]
fn tag_format_whitespace_creates_no_tag() {
    let root = pending_demo("ws-fmt");
    write_release_config(&root, "tag-format = \"v {{ version }}\"\n");
    commit(&root, "config");
    add_bare_origin(&root);
    let server = MockServer::start();
    let create = mock_create(&server, "v0.1.1", 201);
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("whitespace"), "{stderr}");
    create.assert_calls(0);
    assert_eq!(local_tags(&root).trim(), "v0.1.0");
}

#[test]
fn tag_format_invalid_ref_creates_no_tag() {
    let root = pending_demo("bad-ref");
    write_release_config(&root, "tag-format = \"foo..bar\"\n");
    commit(&root, "config");
    add_bare_origin(&root);
    let server = MockServer::start();
    let create = mock_create(&server, "foo..bar", 201);
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("invalid git ref"), "{stderr}");
    create.assert_calls(0);
    assert_eq!(local_tags(&root).trim(), "v0.1.0");
}

#[test]
fn tag_format_unreadable_shape_creates_no_tag() {
    let root = pending_demo("unread-fmt");
    write_release_config(&root, "tag-format = \"release-{{ version }}\"\n");
    commit(&root, "config");
    add_bare_origin(&root);
    let server = MockServer::start();
    let create = mock_create(&server, "release-0.1.1", 201);
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("leftover"), "{stderr}");
    create.assert_calls(0);
    assert_eq!(local_tags(&root).trim(), "v0.1.0");
}

#[test]
fn dirty_worktree_creates_no_tag() {
    let root = pending_demo("dirty");
    add_bare_origin(&root);
    fs::write(root.join("extra.txt"), "dirty").expect("dirty file");
    let server = MockServer::start();
    let create = mock_create(&server, "v0.1.1", 201);
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("dirty"), "{stderr}");
    create.assert_calls(0);
    assert_eq!(local_tags(&root).trim(), "v0.1.0");
}

#[test]
fn untagged_ahead_plans_the_manifest_version() {
    let root = temp_git_repo("untagged");
    cargo_package(&root, "demo", "0.2.0");
    commit(&root, "init");
    add_bare_origin(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.2.0");
    let create = mock_create(&server, "v0.2.0", 201);
    let out = release_cmd(&root, &server);
    assert!(
        out.status.success(),
        "{}{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    create.assert();
    assert!(
        local_tags(&root).contains("v0.2.0"),
        "{}",
        local_tags(&root)
    );
    assert_annotated(&root, "v0.2.0");
}

#[test]
fn tag_format_collision_creates_no_tag() {
    let root = temp_git_repo("collide");
    write_workspace(&root, &[("alpha", "0.2.0"), ("beta", "0.2.0")]);
    write_release_config(&root, "tag-format = \"v{{ version }}\"\n");
    commit(&root, "init");
    add_bare_origin(&root);
    let server = MockServer::start();
    let create = mock_create(&server, "v0.2.0", 201);
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("more than one package"), "{stderr}");
    create.assert_calls(0);
    assert!(local_tags(&root).trim().is_empty(), "{}", local_tags(&root));
}

#[test]
fn tags_two_packages_one_at_a_time() {
    let root = temp_git_repo("two");
    write_workspace(&root, &[("alpha", "0.1.0"), ("beta", "0.1.0")]);
    commit(&root, "init");
    git(&root, &["tag", "alpha/v0.1.0"]);
    git(&root, &["tag", "beta/v0.1.0"]);
    write_workspace(&root, &[("alpha", "0.1.1"), ("beta", "0.1.1")]);
    commit(&root, "version");
    add_bare_origin(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "alpha%2Fv0.1.1");
    mock_lookup_empty(&server, "beta%2Fv0.1.1");
    let create_alpha = mock_create(&server, "alpha/v0.1.1", 201);
    let create_beta = mock_create(&server, "beta/v0.1.1", 500);
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("alpha/v0.1.1"), "{stderr}");
    assert!(stderr.contains("beta/v0.1.1 (pushed)"), "{stderr}");
    assert!(stderr.contains("tags are not deleted"), "{stderr}");
    create_alpha.assert();
    create_beta.assert();
    assert!(
        local_tags(&root).contains("alpha/v0.1.1"),
        "{}",
        local_tags(&root)
    );
    assert!(
        local_tags(&root).contains("beta/v0.1.1"),
        "{}",
        local_tags(&root)
    );
    assert_annotated(&root, "alpha/v0.1.1");
    assert_annotated(&root, "beta/v0.1.1");
}

#[test]
fn both_intent_mechanisms_off_creates_no_tag() {
    let root = pending_demo("no-intent");
    write_release_config(
        &root,
        "change-files = false\nconventional-commits = false\n",
    );
    let (ok, stdout, stderr) = oakum_release(&root);
    assert!(!ok, "{stdout}{stderr}");
    assert!(stderr.contains("change-files"), "{stderr}");
    assert!(stderr.contains("conventional-commits"), "{stderr}");
    assert_eq!(local_tags(&root).trim(), "v0.1.0");
}

#[test]
fn actions_warns_that_the_token_may_not_start_workflows() {
    let root = pending_demo("actions");
    add_bare_origin(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let out = bin()
        .arg("release")
        .current_dir(&root)
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("GITHUB_ACTIONS", "true")
        .output()
        .expect("release");
    assert!(
        out.status.success(),
        "{}{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    let stderr = stderr_of(&out);
    assert!(stderr.contains("downstream workflows"), "{stderr}");
    assert!(stderr.contains("GitHub App"), "{stderr}");
    create.assert();
}

fn mock_lookup_empty<'a>(server: &'a MockServer, encoded_tag: &str) -> httpmock::Mock<'a> {
    server.mock(|when, then| {
        when.method(GET)
            .path(format!("/repos/oakoss/oakum/releases/tags/{encoded_tag}"));
        then.status(404).body("Not Found");
    })
}

fn mock_create<'a>(server: &'a MockServer, tag: &str, status: u16) -> httpmock::Mock<'a> {
    let body = format!("\"tag_name\":\"{tag}\"");
    let url = format!("https://github.com/oakoss/oakum/releases/tag/{tag}");
    server.mock(|when, then| {
        when.method(POST)
            .path("/repos/oakoss/oakum/releases")
            .body_includes(body);
        then.status(status).json_body(json!({ "html_url": url }));
    })
}

fn release_cmd(root: &Path, server: &MockServer) -> std::process::Output {
    bin()
        .arg("release")
        .current_dir(root)
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .output()
        .expect("release")
}

fn stdout_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn write_release_config(root: &Path, extra: &str) {
    let version = env!("CARGO_PKG_VERSION");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        format!("tool-version = \"{version}\"\n{extra}"),
    )
    .expect("config");
    fs::write(
        root.join(".mise.toml"),
        format!("[tools]\noakum = \"{version}\"\n"),
    )
    .expect("mise pin");
}

fn write_workspace(root: &Path, members: &[(&str, &str)]) {
    let listed = members
        .iter()
        .map(|(name, _)| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(", ");
    fs::write(
        root.join("Cargo.toml"),
        format!("[workspace]\nresolver = \"2\"\nmembers = [{listed}]\n"),
    )
    .expect("workspace");
    for (name, version) in members {
        let dir = root.join(name);
        fs::create_dir_all(dir.join("src")).expect("src");
        fs::write(
            dir.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2021\"\n"),
        )
        .expect("member Cargo.toml");
        fs::write(dir.join("src/lib.rs"), "").expect("lib.rs");
    }
}
