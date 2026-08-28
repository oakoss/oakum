//! `oakum release` tags and creates GitHub releases (`okm-mog`).

#![allow(clippy::disallowed_methods)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use httpmock::prelude::*;
use serde_json::json;

fn bin() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_oakum"));
    cmd.env_remove("GITHUB_GRAPHQL_URL");
    cmd.env_remove("GITHUB_ACTIONS");
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("GIT_CONFIG_NOSYSTEM", "1");
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null");
    cmd.env("OAKUM_HANDOFF_FAST", "1");
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

/// Two labels that match name one directory, and `temp_git_repo` clears it
/// before use — so a duplicate deletes a sibling test's repository mid-run.
/// Measured: reintroducing one collision failed 2 of 3 runs here, reported as
/// `cargo metadata: Could not locate working directory`.
fn claim_label(label: &str) {
    static CLAIMED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    let claimed = CLAIMED.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()));
    let fresh = claimed
        .lock()
        .expect("label registry")
        .insert(label.to_owned());
    assert!(fresh, "duplicate temp-dir label {label:?}");
}

fn temp_git_repo(label: &str) -> PathBuf {
    claim_label(label);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("oakum-release-{label}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp repo");
    git(&dir, &["init", "-b", "main"]);
    let hooks = dir.join("no-hooks");
    fs::create_dir(&hooks).expect("no-hooks");
    git(&dir, &["config", "core.hooksPath", "no-hooks"]);
    git(&dir, &["config", "user.email", "oakum@test"]);
    git(&dir, &["config", "user.name", "oakum"]);
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

/// Writes `content` beside `path`, then installs it exec-bit-set from a
/// subprocess. `fs::write` here would hold a write fd in this test process;
/// every concurrent test's fork inherits it, and a child exec'ing the file
/// inside that window dies with ETXTBSY. Measured on CI (Linux); the fd must
/// never exist in this process.
#[cfg(unix)]
fn install_executable(path: &std::path::Path, content: impl AsRef<str>) {
    let source = path.with_file_name("installed.source");
    fs::write(&source, content.as_ref()).expect("executable source");
    let installed = Command::new("sh")
        .args(["-c", r#"cat "$1" > "$2" && chmod 755 "$2""#, "sh"])
        .arg(&source)
        .arg(path)
        .status()
        .expect("install executable")
        .success();
    assert!(installed, "installing {} failed", path.display());
}

fn local_tags(root: &Path) -> String {
    let out = Command::new("git")
        .args(["tag", "--list"])
        .current_dir(root)
        .output()
        .expect("git tag");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn commit_at(root: &Path, reference: &str) -> String {
    let out = Command::new("git")
        .args(["rev-parse", &format!("{reference}^{{commit}}")])
        .current_dir(root)
        .output()
        .expect("rev-parse");
    assert!(
        out.status.success(),
        "rev-parse {reference}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
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

fn write_workflow(root: &Path, name: &str, body: &str) {
    let dir = root.join(".github/workflows");
    fs::create_dir_all(&dir).expect("workflows");
    fs::write(dir.join(name), body).expect("workflow");
}

fn head_sha(root: &Path) -> String {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .expect("HEAD");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

#[derive(Clone)]
enum RunHit {
    Empty,
    Run {
        id: u64,
        path: String,
    },
    Runs {
        ids: Vec<u64>,
        path: String,
    },
    RunWithoutPath {
        id: u64,
    },
    RunWithoutEvent {
        id: u64,
        path: String,
    },
    Event {
        id: u64,
        path: String,
        event: &'static str,
    },
    Page(Vec<(u64, String, &'static str)>),
    Incomplete(Vec<(u64, Option<String>, Option<&'static str>)>),
    Fail(u16),
}

fn run_json(sha: &str, id: u64, path: Option<&str>, event: Option<&str>) -> serde_json::Value {
    let mut run = json!({
        "id": id,
        "head_sha": sha,
        "status": "queued",
        "conclusion": null,
        "html_url": format!("https://github.com/oakoss/oakum/actions/runs/{id}")
    });
    if let Some(path) = path {
        run["path"] = json!(path);
    }
    if let Some(event) = event {
        run["event"] = json!(event);
    }
    run
}

fn run_hit_response(sha: &str, hit: &RunHit) -> (u16, serde_json::Value) {
    match hit {
        RunHit::Empty => (200, json!({ "total_count": 0, "workflow_runs": [] })),
        RunHit::Run { id, path } => (
            200,
            json!({
                "total_count": 1,
                "workflow_runs": [run_json(sha, *id, Some(path), Some("push"))]
            }),
        ),
        RunHit::Runs { ids, path } => (
            200,
            json!({
                "total_count": ids.len(),
                "workflow_runs": ids.iter().map(|id| run_json(sha, *id, Some(path), Some("push"))).collect::<Vec<_>>()
            }),
        ),
        RunHit::RunWithoutPath { id } => (
            200,
            json!({
                "total_count": 1,
                "workflow_runs": [run_json(sha, *id, None, Some("push"))]
            }),
        ),
        RunHit::RunWithoutEvent { id, path } => (
            200,
            json!({
                "total_count": 1,
                "workflow_runs": [run_json(sha, *id, Some(path), None)]
            }),
        ),
        RunHit::Event { id, path, event } => (
            200,
            json!({
                "total_count": 1,
                "workflow_runs": [run_json(sha, *id, Some(path), Some(event))]
            }),
        ),
        RunHit::Page(runs) => (
            200,
            json!({
                "total_count": runs.len(),
                "workflow_runs": runs
                    .iter()
                    .map(|(id, path, event)| run_json(sha, *id, Some(path), Some(event)))
                    .collect::<Vec<_>>()
            }),
        ),
        RunHit::Incomplete(runs) => (
            200,
            json!({
                "total_count": runs.len(),
                "workflow_runs": runs
                    .iter()
                    .map(|(id, path, event)| run_json(sha, *id, path.as_deref(), *event))
                    .collect::<Vec<_>>()
            }),
        ),
        RunHit::Fail(status) => (*status, json!({ "message": "error" })),
    }
}

fn mock_workflow_runs<'a>(
    server: &'a MockServer,
    sha: &str,
    path: &str,
    empty: bool,
) -> httpmock::Mock<'a> {
    let (status, runs) = if empty {
        run_hit_response(sha, &RunHit::Empty)
    } else {
        run_hit_response(
            sha,
            &RunHit::Run {
                id: 9,
                path: path.to_owned(),
            },
        )
    };
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/actions/runs")
            .query_param("head_sha", sha)
            .query_param("per_page", "100");
        then.status(status).json_body(runs);
    })
}

fn mock_workflow_hits<'a>(
    server: &'a MockServer,
    sha: &str,
    hits: &[RunHit],
) -> Vec<httpmock::Mock<'a>> {
    let n = Arc::new(AtomicUsize::new(0));
    hits.iter()
        .enumerate()
        .map(|(i, hit)| {
            let n = n.clone();
            let (status, body) = run_hit_response(sha, hit);
            server.mock(move |when, then| {
                when.method(GET)
                    .path("/repos/oakoss/oakum/actions/runs")
                    .query_param("head_sha", sha)
                    .query_param("per_page", "100")
                    .is_true(move |_| {
                        if n.load(Ordering::SeqCst) == i {
                            n.fetch_add(1, Ordering::SeqCst);
                            true
                        } else {
                            false
                        }
                    });
                then.status(status).json_body(body);
            })
        })
        .collect()
}

/// A tag at HEAD with a matching manifest plans nothing locally, but whether
/// that tag ever became a release is only visible through GitHub. Without a
/// token oakum cannot look, and saying `nothing to release` there is the
/// reassuring-and-wrong answer this tool exists to refuse.
#[test]
fn no_token_cannot_confirm_a_tag_was_released() {
    let root = temp_git_repo("clean");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    add_bare_origin(&root);
    let (ok, stdout, stderr) = oakum_release(&root);
    assert!(!ok, "must refuse rather than reassure: {stdout}{stderr}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(
        stderr.contains("GITHUB_TOKEN and GH_TOKEN are both unset"),
        "the refusal must name what was missing: {stderr}"
    );
    assert!(
        !stdout.contains("nothing to release"),
        "a look that never happened must not print a verdict: {stdout}"
    );
}

/// No tag at all means `resume_tags` asks GitHub nothing, so the answer is
/// fully local and a verdict is honest. Measured: gating the refusal on
/// `planned.is_empty()` alone refuses here and in
/// `remote_but_no_token_with_nothing_to_look_at_answers_locally`.
#[test]
fn no_tag_at_head_answers_locally_without_a_token() {
    let root = temp_git_repo("locally-answerable");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    let (ok, stdout, stderr) = oakum_release(&root);
    assert!(ok, "a local answer must not refuse: {stdout}{stderr}");
    assert!(stdout.contains("nothing to release"), "{stdout}");
}

/// The ordinary state right after a release: the tag is one commit back and
/// HEAD has moved on. Its release can still be missing, so `resume_tags` asks
/// about it and a missing token hides the answer.
#[test]
fn a_tag_behind_head_cannot_be_confirmed_without_a_token() {
    let root = temp_git_repo("tag-behind-head");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    fs::write(root.join("after.txt"), "later").expect("after");
    commit(&root, "later");
    let (ok, stdout, stderr) = oakum_release(&root);
    assert!(!ok, "a look that never happened must not pass: {stdout}");
    assert!(
        stderr.contains("unverified: a tagged version has no local plan"),
        "{stderr}"
    );
    assert!(
        !stdout.contains("nothing to release"),
        "a look that never happened must not print a verdict: {stdout}"
    );
}

/// Measured: before this test, replacing the both-absent arm with
/// `unreachable!()` left all 27 test binaries green.
#[test]
fn neither_remote_nor_token_names_both() {
    let root = temp_git_repo("neither-prerequisite");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    let (ok, _stdout, stderr) = oakum_release(&root);
    assert!(!ok, "{stderr}");
    assert!(stderr.contains("this repository has no remote"), "{stderr}");
    assert!(
        stderr.contains("GITHUB_TOKEN and GH_TOKEN are both unset"),
        "the message must name both, not one: {stderr}"
    );
}

/// The other half of the same refusal: no remote is equally a look that could
/// not happen, and the message must say which one it was.
#[test]
fn no_remote_cannot_confirm_a_tag_was_released() {
    let root = temp_git_repo("unreleased-no-remote");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    let out = bin()
        .arg("release")
        .current_dir(&root)
        .env("GITHUB_TOKEN", "test-token")
        .output()
        .expect("oakum release");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stderr}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(
        stderr.contains("this repository has no remote"),
        "the refusal must name what was missing: {stderr}"
    );
    // The "no remotes to push tags to" error also contains
    // "no remote", so assert a phrase only this refusal produces.
    assert!(
        stderr.contains("missing its release"),
        "this must be the could-not-look refusal, not the push-target one: {stderr}"
    );
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

/// The gate covers the resume path: a tag cut at a skip-ci HEAD whose
/// release is missing is refused before anything is created or pushed.
#[test]
fn skip_ci_head_stops_a_resume_before_it_releases() {
    let root = temp_git_repo("skip-resume");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "version [skip ci]");
    git(&root, &["tag", "v0.1.0"]);
    add_bare_origin(&root);
    let server = MockServer::start();
    let lookup = mock_lookup_empty(&server, "v0.1.0");
    let create = mock_create(&server, "v0.1.0", 201);
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stderr_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("skip-ci"), "{stderr}");
    lookup.assert_calls(1);
    create.assert_calls(0);
}

/// A resumed tag is pushed at its own commit, so the gate reads that commit,
/// not HEAD: a tag cut at a skip-ci commit stays refused after HEAD moves on.
#[test]
fn skip_ci_tagged_commit_stops_a_resume_even_after_head_moves() {
    let root = temp_git_repo("skip-resume-behind");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "version [skip ci]");
    git(&root, &["tag", "v0.1.0"]);
    fs::write(root.join("README.md"), "docs\n").expect("readme");
    commit(&root, "docs: clean follow-up");
    add_bare_origin(&root);
    let server = MockServer::start();
    let lookup = mock_lookup_empty(&server, "v0.1.0");
    let create = mock_create(&server, "v0.1.0", 201);
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stderr_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("skip-ci"), "{stderr}");
    assert!(stderr.contains("v0.1.0"), "{stderr}");
    lookup.assert_calls(1);
    create.assert_calls(0);
}

/// The steady state: tag cut at a skip-ci commit, release already on GitHub.
/// Nothing will be pushed, so nothing is gated — every later `oakum release`
/// stays a no-op instead of a permanent refusal.
#[test]
fn skip_ci_tagged_commit_with_existing_release_is_nothing_to_release() {
    let root = temp_git_repo("skip-released");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "chore(release): v0.1.0 [skip ci]");
    git(&root, &["tag", "v0.1.0"]);
    add_bare_origin(&root);
    git(&root, &["push", "origin", "refs/tags/v0.1.0"]);
    let server = MockServer::start();
    let lookup = server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/releases/tags/v0.1.0");
        then.status(200).json_body(json!({
            "html_url": "https://github.com/oakoss/oakum/releases/tag/v0.1.0",
            "tag_name": "v0.1.0"
        }));
    });
    let create = mock_create(&server, "v0.1.0", 201);
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
    lookup.assert_calls(1);
    create.assert_calls(0);
}

/// With nothing to tag there is no release for GitHub to suppress, so a
/// skip-ci HEAD is not gated and the answer stays local.
#[test]
fn skip_ci_head_with_nothing_to_release_answers_locally() {
    let root = temp_git_repo("skip-noop");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "docs: fix [skip ci]");
    add_bare_origin(&root);
    let (ok, stdout, stderr) = oakum_release(&root);
    assert!(ok, "{stdout}{stderr}");
    assert!(stdout.contains("nothing to release"), "{stdout}");
}

/// A remote that is not a github.com URL leaves the release lookup unmade, so
/// the outcome class is `unverified`, not a plain error.
#[test]
fn a_non_github_remote_reports_unverified() {
    let root = temp_git_repo("gitlab-remote");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "https://gitlab.invalid/oakoss/demo.git",
        ],
    );
    let out = bin()
        .arg("release")
        .current_dir(&root)
        .env("GITHUB_TOKEN", "token")
        .env_remove("GITHUB_REPOSITORY")
        .output()
        .expect("release");
    assert!(!out.status.success(), "a slugless remote must not pass");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(
        stderr.contains("not a github.com owner/repo URL"),
        "{stderr}"
    );
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

/// `pushurl` can be set more than once, and measured, `git push` contacts every
/// one of them while `git remote get-url --push` reports only the first. A
/// later URL over ssh has to count.
#[cfg(unix)]
#[test]
fn a_second_push_url_over_ssh_still_gets_the_note() {
    let root = pending_demo("pushurl-many");
    add_bare_origin(&root);
    let bare = root.parent().expect("parent").join("pushurl-many.git");
    // The local one first, so the fetch URL and the leading push URL are both
    // ordinary paths and only the second reaches ssh.
    git(
        &root,
        &[
            "config",
            "--add",
            "remote.origin.pushurl",
            bare.to_str().expect("utf-8"),
        ],
    );
    git(
        &root,
        &[
            "config",
            "--add",
            "remote.origin.pushurl",
            "git+ssh://git@example.invalid/demo.git",
        ],
    );
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/releases/tags/v0.1.1");
        then.status(404).body("Not Found");
    });
    server.mock(|when, then| {
        when.method(POST).path("/repos/oakoss/oakum/releases");
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
        .env_remove("GIT_SSH_COMMAND")
        .env("GIT_SSH", "/usr/local/bin/my-ssh")
        .output()
        .expect("release");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cannot refuse ssh prompts"),
        "a later push URL over ssh must still be warned about: {stderr}"
    );
}

/// A remote can fetch over one transport and push over another, so the note is
/// tracked per direction. Both URL probes fail here, so both directions are
/// unestablished and both are entitled to say so.
#[cfg(unix)]
#[test]
fn the_note_is_said_for_the_fetch_and_again_for_the_push() {
    let root = pending_demo("note-per-direction");
    add_bare_origin(&root);
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/releases/tags/v0.1.1");
        then.status(404).body("Not Found");
    });
    server.mock(|when, then| {
        when.method(POST).path("/repos/oakoss/oakum/releases");
        then.status(201).json_body(json!({
            "html_url": "https://github.com/oakoss/oakum/releases/tag/v0.1.1"
        }));
    });
    let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("oakum-note-direction-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    let shim_dir = scratch.join("shim");
    fs::create_dir_all(&shim_dir).expect("shim dir");
    let real = String::from_utf8(
        Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .expect("which git")
            .stdout,
    )
    .expect("utf-8");
    let shim = shim_dir.join("git");
    // Only oakum's own URL probes fail; git resolves the remote from config as
    // usual, so the fetch and the push both still reach the local bare remote.
    install_executable(
        &shim,
        format!(
            "#!/bin/sh\ncase \"$*\" in\n\
             'remote get-url'*) echo 'fatal: unable to read config file' >&2; exit 128 ;;\n\
             esac\nexec {real} \"$@\"\n",
            real = real.trim()
        ),
    );

    let out = bin()
        .arg("release")
        .current_dir(&root)
        .env(
            "PATH",
            format!(
                "{}:{}",
                shim_dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env_remove("GIT_SSH_COMMAND")
        .env("GIT_SSH", "/usr/local/bin/my-ssh")
        .output()
        .expect("release");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "stdout={} stderr={stderr}",
        String::from_utf8_lossy(&out.stdout)
    );
    let notes = stderr
        .lines()
        .filter(|line| line.contains("cannot refuse ssh prompts"))
        .count();
    assert_eq!(
        notes, 2,
        "one note for the fetch and one for the push, got {notes}: {stderr}"
    );
}

/// A push can go somewhere else entirely. Measured: a remote with an `https`
/// `url` and a `git+ssh` `pushurl` fetches over https and pushes over ssh, so
/// asking about the fetch URL leaves the push — the operation that actually
/// needs the note — unwarned.
#[cfg(unix)]
#[test]
fn a_push_url_that_uses_ssh_gets_the_note_when_the_fetch_url_does_not() {
    let root = pending_demo("pushurl-ssh");
    add_bare_origin(&root);
    // The fetch URL stays the local bare remote so the release still completes;
    // the push URL is what the note has to be decided from.
    git(
        &root,
        &[
            "config",
            "remote.origin.pushurl",
            "git+ssh://git@example.invalid/demo.git",
        ],
    );
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/releases/tags/v0.1.1");
        then.status(404).body("Not Found");
    });
    server.mock(|when, then| {
        when.method(POST).path("/repos/oakoss/oakum/releases");
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
        .env_remove("GIT_SSH_COMMAND")
        .env("GIT_SSH", "/usr/local/bin/my-ssh")
        .output()
        .expect("release");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cannot refuse ssh prompts"),
        "a push over ssh must be warned even when the fetch URL is not: {stderr}"
    );
}

/// The ssh transport resolves from the process environment and the repository
/// config, neither of which changes mid-run, so it is read once per command
/// rather than once per remote child. A release makes 1 + 2N remote children
/// for N tags — here three — and each would otherwise re-run
/// `git config --get-regexp` to ask the same question.
#[cfg(unix)]
#[test]
fn the_ssh_transport_is_read_once_however_many_remote_children_run() {
    let root = pending_demo("probe-once");
    add_bare_origin(&root);
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/releases/tags/v0.1.1");
        then.status(404).body("Not Found");
    });
    server.mock(|when, then| {
        when.method(POST).path("/repos/oakoss/oakum/releases");
        then.status(201).json_body(json!({
            "html_url": "https://github.com/oakoss/oakum/releases/tag/v0.1.1"
        }));
    });

    // Outside the repository: a shim or a log inside it is an untracked file,
    // and `oakum release` refuses a dirty worktree.
    let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("oakum-probe-once-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    let log = scratch.join("git-argv.log");
    let shim_dir = scratch.join("shim");
    fs::create_dir_all(&shim_dir).expect("shim dir");
    let real = String::from_utf8(
        Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .expect("which git")
            .stdout,
    )
    .expect("utf-8");
    let shim = shim_dir.join("git");
    install_executable(
        &shim,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {log}\nexec {real} \"$@\"\n",
            log = log.to_str().expect("utf-8 log"),
            real = real.trim()
        ),
    );

    let out = bin()
        .arg("release")
        .current_dir(&root)
        .env(
            "PATH",
            format!(
                "{}:{}",
                shim_dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        // An opaque transport, so the ssh note is pending and the remote's URL
        // has to be consulted to decide whether it applies — the second thing
        // that would otherwise be re-read per remote child. The remote here is
        // a local path, so no ssh is invoked either way.
        .env_remove("GIT_SSH_COMMAND")
        .env("GIT_SSH", "/usr/local/bin/my-ssh")
        .output()
        .expect("release");
    assert!(
        out.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let argv = fs::read_to_string(&log).unwrap_or_default();
    let probes = argv
        .lines()
        .filter(|line| line.contains("sshcommand"))
        .count();
    let remote_children = argv
        .lines()
        .filter(|line| line.starts_with("ls-remote") || line.starts_with("push"))
        .count();
    assert!(
        remote_children >= 3,
        "the release should make several remote children, got {remote_children}:\n{argv}"
    );
    assert_eq!(
        probes, 1,
        "the transport should be read once, not once per remote child \
         ({remote_children} of those):\n{argv}"
    );
    // Two kinds of URL, each read once: `ls-remote` is judged by the fetch URL
    // and `push` by the push URL, which `remote.<name>.pushurl` can point
    // somewhere else entirely.
    let fetch_urls = argv
        .lines()
        .filter(|line| line.starts_with("remote get-url -- "))
        .count();
    let push_urls = argv
        .lines()
        .filter(|line| line.starts_with("remote get-url --push"))
        .count();
    assert_eq!(
        (fetch_urls, push_urls),
        (1, 1),
        "each remote URL should be read once, not once per remote child \
         ({remote_children} of those):\n{argv}"
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
    install_executable(
        &stub,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$0 $*\" >> '{}'\nexit 1\n",
            log.display()
        ),
    );
    install_executable(
        &editor,
        format!(
            "#!/bin/sh\necho ran >> '{}'\nexit 1\n",
            editor_ran.display()
        ),
    );
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

/// A tag one commit back whose release is missing is the failure this tool
/// exists to catch, and where HEAD sits has nothing to do with it. The release
/// is created against the commit the tag already names.
#[test]
fn current_tag_behind_head_is_resumed() {
    let root = temp_git_repo("local-other");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    cargo_package(&root, "demo", "0.1.1");
    commit(&root, "version");
    git(&root, &["tag", "v0.1.1"]);
    fs::write(root.join("later.txt"), "moved").expect("later");
    commit(&root, "later");
    let tagged = commit_at(&root, "v0.1.1");
    add_bare_origin(&root);
    let server = MockServer::start();
    let lookup = mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let out = release_cmd(&root, &server);
    assert!(
        out.status.success(),
        "{}{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    assert!(
        !stdout_of(&out).contains("nothing to release"),
        "{}",
        stdout_of(&out)
    );
    // `preflight` re-reads what `resume_tags` already read, so the tag is
    // asked about twice.
    assert_eq!(lookup.calls(), 2);
    create.assert();
    assert_eq!(
        commit_at(&root, "v0.1.1"),
        tagged,
        "resuming must release the tag where it stands, not move it"
    );
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
    create_alpha.assert();
    create_beta.assert();
    assert_eq!(
        commit_at(&root, "alpha/v0.1.0"),
        commit_at(&root, "HEAD~1"),
        "resuming must release the tag where it stands, not move it"
    );
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
    install_executable(&hook, "#!/bin/sh\nexit 1\n");
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
fn no_downstream_workflow_is_a_completed_look() {
    let root = pending_demo("no-dist");
    add_bare_origin(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let runs = server.mock(|when, then| {
        when.method(GET).path("/repos/oakoss/oakum/actions/runs");
        then.status(200)
            .json_body(json!({ "total_count": 0, "workflow_runs": [] }));
    });
    let out = release_cmd(&root, &server);
    assert!(
        out.status.success(),
        "{}{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    let stderr = stderr_of(&out);
    assert!(stderr.contains("no downstream workflow"), "{stderr}");
    create.assert();
    runs.assert_calls(0);
}

#[test]
fn tag_workflow_run_confirms_the_handoff() {
    let root = pending_demo("dist-run");
    write_workflow(&root, "dist.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    commit(&root, "workflow");
    add_bare_origin(&root);
    let sha = head_sha(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let runs = mock_workflow_hits(
        &server,
        &sha,
        &[
            RunHit::Empty,
            RunHit::Run {
                id: 9,
                path: ".github/workflows/dist.yml".into(),
            },
        ],
    );
    let out = release_cmd(&root, &server);
    assert!(
        out.status.success(),
        "{}{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    let stdout = stdout_of(&out);
    assert!(
        stdout.contains("https://github.com/oakoss/oakum/actions/runs/9"),
        "{stdout}"
    );
    create.assert();
    assert_eq!(runs.iter().map(httpmock::Mock::calls).sum::<usize>(), 2);
}

/// A resumed tag sits behind HEAD, and the workflow its push starts belongs to
/// the commit the tag names. Keying the handoff at HEAD instead looks for a run
/// at a commit that was never tagged, and reports `unverified` forever.
#[test]
fn resumed_tag_confirms_the_handoff_at_its_own_commit() {
    let root = temp_git_repo("resume-handoff");
    cargo_package(&root, "demo", "0.1.0");
    write_workflow(&root, "dist.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    fs::write(root.join("later.txt"), "moved").expect("later");
    commit(&root, "later");
    add_bare_origin(&root);
    let tagged = commit_at(&root, "v0.1.0");
    assert_ne!(tagged, head_sha(&root), "the tag must sit behind HEAD");
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.0");
    let create = mock_create(&server, "v0.1.0", 201);
    let runs = mock_workflow_hits(
        &server,
        &tagged,
        &[
            RunHit::Empty,
            RunHit::Run {
                id: 9,
                path: ".github/workflows/dist.yml".into(),
            },
        ],
    );
    let out = release_cmd(&root, &server);
    assert!(
        out.status.success(),
        "{}{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    assert!(
        stdout_of(&out).contains("https://github.com/oakoss/oakum/actions/runs/9"),
        "{}",
        stdout_of(&out)
    );
    create.assert();
    assert_eq!(runs.iter().map(httpmock::Mock::calls).sum::<usize>(), 2);
}

/// The commonest resume: the tag was pushed, HEAD moved on, and only the
/// GitHub release is missing. `preflight` compares the advertised tag against
/// the tag's own commit — comparing it against HEAD refuses this outright.
#[test]
fn resumed_tag_already_pushed_is_released_where_it_stands() {
    let root = temp_git_repo("resume-pushed");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    add_bare_origin(&root);
    git(&root, &["push", "origin", "refs/tags/v0.1.0"]);
    fs::write(root.join("later.txt"), "moved").expect("later");
    commit(&root, "later");
    assert_ne!(
        commit_at(&root, "v0.1.0"),
        head_sha(&root),
        "the tag must sit behind HEAD"
    );
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.0");
    let create = mock_create(&server, "v0.1.0", 201);
    let out = release_cmd(&root, &server);
    assert!(
        out.status.success(),
        "{}{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    create.assert();
}

/// A resumed tag and a pending tag sit at different commits, so the handoff
/// keeps a snapshot per commit. Sharing one bucket lets the older commit's run
/// read as a leftover; run id 9 is reused at both commits to catch that.
#[test]
fn mixed_commit_plan_snapshots_each_commit_separately() {
    let root = temp_git_repo("mixed-commit-handoff");
    write_workspace(&root, &[("alpha", "0.1.0"), ("beta", "0.1.0")]);
    write_workflow(&root, "dist.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    commit(&root, "init");
    git(&root, &["tag", "alpha/v0.1.0"]);
    git(&root, &["tag", "beta/v0.1.0"]);
    write_workspace(&root, &[("alpha", "0.1.0"), ("beta", "0.1.1")]);
    commit(&root, "version");
    add_bare_origin(&root);
    let older = commit_at(&root, "alpha/v0.1.0");
    let head = head_sha(&root);
    assert_ne!(older, head, "alpha must sit behind HEAD");
    let server = MockServer::start();
    mock_lookup_empty(&server, "alpha%2Fv0.1.0");
    mock_lookup_empty(&server, "beta%2Fv0.1.1");
    let create_alpha = mock_create(&server, "alpha/v0.1.0", 201);
    let create_beta = mock_create(&server, "beta/v0.1.1", 201);
    let head_runs = mock_workflow_hits(
        &server,
        &head,
        &[
            RunHit::Empty,
            RunHit::Run {
                id: 9,
                path: ".github/workflows/dist.yml".into(),
            },
        ],
    );
    let older_runs = mock_workflow_hits(
        &server,
        &older,
        &[
            RunHit::Empty,
            RunHit::Run {
                id: 9,
                path: ".github/workflows/dist.yml".into(),
            },
        ],
    );
    let out = release_cmd(&root, &server);
    assert!(
        out.status.success(),
        "{}{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    create_alpha.assert();
    create_beta.assert();
    assert_eq!(
        head_runs.iter().map(httpmock::Mock::calls).sum::<usize>(),
        2
    );
    assert_eq!(
        older_runs.iter().map(httpmock::Mock::calls).sum::<usize>(),
        2
    );
}

/// Snapshots for every commit precede every write, so a look that fails on the
/// second commit cannot leave the first tag pushed and released with no report.
/// Taking the snapshot inside the release loop instead releases `beta` first.
#[test]
fn second_commit_snapshot_failure_creates_no_release() {
    let root = temp_git_repo("snapshot-second-commit");
    write_workspace(&root, &[("alpha", "0.1.0"), ("beta", "0.1.0")]);
    write_workflow(&root, "dist.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    commit(&root, "init");
    git(&root, &["tag", "alpha/v0.1.0"]);
    git(&root, &["tag", "beta/v0.1.0"]);
    write_workspace(&root, &[("alpha", "0.1.0"), ("beta", "0.1.1")]);
    commit(&root, "version");
    add_bare_origin(&root);
    let older = commit_at(&root, "alpha/v0.1.0");
    let head = head_sha(&root);
    assert_ne!(older, head, "alpha must sit behind HEAD");
    let server = MockServer::start();
    mock_lookup_empty(&server, "alpha%2Fv0.1.0");
    mock_lookup_empty(&server, "beta%2Fv0.1.1");
    let create_alpha = mock_create(&server, "alpha/v0.1.0", 201);
    let create_beta = mock_create(&server, "beta/v0.1.1", 201);
    mock_workflow_hits(&server, &head, &[RunHit::Empty]);
    mock_workflow_hits(&server, &older, &[RunHit::Fail(500)]);
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    assert!(
        stderr_of(&out).contains("unverified"),
        "{}",
        stderr_of(&out)
    );
    create_alpha.assert_calls(0);
    create_beta.assert_calls(0);
    assert!(
        !local_tags(&root).contains("beta/v0.1.1"),
        "no tag may be cut before every snapshot is taken: {}",
        local_tags(&root)
    );
}

/// A tag left on a commit that history rewrote away: `v0.1.1` was cut, the
/// commit was reset out, and the manifest bumped to 0.1.1 again on the
/// surviving line. The package reads as pending and is planned at HEAD, so
/// only `preflight`'s local-tag check stands between oakum and a release
/// against abandoned history.
#[test]
fn local_tag_on_rewound_history_creates_no_release() {
    let root = temp_git_repo("rewound-tag");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    cargo_package(&root, "demo", "0.1.1");
    commit(&root, "version");
    git(&root, &["tag", "v0.1.1"]);
    let abandoned = commit_at(&root, "v0.1.1");
    git(&root, &["reset", "--hard", "HEAD~1"]);
    cargo_package(&root, "demo", "0.1.1");
    commit(&root, "again");
    add_bare_origin(&root);
    assert_ne!(abandoned, head_sha(&root), "the tag must be off the line");
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    assert!(
        stderr_of(&out).contains("local tag `v0.1.1` points at"),
        "{}",
        stderr_of(&out)
    );
    create.assert_calls(0);
}

/// The plan interleaves commits: `[beta@head, alpha@older, gamma@head]`, so the
/// tag sharing a commit with `beta` is not the next one. `absorb` must scan
/// every later tag — a next-only check skips it, and the leftover run 999 that
/// appears at HEAD then confirms `gamma`'s handoff instead of its own run.
#[test]
fn absorb_scans_every_later_tag_for_a_shared_commit() {
    let root = temp_git_repo("interleaved-absorb");
    write_workspace(
        &root,
        &[("alpha", "0.1.0"), ("beta", "0.1.0"), ("gamma", "0.1.0")],
    );
    write_workflow(&root, "dist.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    commit(&root, "init");
    git(&root, &["tag", "alpha/v0.1.0"]);
    write_workspace(
        &root,
        &[("alpha", "0.1.0"), ("beta", "0.1.1"), ("gamma", "0.1.0")],
    );
    commit(&root, "version");
    git(&root, &["tag", "gamma/v0.1.0"]);
    add_bare_origin(&root);
    let older = commit_at(&root, "alpha/v0.1.0");
    let head = head_sha(&root);
    assert_ne!(older, head, "alpha must sit behind HEAD");
    assert_eq!(
        commit_at(&root, "gamma/v0.1.0"),
        head,
        "gamma must sit at HEAD"
    );
    let server = MockServer::start();
    mock_lookup_empty(&server, "alpha%2Fv0.1.0");
    mock_lookup_empty(&server, "gamma%2Fv0.1.0");
    mock_lookup_empty(&server, "beta%2Fv0.1.1");
    let create_alpha = mock_create(&server, "alpha/v0.1.0", 201);
    let create_beta = mock_create(&server, "beta/v0.1.1", 201);
    let create_gamma = mock_create(&server, "gamma/v0.1.0", 201);
    let path = ".github/workflows/dist.yml";
    mock_workflow_hits(
        &server,
        &head,
        &[
            RunHit::Empty,
            RunHit::Runs {
                ids: vec![100],
                path: path.into(),
            },
            RunHit::Runs {
                ids: vec![100, 999],
                path: path.into(),
            },
            RunHit::Runs {
                ids: vec![100, 999, 101],
                path: path.into(),
            },
        ],
    );
    mock_workflow_hits(
        &server,
        &older,
        &[
            RunHit::Empty,
            RunHit::Runs {
                ids: vec![200],
                path: path.into(),
            },
        ],
    );
    let out = release_cmd(&root, &server);
    assert!(
        out.status.success(),
        "{}{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    let stdout = stdout_of(&out);
    create_alpha.assert();
    create_beta.assert();
    create_gamma.assert();
    assert!(stdout.contains("/actions/runs/101"), "{stdout}");
    assert!(
        !stdout.contains("/actions/runs/999"),
        "a run absorbed before this tag was pushed must not confirm it: {stdout}"
    );
}

/// A workflow added after the tag was cut cannot have run at that tag. The
/// listener set is read from the worktree, so oakum asks about a file that was
/// not there — and the message must say so rather than implying a run is
/// missing.
#[test]
fn a_listener_added_after_the_tag_names_the_worktree_in_the_verdict() {
    let root = temp_git_repo("listener-after-tag");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_workflow(&root, "dist.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    commit(&root, "workflow");
    add_bare_origin(&root);
    let tagged = commit_at(&root, "v0.1.0");
    assert_ne!(tagged, head_sha(&root), "the tag must predate the workflow");
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.0");
    let create = mock_create(&server, "v0.1.0", 201);
    mock_workflow_hits(&server, &tagged, &[RunHit::Empty, RunHit::Empty]);
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(
        stderr.contains("read from the worktree"),
        "the verdict must name what it actually looked at: {stderr}"
    );
    create.assert();
}

/// The other cell of the remote-or-token gate: a remote is present and only the
/// token is missing, with no tagged version to ask about.
#[test]
fn remote_but_no_token_with_nothing_to_look_at_answers_locally() {
    let root = temp_git_repo("remote-no-token");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    add_bare_origin(&root);
    let (ok, stdout, stderr) = oakum_release(&root);
    assert!(ok, "a local answer must not refuse: {stdout}{stderr}");
    assert!(stdout.contains("nothing to release"), "{stdout}");
}

#[test]
fn missing_tag_workflow_run_is_unverified() {
    let root = pending_demo("dist-miss");
    write_workflow(&root, "dist.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    commit(&root, "workflow");
    add_bare_origin(&root);
    let sha = head_sha(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let runs = mock_workflow_runs(&server, &sha, ".github/workflows/dist.yml", true);
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("no workflow run"), "{stderr}");
    assert!(stderr.contains("v0.1.1 (released)"), "{stderr}");
    assert!(!stderr.contains("remaining:\n  v0.1.1"), "{stderr}");
    assert!(
        local_tags(&root).contains("v0.1.1"),
        "{}",
        local_tags(&root)
    );
    create.assert();
    runs.assert_calls(2);
}

#[test]
fn dispatch_only_workflow_is_not_triggered() {
    let root = pending_demo("dispatch");
    write_workflow(
        &root,
        "dist.yml",
        "on:\n  workflow_dispatch:\n    inputs:\n      tag:\n        type: string\n",
    );
    commit(&root, "workflow");
    add_bare_origin(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let runs = server.mock(|when, then| {
        when.method(GET).path("/repos/oakoss/oakum/actions/runs");
        then.status(200)
            .json_body(json!({ "total_count": 0, "workflow_runs": [] }));
    });
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("workflow_dispatch"), "{stderr}");
    assert!(!stderr.contains("unverified"), "{stderr}");
    assert_eq!(local_tags(&root).trim(), "v0.1.0");
    create.assert_calls(0);
    runs.assert_calls(0);
}

#[test]
fn other_jobs_run_does_not_confirm_the_handoff() {
    let root = pending_demo("own-job");
    write_workflow(&root, "dist.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    commit(&root, "workflow");
    add_bare_origin(&root);
    let sha = head_sha(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let runs = mock_workflow_runs(&server, &sha, ".github/workflows/release.yml", false);
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(
        local_tags(&root).contains("v0.1.1"),
        "{}",
        local_tags(&root)
    );
    create.assert();
    runs.assert_calls(2);
}

#[test]
fn branch_push_workflow_is_a_completed_look() {
    let root = pending_demo("branch-push");
    write_workflow(&root, "release.yml", "on: push\n");
    commit(&root, "workflow");
    add_bare_origin(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let runs = server.mock(|when, then| {
        when.method(GET).path("/repos/oakoss/oakum/actions/runs");
        then.status(200)
            .json_body(json!({ "total_count": 0, "workflow_runs": [] }));
    });
    let out = release_cmd(&root, &server);
    assert!(
        out.status.success(),
        "{}{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    let stderr = stderr_of(&out);
    assert!(stderr.contains("no downstream workflow"), "{stderr}");
    create.assert();
    runs.assert_calls(0);
}

#[test]
fn later_tag_does_not_reuse_an_earlier_run() {
    let root = temp_git_repo("reuse-run");
    write_workspace(&root, &[("alpha", "0.1.0"), ("beta", "0.1.0")]);
    commit(&root, "init");
    git(&root, &["tag", "alpha/v0.1.0"]);
    git(&root, &["tag", "beta/v0.1.0"]);
    write_workspace(&root, &[("alpha", "0.1.1"), ("beta", "0.1.1")]);
    write_workflow(&root, "dist.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    commit(&root, "version");
    add_bare_origin(&root);
    let sha = head_sha(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "alpha%2Fv0.1.1");
    mock_lookup_empty(&server, "beta%2Fv0.1.1");
    let create_alpha = mock_create(&server, "alpha/v0.1.1", 201);
    let create_beta = mock_create(&server, "beta/v0.1.1", 201);
    let runs = mock_workflow_hits(
        &server,
        &sha,
        &[
            RunHit::Empty,
            RunHit::Run {
                id: 9,
                path: ".github/workflows/dist.yml".into(),
            },
            RunHit::Run {
                id: 9,
                path: ".github/workflows/dist.yml".into(),
            },
            RunHit::Run {
                id: 9,
                path: ".github/workflows/dist.yml".into(),
            },
        ],
    );
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("beta/v0.1.1 (released)"), "{stderr}");
    assert!(!stderr.contains("remaining:\n  beta/v0.1.1"), "{stderr}");
    create_alpha.assert();
    create_beta.assert();
    assert_eq!(runs.iter().map(httpmock::Mock::calls).sum::<usize>(), 4);
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
}

#[test]
fn later_tag_confirms_a_new_run() {
    let root = temp_git_repo("new-run");
    write_workspace(&root, &[("alpha", "0.1.0"), ("beta", "0.1.0")]);
    commit(&root, "init");
    git(&root, &["tag", "alpha/v0.1.0"]);
    git(&root, &["tag", "beta/v0.1.0"]);
    write_workspace(&root, &[("alpha", "0.1.1"), ("beta", "0.1.1")]);
    write_workflow(&root, "dist.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    commit(&root, "version");
    add_bare_origin(&root);
    let sha = head_sha(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "alpha%2Fv0.1.1");
    mock_lookup_empty(&server, "beta%2Fv0.1.1");
    let create_alpha = mock_create(&server, "alpha/v0.1.1", 201);
    let create_beta = mock_create(&server, "beta/v0.1.1", 201);
    let runs = mock_workflow_hits(
        &server,
        &sha,
        &[
            RunHit::Empty,
            RunHit::Run {
                id: 9,
                path: ".github/workflows/dist.yml".into(),
            },
            RunHit::Run {
                id: 9,
                path: ".github/workflows/dist.yml".into(),
            },
            RunHit::Run {
                id: 10,
                path: ".github/workflows/dist.yml".into(),
            },
        ],
    );
    let out = release_cmd(&root, &server);
    assert!(
        out.status.success(),
        "{}{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    let stdout = stdout_of(&out);
    assert!(
        stdout.contains("https://github.com/oakoss/oakum/actions/runs/9"),
        "{stdout}"
    );
    assert!(
        stdout.contains("https://github.com/oakoss/oakum/actions/runs/10"),
        "{stdout}"
    );
    create_alpha.assert();
    create_beta.assert();
    assert_eq!(runs.iter().map(httpmock::Mock::calls).sum::<usize>(), 4);
}

#[test]
fn sibling_listener_run_does_not_confirm_a_later_tag() {
    let root = temp_git_repo("sibling-run");
    write_workspace(&root, &[("alpha", "0.1.0"), ("beta", "0.1.0")]);
    commit(&root, "init");
    git(&root, &["tag", "alpha/v0.1.0"]);
    git(&root, &["tag", "beta/v0.1.0"]);
    write_workspace(&root, &[("alpha", "0.1.1"), ("beta", "0.1.1")]);
    write_workflow(&root, "dist.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    write_workflow(&root, "other.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    commit(&root, "version");
    add_bare_origin(&root);
    let sha = head_sha(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "alpha%2Fv0.1.1");
    mock_lookup_empty(&server, "beta%2Fv0.1.1");
    let create_alpha = mock_create(&server, "alpha/v0.1.1", 201);
    let create_beta = mock_create(&server, "beta/v0.1.1", 201);
    let page = RunHit::Page(vec![
        (9, ".github/workflows/dist.yml".into(), "push"),
        (10, ".github/workflows/other.yml".into(), "push"),
    ]);
    let runs = mock_workflow_hits(
        &server,
        &sha,
        &[RunHit::Empty, page.clone(), page.clone(), page],
    );
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stdout = stdout_of(&out);
    let stderr = stderr_of(&out);
    assert!(
        stdout.contains("https://github.com/oakoss/oakum/actions/runs/9"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("https://github.com/oakoss/oakum/actions/runs/10"),
        "{stdout}"
    );
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("beta/v0.1.1 (released)"), "{stderr}");
    create_alpha.assert();
    create_beta.assert();
    assert_eq!(runs.iter().map(httpmock::Mock::calls).sum::<usize>(), 4);
}

#[test]
fn late_sibling_run_does_not_confirm_a_later_tag() {
    let root = temp_git_repo("late-sibling");
    write_workspace(&root, &[("alpha", "0.1.0"), ("beta", "0.1.0")]);
    commit(&root, "init");
    git(&root, &["tag", "alpha/v0.1.0"]);
    git(&root, &["tag", "beta/v0.1.0"]);
    write_workspace(&root, &[("alpha", "0.1.1"), ("beta", "0.1.1")]);
    write_workflow(&root, "dist.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    write_workflow(&root, "other.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    commit(&root, "version");
    add_bare_origin(&root);
    let sha = head_sha(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "alpha%2Fv0.1.1");
    mock_lookup_empty(&server, "beta%2Fv0.1.1");
    let create_alpha = mock_create(&server, "alpha/v0.1.1", 201);
    let create_beta = mock_create(&server, "beta/v0.1.1", 201);
    let page = RunHit::Page(vec![
        (9, ".github/workflows/dist.yml".into(), "push"),
        (10, ".github/workflows/other.yml".into(), "push"),
    ]);
    let runs = mock_workflow_hits(
        &server,
        &sha,
        &[
            RunHit::Empty,
            RunHit::Run {
                id: 9,
                path: ".github/workflows/dist.yml".into(),
            },
            page.clone(),
            page,
        ],
    );
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stdout = stdout_of(&out);
    let stderr = stderr_of(&out);
    assert!(
        stdout.contains("https://github.com/oakoss/oakum/actions/runs/9"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("https://github.com/oakoss/oakum/actions/runs/10"),
        "{stdout}"
    );
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("beta/v0.1.1 (released)"), "{stderr}");
    create_alpha.assert();
    create_beta.assert();
    assert_eq!(runs.iter().map(httpmock::Mock::calls).sum::<usize>(), 4);
}

#[test]
fn absorb_500_after_first_confirm_leaves_later_tag() {
    let root = temp_git_repo("absorb-500");
    write_workspace(&root, &[("alpha", "0.1.0"), ("beta", "0.1.0")]);
    commit(&root, "init");
    git(&root, &["tag", "alpha/v0.1.0"]);
    git(&root, &["tag", "beta/v0.1.0"]);
    write_workspace(&root, &[("alpha", "0.1.1"), ("beta", "0.1.1")]);
    write_workflow(&root, "dist.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    commit(&root, "version");
    add_bare_origin(&root);
    let sha = head_sha(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "alpha%2Fv0.1.1");
    mock_lookup_empty(&server, "beta%2Fv0.1.1");
    let create_alpha = mock_create(&server, "alpha/v0.1.1", 201);
    let create_beta = mock_create(&server, "beta/v0.1.1", 201);
    let runs = mock_workflow_hits(
        &server,
        &sha,
        &[
            RunHit::Empty,
            RunHit::Run {
                id: 9,
                path: ".github/workflows/dist.yml".into(),
            },
            RunHit::Fail(500),
        ],
    );
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stdout = stdout_of(&out);
    let stderr = stderr_of(&out);
    assert!(
        stdout.contains("https://github.com/oakoss/oakum/actions/runs/9"),
        "{stdout}"
    );
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("alpha/v0.1.1 (released)"), "{stderr}");
    assert!(stderr.contains("remaining:\n  beta/v0.1.1"), "{stderr}");
    create_alpha.assert();
    create_beta.assert_calls(0);
    assert_eq!(runs.iter().map(httpmock::Mock::calls).sum::<usize>(), 3);
}

#[test]
fn absorb_run_without_path_does_not_confirm_a_later_tag() {
    let root = temp_git_repo("absorb-no-path");
    write_workspace(&root, &[("alpha", "0.1.0"), ("beta", "0.1.0")]);
    commit(&root, "init");
    git(&root, &["tag", "alpha/v0.1.0"]);
    git(&root, &["tag", "beta/v0.1.0"]);
    write_workspace(&root, &[("alpha", "0.1.1"), ("beta", "0.1.1")]);
    write_workflow(&root, "dist.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    write_workflow(&root, "other.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    commit(&root, "version");
    add_bare_origin(&root);
    let sha = head_sha(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "alpha%2Fv0.1.1");
    mock_lookup_empty(&server, "beta%2Fv0.1.1");
    let create_alpha = mock_create(&server, "alpha/v0.1.1", 201);
    let create_beta = mock_create(&server, "beta/v0.1.1", 201);
    let allowed = (9, Some(".github/workflows/dist.yml".into()), Some("push"));
    let later = RunHit::Page(vec![
        (9, ".github/workflows/dist.yml".into(), "push"),
        (10, ".github/workflows/other.yml".into(), "push"),
    ]);
    let runs = mock_workflow_hits(
        &server,
        &sha,
        &[
            RunHit::Empty,
            RunHit::Run {
                id: 9,
                path: ".github/workflows/dist.yml".into(),
            },
            RunHit::Incomplete(vec![allowed, (10, None, Some("push"))]),
            later,
        ],
    );
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stdout = stdout_of(&out);
    let stderr = stderr_of(&out);
    assert!(
        stdout.contains("https://github.com/oakoss/oakum/actions/runs/9"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("https://github.com/oakoss/oakum/actions/runs/10"),
        "{stdout}"
    );
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("beta/v0.1.1 (released)"), "{stderr}");
    create_alpha.assert();
    create_beta.assert();
    assert_eq!(runs.iter().map(httpmock::Mock::calls).sum::<usize>(), 4);
}

#[test]
fn stale_run_on_head_does_not_confirm() {
    let root = pending_demo("stale-run");
    write_workflow(&root, "dist.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    commit(&root, "workflow");
    add_bare_origin(&root);
    let sha = head_sha(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let runs = mock_workflow_runs(&server, &sha, ".github/workflows/dist.yml", false);
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("v0.1.1 (released)"), "{stderr}");
    create.assert();
    runs.assert_calls(2);
}

#[test]
fn missing_run_path_is_unverified() {
    let root = pending_demo("no-path");
    write_workflow(&root, "dist.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    commit(&root, "workflow");
    add_bare_origin(&root);
    let sha = head_sha(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let runs = mock_workflow_hits(
        &server,
        &sha,
        &[
            RunHit::RunWithoutPath { id: 9 },
            RunHit::RunWithoutPath { id: 9 },
        ],
    );
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(
        local_tags(&root).contains("v0.1.1"),
        "{}",
        local_tags(&root)
    );
    create.assert();
    assert_eq!(runs.iter().map(httpmock::Mock::calls).sum::<usize>(), 2);
}

#[test]
fn unreadable_workflow_is_unverified() {
    let root = pending_demo("bad-yaml");
    write_workflow(&root, "dist.yml", "on: [\n");
    commit(&root, "workflow");
    add_bare_origin(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("on: block is not readable"), "{stderr}");
    assert_eq!(local_tags(&root).trim(), "v0.1.0");
    create.assert_calls(0);
}

#[test]
fn ci_dispatch_button_is_a_completed_look() {
    let root = pending_demo("ci-button");
    write_workflow(&root, "release.yml", "on: push\n");
    write_workflow(
        &root,
        "ci.yml",
        "on:\n  pull_request:\n  push:\n    branches: [main]\n  workflow_dispatch:\n",
    );
    commit(&root, "workflow");
    add_bare_origin(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let runs = server.mock(|when, then| {
        when.method(GET).path("/repos/oakoss/oakum/actions/runs");
        then.status(200)
            .json_body(json!({ "total_count": 0, "workflow_runs": [] }));
    });
    let out = release_cmd(&root, &server);
    assert!(
        out.status.success(),
        "{}{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    let stderr = stderr_of(&out);
    assert!(stderr.contains("no downstream workflow"), "{stderr}");
    create.assert();
    runs.assert_calls(0);
}

#[test]
fn leftover_plus_new_run_confirms() {
    let root = pending_demo("leftover-new");
    write_workflow(&root, "dist.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    commit(&root, "workflow");
    add_bare_origin(&root);
    let sha = head_sha(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let runs = mock_workflow_hits(
        &server,
        &sha,
        &[
            RunHit::Run {
                id: 9,
                path: ".github/workflows/dist.yml".into(),
            },
            RunHit::Runs {
                ids: vec![9, 10],
                path: ".github/workflows/dist.yml".into(),
            },
        ],
    );
    let out = release_cmd(&root, &server);
    assert!(
        out.status.success(),
        "{}{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    let stdout = stdout_of(&out);
    assert!(
        stdout.contains("https://github.com/oakoss/oakum/actions/runs/10"),
        "{stdout}"
    );
    create.assert();
    assert_eq!(runs.iter().map(httpmock::Mock::calls).sum::<usize>(), 2);
}

#[test]
fn confirm_second_look_finds_the_run() {
    let root = pending_demo("second-look");
    write_workflow(&root, "dist.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    commit(&root, "workflow");
    add_bare_origin(&root);
    let sha = head_sha(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let runs = mock_workflow_hits(
        &server,
        &sha,
        &[
            RunHit::Empty,
            RunHit::Empty,
            RunHit::Run {
                id: 9,
                path: ".github/workflows/dist.yml".into(),
            },
        ],
    );
    let out = bin()
        .arg("release")
        .current_dir(&root)
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("OAKUM_HANDOFF_FAST", "2")
        .output()
        .expect("release");
    assert!(
        out.status.success(),
        "{}{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    let stdout = stdout_of(&out);
    assert!(
        stdout.contains("https://github.com/oakoss/oakum/actions/runs/9"),
        "{stdout}"
    );
    create.assert();
    assert_eq!(runs.iter().map(httpmock::Mock::calls).sum::<usize>(), 3);
}

#[test]
fn confirm_look_count_does_not_see_a_later_run() {
    let root = pending_demo("look-cap");
    write_workflow(&root, "dist.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    commit(&root, "workflow");
    add_bare_origin(&root);
    let sha = head_sha(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let runs = mock_workflow_hits(
        &server,
        &sha,
        &[
            RunHit::Empty,
            RunHit::Empty,
            RunHit::Empty,
            RunHit::Run {
                id: 9,
                path: ".github/workflows/dist.yml".into(),
            },
        ],
    );
    let out = bin()
        .arg("release")
        .current_dir(&root)
        .env("GITHUB_TOKEN", "token")
        .env("GITHUB_API_URL", server.base_url())
        .env("GITHUB_REPOSITORY", "oakoss/oakum")
        .env("OAKUM_HANDOFF_FAST", "2")
        .output()
        .expect("release");
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stdout = stdout_of(&out);
    let stderr = stderr_of(&out);
    assert!(
        !stdout.contains("https://github.com/oakoss/oakum/actions/runs/9"),
        "{stdout}"
    );
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("after 2 looks"), "{stderr}");
    create.assert();
    assert_eq!(runs.iter().map(httpmock::Mock::calls).sum::<usize>(), 3);
}

#[test]
fn snapshot_500_creates_no_tag() {
    let root = pending_demo("snap-500");
    write_workflow(&root, "dist.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    commit(&root, "workflow");
    add_bare_origin(&root);
    let sha = head_sha(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let runs = mock_workflow_hits(&server, &sha, &[RunHit::Fail(500)]);
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("unverified"), "{stderr}");
    assert_eq!(local_tags(&root).trim(), "v0.1.0");
    create.assert_calls(0);
    assert_eq!(runs.iter().map(httpmock::Mock::calls).sum::<usize>(), 1);
}

#[test]
fn tag_listener_plus_dispatch_sibling_confirms() {
    let root = pending_demo("sibling-dispatch");
    write_workflow(&root, "dist.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    write_workflow(
        &root,
        "manual.yml",
        "on:\n  workflow_dispatch:\n    inputs: {}\n",
    );
    commit(&root, "workflow");
    add_bare_origin(&root);
    let sha = head_sha(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let runs = mock_workflow_hits(
        &server,
        &sha,
        &[
            RunHit::Empty,
            RunHit::Run {
                id: 9,
                path: ".github/workflows/dist.yml".into(),
            },
        ],
    );
    let out = release_cmd(&root, &server);
    assert!(
        out.status.success(),
        "{}{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    create.assert();
    assert_eq!(runs.iter().map(httpmock::Mock::calls).sum::<usize>(), 2);
}

#[test]
fn confirm_500_after_release_reports_released() {
    let root = pending_demo("confirm-500");
    write_workflow(&root, "dist.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    commit(&root, "workflow");
    add_bare_origin(&root);
    let sha = head_sha(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let runs = mock_workflow_hits(&server, &sha, &[RunHit::Empty, RunHit::Fail(500)]);
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("v0.1.1 (released)"), "{stderr}");
    assert_eq!(
        stderr.matches("v0.1.1").count(),
        1,
        "the tag must be named once, at one stage: {stderr}"
    );
    assert!(
        local_tags(&root).contains("v0.1.1"),
        "{}",
        local_tags(&root)
    );
    create.assert();
    assert_eq!(runs.iter().map(httpmock::Mock::calls).sum::<usize>(), 2);
}

#[test]
fn dispatch_event_does_not_confirm() {
    let root = pending_demo("dispatch-event");
    write_workflow(&root, "dist.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    commit(&root, "workflow");
    add_bare_origin(&root);
    let sha = head_sha(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let runs = mock_workflow_hits(
        &server,
        &sha,
        &[
            RunHit::Empty,
            RunHit::Event {
                id: 9,
                path: ".github/workflows/dist.yml".into(),
                event: "workflow_dispatch",
            },
        ],
    );
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("unverified"), "{stderr}");
    create.assert();
    assert_eq!(runs.iter().map(httpmock::Mock::calls).sum::<usize>(), 2);
}

#[test]
fn create_event_confirms_the_handoff() {
    let root = pending_demo("create-event");
    write_workflow(&root, "dist.yml", "on: create\n");
    commit(&root, "workflow");
    add_bare_origin(&root);
    let sha = head_sha(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let runs = mock_workflow_hits(
        &server,
        &sha,
        &[
            RunHit::Empty,
            RunHit::Event {
                id: 9,
                path: ".github/workflows/dist.yml".into(),
                event: "create",
            },
        ],
    );
    let out = release_cmd(&root, &server);
    assert!(
        out.status.success(),
        "{}{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    let stdout = stdout_of(&out);
    assert!(
        stdout.contains("https://github.com/oakoss/oakum/actions/runs/9"),
        "{stdout}"
    );
    create.assert();
    assert_eq!(runs.iter().map(httpmock::Mock::calls).sum::<usize>(), 2);
}

#[test]
fn pull_request_event_does_not_confirm() {
    let root = pending_demo("pr-event");
    write_workflow(&root, "dist.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    commit(&root, "workflow");
    add_bare_origin(&root);
    let sha = head_sha(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let runs = mock_workflow_hits(
        &server,
        &sha,
        &[
            RunHit::Empty,
            RunHit::Event {
                id: 9,
                path: ".github/workflows/dist.yml".into(),
                event: "pull_request",
            },
        ],
    );
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("unverified"), "{stderr}");
    create.assert();
    assert_eq!(runs.iter().map(httpmock::Mock::calls).sum::<usize>(), 2);
}

#[test]
fn tags_ignore_workflow_confirms_the_handoff() {
    let root = pending_demo("tags-ignore");
    write_workflow(
        &root,
        "dist.yml",
        "on:\n  push:\n    tags-ignore:\n      - '*-dev'\n",
    );
    commit(&root, "workflow");
    add_bare_origin(&root);
    let sha = head_sha(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let runs = mock_workflow_hits(
        &server,
        &sha,
        &[
            RunHit::Empty,
            RunHit::Run {
                id: 9,
                path: ".github/workflows/dist.yml".into(),
            },
        ],
    );
    let out = release_cmd(&root, &server);
    assert!(
        out.status.success(),
        "{}{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    let stdout = stdout_of(&out);
    assert!(
        stdout.contains("https://github.com/oakoss/oakum/actions/runs/9"),
        "{stdout}"
    );
    create.assert();
    assert_eq!(runs.iter().map(httpmock::Mock::calls).sum::<usize>(), 2);
}

#[test]
fn leftover_run_confirms_when_tag_already_on_remote() {
    let root = pending_demo("resume-leftover");
    write_workflow(&root, "dist.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    commit(&root, "workflow");
    add_bare_origin(&root);
    git(&root, &["tag", "v0.1.1"]);
    git(&root, &["push", "origin", "refs/tags/v0.1.1"]);
    git(&root, &["tag", "-d", "v0.1.1"]);
    let sha = head_sha(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let runs = mock_workflow_hits(
        &server,
        &sha,
        &[
            RunHit::Run {
                id: 9,
                path: ".github/workflows/dist.yml".into(),
            },
            RunHit::Run {
                id: 9,
                path: ".github/workflows/dist.yml".into(),
            },
        ],
    );
    let out = release_cmd(&root, &server);
    assert!(
        out.status.success(),
        "{}{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    let stdout = stdout_of(&out);
    assert!(
        stdout.contains("https://github.com/oakoss/oakum/actions/runs/9"),
        "{stdout}"
    );
    create.assert();
    assert_eq!(runs.iter().map(httpmock::Mock::calls).sum::<usize>(), 2);
}

#[test]
fn leftover_without_path_does_not_confirm_when_path_appears() {
    let root = pending_demo("leftover-no-path");
    write_workflow(&root, "dist.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    commit(&root, "workflow");
    add_bare_origin(&root);
    let sha = head_sha(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let runs = mock_workflow_hits(
        &server,
        &sha,
        &[
            RunHit::RunWithoutPath { id: 9 },
            RunHit::Run {
                id: 9,
                path: ".github/workflows/dist.yml".into(),
            },
        ],
    );
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("v0.1.1 (released)"), "{stderr}");
    create.assert();
    assert_eq!(runs.iter().map(httpmock::Mock::calls).sum::<usize>(), 2);
}

#[test]
fn snapshot_304_creates_no_tag() {
    let root = pending_demo("snap-304");
    write_workflow(&root, "dist.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    commit(&root, "workflow");
    add_bare_origin(&root);
    let sha = head_sha(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let runs = mock_workflow_hits(&server, &sha, &[RunHit::Fail(304)]);
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("not fresh"), "{stderr}");
    assert_eq!(local_tags(&root).trim(), "v0.1.0");
    create.assert_calls(0);
    assert_eq!(runs.iter().map(httpmock::Mock::calls).sum::<usize>(), 1);
}

#[test]
fn leftover_without_event_does_not_confirm_when_event_appears() {
    let root = pending_demo("leftover-no-event");
    write_workflow(&root, "dist.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    commit(&root, "workflow");
    add_bare_origin(&root);
    let sha = head_sha(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let runs = mock_workflow_hits(
        &server,
        &sha,
        &[
            RunHit::RunWithoutEvent {
                id: 9,
                path: ".github/workflows/dist.yml".into(),
            },
            RunHit::Run {
                id: 9,
                path: ".github/workflows/dist.yml".into(),
            },
        ],
    );
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("v0.1.1 (released)"), "{stderr}");
    create.assert();
    assert_eq!(runs.iter().map(httpmock::Mock::calls).sum::<usize>(), 2);
}

#[test]
fn confirm_304_after_release_is_unverified() {
    let root = pending_demo("confirm-304");
    write_workflow(&root, "dist.yml", "on:\n  push:\n    tags:\n      - '*'\n");
    commit(&root, "workflow");
    add_bare_origin(&root);
    let sha = head_sha(&root);
    let server = MockServer::start();
    mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let runs = mock_workflow_hits(&server, &sha, &[RunHit::Empty, RunHit::Fail(304)]);
    let out = release_cmd(&root, &server);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let stderr = stderr_of(&out);
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("v0.1.1 (released)"), "{stderr}");
    assert!(
        local_tags(&root).contains("v0.1.1"),
        "{}",
        local_tags(&root)
    );
    create.assert();
    assert_eq!(runs.iter().map(httpmock::Mock::calls).sum::<usize>(), 2);
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

/// A fake ssh that is also a working local transport: it records the arguments
/// git passed, then serves the request from a bare repository on disk. Without
/// that, `ls-remote` fails and the push is never reached — which is why the
/// push arm went unmeasured while every other remote in this file was a plain
/// path that never invokes ssh.
#[cfg(unix)]
fn local_ssh_transport(root: &Path, bare: &Path, log: &Path) -> PathBuf {
    let script = root.join("fake-ssh");
    install_executable(
        &script,
        format!(
            "#!/bin/sh\nargs=\"$*\"\nfor a in \"$@\"; do last=$a; done\n\
             for a in \"$@\"; do [ \"$a\" = -G ] && {{ printf 'probe :: %s\\n' \"$args\" >> {log}; exit 0; }}; done\n\
             verb=${{last%% *}}\nprintf '%s :: %s\\n' \"$verb\" \"$args\" >> {log}\n\
             exec git \"${{verb#git-}}\" {bare}\n",
            log = log.display(),
            bare = bare.display()
        ),
    );
    script
}

#[cfg(unix)]
#[test]
fn the_tag_push_refuses_prompts() {
    let root = pending_demo("push-batchmode");
    let bare = root.parent().expect("parent").join("push-batchmode.git");
    let _ = fs::remove_dir_all(&bare);
    git(&root, &["init", "--bare", bare.to_str().expect("utf-8")]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "git@example.invalid:demo/demo.git",
        ],
    );
    // Outside the checkout: `release` refuses to tag a dirty worktree.
    let scratch = root.parent().expect("parent");
    let log = scratch.join("push-batchmode-ssh.log");
    let _ = fs::remove_file(&log);
    let script = local_ssh_transport(scratch, &bare, &log);

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/oakoss/oakum/releases/tags/v0.1.1");
        then.status(404).body("Not Found");
    });
    let create = server.mock(|when, then| {
        when.method(POST).path("/repos/oakoss/oakum/releases");
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
        .env("GIT_SSH_COMMAND", script.to_str().expect("utf-8"))
        .output()
        .expect("release");
    assert!(
        out.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    create.assert();

    let recorded = fs::read_to_string(&log).unwrap_or_default();
    let push = recorded
        .lines()
        .find(|line| line.starts_with("git-receive-pack ::"))
        .unwrap_or_else(|| panic!("no push reached the transport; recorded: {recorded:?}"));
    assert!(
        push.contains("-o BatchMode=yes"),
        "the tag push can still stop at an ssh prompt; recorded: {push:?}"
    );
}

/// A `[skip ci]` trailer usually sits in the body, not the subject. Reading only
/// the subject would tag and release a commit that asked not to be.
#[test]
fn skip_ci_in_the_commit_body_creates_no_tag() {
    let root = temp_git_repo("skip-body");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    cargo_package(&root, "demo", "0.1.1");
    git(&root, &["add", "-A"]);
    git(
        &root,
        &[
            "-c",
            "user.email=oakum@test",
            "-c",
            "user.name=oakum",
            "commit",
            "--no-verify",
            "-m",
            "chore: release demo 0.1.1",
            "-m",
            "[skip ci]",
        ],
    );
    add_bare_origin(&root);
    let server = MockServer::start();
    let lookup = mock_lookup_empty(&server, "v0.1.1");
    let create = mock_create(&server, "v0.1.1", 201);
    let out = release_cmd(&root, &server);
    let stderr = stderr_of(&out);
    assert!(!out.status.success(), "{stderr}");
    assert!(stderr.contains("skip-ci"), "{stderr}");
    assert_eq!(local_tags(&root).trim(), "v0.1.0");
    lookup.assert_calls(0);
    create.assert_calls(0);
}
