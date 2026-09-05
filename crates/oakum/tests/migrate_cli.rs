//! `oakum migrate` (`okm-de5`).

#![allow(clippy::disallowed_methods)]

mod support;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
#[cfg(unix)]
use std::process::{Command, Output};
#[cfg(unix)]
use support::fixture::git_env;
#[cfg(unix)]
use support::fixture::install_executable;
use support::fixture::{cargo_package, oakum, plain_repo, sibling, Fixture};
#[cfg(unix)]
use support::repo_state::RepoState;

use httpmock::prelude::*;
use serde_json::json;

const BINARY_VERSION: &str = env!("CARGO_PKG_VERSION");
const CHECKOUT_PIN: &str = "v9.9.9";

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

fn temp_repo(label: &str) -> Fixture {
    let root = plain_repo("migrate", label);
    fs::create_dir(root.join(".git")).expect("fixture .git");
    root
}

fn migrate(root: &Path) -> std::process::Output {
    let server = mock_checkout_latest();
    oakum(root)
        .args(["migrate"])
        .env("GITHUB_API_URL", server.base_url())
        .output()
        .expect("oakum migrate")
}

fn migrate_args(root: &Path, args: &[&str]) -> std::process::Output {
    let server = mock_checkout_latest();
    oakum(root)
        .args(["migrate"])
        .args(args)
        .env("GITHUB_API_URL", server.base_url())
        .output()
        .expect("oakum migrate")
}

/// Runs `oakum migrate` on a pseudo-TTY via python3. Sends `answer` when the
/// confirmation prompt appears; pass `None` when the run should not prompt.
#[cfg(unix)]
fn migrate_on_tty(
    root: &Path,
    api_url: &str,
    migrate_args: &[&str],
    answer: Option<&str>,
) -> Output {
    const SCRIPT: &str = r#"
import errno
import os
import pty
import select
import subprocess
import sys

def read_pty(master):
    try:
        return os.read(master, 4096)
    except OSError as err:
        if err.errno == errno.EIO:
            return b""
        raise

cmd = [sys.argv[1], "migrate", *sys.argv[5:]]
master, slave = pty.openpty()
proc = subprocess.Popen(
    cmd,
    cwd=sys.argv[2],
    stdin=slave,
    stdout=slave,
    stderr=slave,
    env={**os.environ, "GITHUB_API_URL": sys.argv[4]},
    close_fds=True,
)
os.close(slave)
output = b""
answer = sys.argv[3]
while True:
    ready, _, _ = select.select([master], [], [], 15)
    if not ready:
        break
    chunk = read_pty(master)
    if not chunk:
        break
    output += chunk
    if answer and b"Apply these changes?" in output:
        os.write(master, answer.encode())
        break
while True:
    ready, _, _ = select.select([master], [], [], 5)
    if not ready:
        break
    chunk = read_pty(master)
    if not chunk:
        break
    output += chunk
code = proc.wait(timeout=30)
sys.stdout.buffer.write(output)
sys.exit(code)
"#;
    let mut command = Command::new("python3");
    git_env(&mut command, root);
    command
        .arg("-c")
        .arg(SCRIPT)
        .arg(env!("CARGO_BIN_EXE_oakum"))
        .arg(root)
        .arg(answer.unwrap_or(""))
        .arg(api_url);
    for arg in migrate_args {
        command.arg(arg);
    }
    command.output().expect("python3 pty migrate")
}

fn config_path(root: &Path) -> PathBuf {
    root.join(".changeset/_config.toml")
}

/// Default fixtures have no runnable source tool → writes kept, exit unverified.
fn assert_migrate_unverified_kept(output: &std::process::Output, root: &Path) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    assert!(
        !output.status.success(),
        "expected unverified exit; stdout={stdout}\nstderr={stderr}"
    );
    assert!(
        combined.contains("unverified"),
        "output must name unverified: {combined}"
    );
    assert!(
        combined.contains("source-tool before-plan unavailable")
            || combined.contains("no packages discovered"),
        "{combined}"
    );
    assert!(
        combined.contains("will exit unverified")
            || combined.contains("plan comparison skipped: no packages discovered"),
        "{combined}"
    );
    // Banner alone is not load-bearing; resolve_before_proof prints it before conclude.
    if combined.contains("will exit unverified") {
        assert!(
            combined.contains("source-tool before-plan unavailable"),
            "Simulated path must fail with unavailable, not only the banner: {combined}"
        );
    }
    assert!(
        config_path(root).is_file(),
        "migrate must keep writes on unverified fallback"
    );
}

#[test]
fn nothing_to_migrate_names_init() {
    let root = temp_repo("empty");
    let output = migrate(&root);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("oakum init"), "{err}");
    assert!(!config_path(&root).exists());
}

#[test]
fn quoted_unscoped_keys_are_rewritten() {
    let root = temp_repo("quoted");
    cargo_package(&root, "core", "0.1.0");
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
    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);
    let body = fs::read_to_string(root.join(".changeset/feat.md")).expect("bump");
    assert_eq!(body, "---\ncore: minor\n---\nnote\n");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("pending:"), "{stdout}");
    assert!(stdout.contains("drop `access`"), "{stdout}");
    assert!(stdout.contains("drop `changelog`"), "{stdout}");
    assert!(stdout.contains("rewrote .changeset/feat.md"), "{stdout}");
    assert!(stdout.contains("remaining"), "{stdout}");
    assert_eq!(
        stdout
            .matches(&format!("actions/checkout@{CHECKOUT_PIN}"))
            .count(),
        3,
        "{stdout}"
    );
    assert!(!stdout.contains("actions/checkout@v4"), "{stdout}");
    let config = fs::read_to_string(config_path(&root)).expect("oakum config");
    assert!(config.contains("versioning = \"semver\""), "{config}");
    assert!(
        config.contains(&format!("tool-version = \"{BINARY_VERSION}\"")),
        "{config}"
    );
    assert!(root.join(".changeset/config.json").is_file());
    let readme = fs::read_to_string(root.join(".changeset/README.md")).expect("readme");
    support::assert_shipped_changeset_readme(&readme);
}

#[test]
fn checkout_lookup_failure_is_unverified_and_writes_nothing() {
    let root = temp_repo("checkout-500");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\n\"core\": minor\n---\nnote\n",
    )
    .expect("bump");
    fs::write(
        root.join(".changeset/config.json"),
        r#"{"changelog": "@changesets/cli/changelog"}"#,
    )
    .expect("config");
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/actions/checkout/releases/latest");
        then.status(500);
    });
    let output = oakum(&root)
        .args(["migrate"])
        .env("GITHUB_API_URL", server.base_url())
        .output()
        .expect("oakum migrate");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(
        stderr.contains("/repos/actions/checkout/releases/latest"),
        "{stderr}"
    );
    assert!(stderr.contains("500"), "{stderr}");
    assert!(!config_path(&root).exists());
    assert!(!root.join(".changeset/_schema.json").exists());
    assert!(!root.join(".changeset/README.md").exists());
    let body = fs::read_to_string(root.join(".changeset/feat.md")).expect("bump");
    assert_eq!(body, "---\n\"core\": minor\n---\nnote\n");
}

#[test]
fn knope_sets_zero_major_and_warns_about_readme() {
    let root = temp_repo("knope");
    cargo_package(&root, "core", "0.1.0");
    fs::write(root.join("knope.toml"), "").expect("knope");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\n\"core\": patch\n---\n",
    )
    .expect("bump");
    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);
    let config = fs::read_to_string(config_path(&root)).expect("config");
    let bump = fs::read_to_string(root.join(".changeset/feat.md")).expect("bump");
    assert_eq!(bump, "---\ncore: patch\n---\n");
    assert!(config.contains("versioning = \"zero-major\""), "{config}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("knope.toml"), "{stdout}");
    assert!(stdout.contains("aborts knope"), "{stdout}");
    assert!(!stdout.contains("remove .changeset/"), "{stdout}");
    assert!(root.join("knope.toml").is_file());
}

#[test]
fn knope_plus_scoped_package_refuses_and_writes_nothing() {
    let root = temp_repo("scoped-knope");
    fs::write(root.join("knope.toml"), "").expect("knope");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\n\"@oakum/cli\": minor\n---\n",
    )
    .expect("bump");
    let output = migrate(&root);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("@oakum/cli"), "{err}");
    assert!(err.contains("knope.toml"), "{err}");
    assert!(!config_path(&root).exists());
    assert!(!root.join(".changeset/_schema.json").exists());
    assert!(!root.join(".changeset/README.md").exists());
    let body = fs::read_to_string(root.join(".changeset/feat.md")).expect("bump");
    assert!(body.contains("\"@oakum/cli\""), "{body}");
}

#[test]
fn already_migrated_is_idempotent() {
    let root = temp_repo("again");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\n\"core\": minor\n---\n",
    )
    .expect("bump");
    fs::write(root.join(".changeset/config.json"), "{}").expect("json");
    let first = migrate(&root);
    assert_migrate_unverified_kept(&first, &root);
    let bump_before = fs::read_to_string(root.join(".changeset/feat.md")).expect("bump");
    assert_eq!(bump_before, "---\ncore: minor\n---\n");
    let config_before = fs::read_to_string(config_path(&root)).expect("config");
    let output = migrate(&root);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("already migrated"), "{stdout}");
    assert_eq!(
        fs::read_to_string(root.join(".changeset/feat.md")).expect("bump"),
        bump_before
    );
    assert_eq!(
        fs::read_to_string(config_path(&root)).expect("config"),
        config_before
    );
}

#[test]
fn already_migrated_refuses_a_missing_template_file() {
    let root = temp_repo("missing-tpl");
    fs::create_dir(root.join(".changeset")).expect("dir");
    let body =
        format!("tool-version = \"{BINARY_VERSION}\"\ntag-format = {{ file = \"notes.md\" }}\n");
    fs::write(config_path(&root), &body).expect("config");
    let output = migrate(&root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "missing template file must fail: stdout={stdout} stderr={err}"
    );
    assert!(err.contains("failed to resolve template"), "{err}");
    assert!(err.contains("tag-format"), "{err}");
    assert!(!stdout.contains("already migrated"), "{stdout}");
    assert_eq!(
        fs::read_to_string(config_path(&root)).expect("config"),
        body
    );
    assert!(!root.join(".changeset/_schema.json").exists());
}

#[test]
fn versioning_flag_overrides_inference() {
    let root = temp_repo("override");
    fs::write(root.join("knope.toml"), "").expect("knope");
    let output = migrate_args(&root, &["--versioning", "semver"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config = fs::read_to_string(config_path(&root)).expect("config");
    assert!(config.contains("versioning = \"semver\""), "{config}");
}

#[test]
fn instruction_file_is_warned() {
    let root = temp_repo("agents");
    fs::write(root.join("knope.toml"), "").expect("knope");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(root.join(".changeset/AGENTS.md"), "notes\n").expect("agents");
    let output = migrate(&root);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("AGENTS.md"), "{stdout}");
    assert!(
        stdout.contains("aborts knope") || stdout.contains("`AGENTS.md`"),
        "{stdout}"
    );
}

#[test]
fn malformed_later_file_does_not_rewrite_earlier_files() {
    let root = temp_repo("partial");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(root.join(".changeset/a.md"), "---\n\"core\": minor\n---\n").expect("a");
    fs::write(root.join(".changeset/b.md"), "not a bump file\n").expect("b");
    let output = migrate(&root);
    assert!(!output.status.success());
    let body = fs::read_to_string(root.join(".changeset/a.md")).expect("a");
    assert!(body.contains("\"core\""), "{body}");
    assert!(!config_path(&root).exists());
}

#[test]
fn knope_with_none_level_refuses() {
    let root = temp_repo("knope-none");
    cargo_package(&root, "core", "0.1.0");
    fs::write(root.join("knope.toml"), "").expect("knope");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(root.join(".changeset/feat.md"), "---\ncore: none\n---\n").expect("bump");
    let output = migrate(&root);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("a `none` entry is unsafe while knope.toml is present"),
        "{err}"
    );
    assert!(!config_path(&root).exists());
}

#[test]
fn changesets_none_level_is_preserved() {
    let root = temp_repo("changesets-none");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/config.json"),
        r#"{"changelog": "@changesets/cli/changelog"}"#,
    )
    .expect("config");
    fs::write(
        root.join(".changeset/cover.md"),
        "---\n\"core\": none\n---\ncovered without a release\n",
    )
    .expect("bump");
    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);
    let body = fs::read_to_string(root.join(".changeset/cover.md")).expect("bump");
    assert_eq!(body, "---\ncore: none\n---\ncovered without a release\n");
    assert!(config_path(&root).is_file());
}

#[test]
fn bumpy_none_level_is_preserved() {
    let root = temp_repo("bumpy-none");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".bumpy")).expect("dir");
    fs::write(root.join(".bumpy/_config.json"), "{}").expect("config");
    fs::write(
        root.join(".bumpy/cover.md"),
        "---\n\"core\": none\n---\ncovered without a release\n",
    )
    .expect("bump");
    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);
    let body = fs::read_to_string(root.join(".changeset/cover.md")).expect("copied");
    assert_eq!(body, "---\ncore: none\n---\ncovered without a release\n");
    assert!(root.join(".bumpy/cover.md").is_file());
    assert!(config_path(&root).is_file());
}

#[test]
fn changesets_empty_frontmatter_is_preserved() {
    let root = temp_repo("changesets-empty");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/config.json"),
        r#"{"changelog": "@changesets/cli/changelog"}"#,
    )
    .expect("config");
    fs::write(
        root.join(".changeset/empty.md"),
        "---\n---\nintentionally releaseless\n",
    )
    .expect("bump");
    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);
    let body = fs::read_to_string(root.join(".changeset/empty.md")).expect("bump");
    assert_eq!(body, "---\n---\nintentionally releaseless\n");
    assert!(config_path(&root).is_file());
}

#[test]
fn bumpy_empty_frontmatter_is_preserved() {
    let root = temp_repo("bumpy-empty");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".bumpy")).expect("dir");
    fs::write(root.join(".bumpy/_config.json"), "{}").expect("config");
    fs::write(
        root.join(".bumpy/empty.md"),
        "---\n---\nintentionally releaseless\n",
    )
    .expect("bump");
    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);
    let body = fs::read_to_string(root.join(".changeset/empty.md")).expect("copied");
    assert_eq!(body, "---\n---\nintentionally releaseless\n");
    assert!(root.join(".bumpy/empty.md").is_file());
    assert!(config_path(&root).is_file());
}

#[test]
fn knope_with_empty_frontmatter_refuses() {
    let root = temp_repo("knope-empty");
    cargo_package(&root, "core", "0.1.0");
    fs::write(root.join("knope.toml"), "").expect("knope");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(root.join(".changeset/empty.md"), "---\n---\nnote\n").expect("empty");
    let output = migrate(&root);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("empty frontmatter is unsafe while knope.toml is present"),
        "{err}"
    );
    assert!(!config_path(&root).exists());
}

#[test]
fn bumpy_pending_files_are_copied_into_changeset() {
    let root = temp_repo("bumpy");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".bumpy")).expect("dir");
    fs::write(root.join(".bumpy/_config.json"), "{}").expect("config");
    fs::write(
        root.join(".bumpy/feat.md"),
        "---\n\"core\": minor\n---\nnote\n",
    )
    .expect("bump");
    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);
    let body = fs::read_to_string(root.join(".changeset/feat.md")).expect("copied");
    assert_eq!(body, "---\ncore: minor\n---\nnote\n");
    assert!(root.join(".bumpy/feat.md").is_file());
    assert!(config_path(&root).is_file());
}

#[test]
fn knope_pre1_feature_is_expected_plan_divergence() {
    let root = temp_repo("knope-feature");
    cargo_package(&root, "core", "0.1.0");
    fs::write(root.join("knope.toml"), "").expect("knope");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(root.join(".changeset/feat.md"), "---\ncore: minor\n---\n").expect("bump");
    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("pending:"), "{stdout}");
    assert!(
        stdout.contains("knope maps a pending feature on a pre-1.0 package to patch"),
        "{stdout}"
    );
    assert!(stdout.contains("0.1.0 → 0.1.1 (knope)"), "{stdout}");
    assert!(stdout.contains("0.1.0 → 0.2.0 (oakum)"), "{stdout}");
    assert!(!stdout.contains("unexpected difference"), "{stdout}");
    assert!(config_path(&root).is_file());
}

#[test]
fn knope_pre1_patch_plans_match() {
    let root = temp_repo("knope-patch");
    cargo_package(&root, "core", "0.1.0");
    fs::write(root.join("knope.toml"), "").expect("knope");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(root.join(".changeset/feat.md"), "---\ncore: patch\n---\n").expect("bump");
    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("will exit unverified"), "{stdout}");
    assert!(!stdout.contains("unexpected difference"), "{stdout}");
}

#[test]
fn unexpected_plan_difference_keeps_transform() {
    let root = temp_repo("plan-diff");
    cargo_package(&root, "core", "0.1.0");
    fs::write(root.join("knope.toml"), "").expect("knope");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(root.join(".changeset/feat.md"), "---\ncore: major\n---\n").expect("bump");
    let output = migrate_args(&root, &["--versioning", "semver"]);
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("plan comparison: unexpected difference"),
        "{stdout}"
    );
    assert!(stdout.contains("0.1.0 → 0.2.0"), "{stdout}");
    assert!(stdout.contains("0.1.0 → 1.0.0"), "{stdout}");
    assert!(stderr.contains("migrated files were kept"), "{stderr}");
    assert!(
        !stderr.contains("unverified"),
        "unexpected diffs are hard failures, not unverified: {stderr}"
    );
    assert!(stdout.contains("remaining"), "{stdout}");
    let remaining = stdout.find("remaining").expect("remaining");
    let banner = stdout
        .find("plan comparison: unexpected difference")
        .expect("banner");
    assert!(remaining > banner, "{stdout}");
    let bump = fs::read_to_string(root.join(".changeset/feat.md")).expect("bump");
    assert_eq!(bump, "---\ncore: major\n---\n");
    assert!(config_path(&root).is_file());
}

#[test]
fn unknown_package_is_reported_not_dropped() {
    let root = temp_repo("unknown");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\n\"ghost\": minor\n---\nnote\n",
    )
    .expect("bump");
    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("unknown package `ghost` in `.changeset/feat.md`"),
        "{stdout}"
    );
    let body = fs::read_to_string(root.join(".changeset/feat.md")).expect("bump");
    assert_eq!(body, "---\nghost: minor\n---\nnote\n");
}

#[test]
fn changeset_subdirectory_is_reported() {
    let root = temp_repo("subdir");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir_all(root.join(".changeset/nested")).expect("dir");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\n\"core\": patch\n---\n",
    )
    .expect("bump");
    fs::write(root.join(".changeset/nested/skip.md"), "ignored\n").expect("nested");
    fs::write(
        root.join(".changeset/nested/quoted.md"),
        "---\n\"core\": patch\n---\n",
    )
    .expect("nested bump");
    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("subdirectory `.changeset/nested` (ignored)"),
        "{stdout}"
    );
    let nested = fs::read_to_string(root.join(".changeset/nested/skip.md")).expect("nested");
    assert_eq!(nested, "ignored\n");
    let nested_bump =
        fs::read_to_string(root.join(".changeset/nested/quoted.md")).expect("nested bump");
    assert_eq!(nested_bump, "---\n\"core\": patch\n---\n");
}

#[test]
fn knope_pre1_major_plans_match_without_flag() {
    let root = temp_repo("knope-major");
    cargo_package(&root, "core", "0.1.0");
    fs::write(root.join("knope.toml"), "").expect("knope");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(root.join(".changeset/feat.md"), "---\ncore: major\n---\n").expect("bump");
    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("will exit unverified"), "{stdout}");
    assert!(!stdout.contains("unexpected difference"), "{stdout}");
}

#[test]
fn knope_pre1_feature_cascade_is_expected_divergence() {
    let root = temp_repo("knope-cascade");
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"lib\", \"app\"]\n",
    )
    .expect("workspace");
    fs::create_dir_all(root.join("lib/src")).expect("lib src");
    fs::write(
        root.join("lib/Cargo.toml"),
        "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("lib manifest");
    fs::write(root.join("lib/src/lib.rs"), "").expect("lib src");
    fs::create_dir_all(root.join("app/src")).expect("app src");
    fs::write(
        root.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ncore = { path = \"../lib\", version = \"0.1.0\" }\n",
    )
    .expect("app manifest");
    fs::write(root.join("app/src/lib.rs"), "").expect("app src");
    fs::write(root.join("knope.toml"), "").expect("knope");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(root.join(".changeset/feat.md"), "---\ncore: minor\n---\n").expect("bump");
    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("knope maps a pending feature on a pre-1.0 package to patch"),
        "{stdout}"
    );
    assert!(!stdout.contains("unexpected difference"), "{stdout}");
    assert!(
        stdout.contains("core (cargo): 0.1.0 → 0.1.1 (knope) vs 0.1.0 → 0.2.0 (oakum)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("app (cargo): absent (knope) vs 0.1.0 → 0.1.1 (oakum)"),
        "{stdout}"
    );
}

#[test]
fn knope_pre1_feature_transitive_cascade_is_expected_divergence() {
    let root = temp_repo("knope-transitive");
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"lib\", \"mid\", \"app\"]\n",
    )
    .expect("workspace");
    fs::create_dir_all(root.join("lib/src")).expect("lib src");
    fs::write(
        root.join("lib/Cargo.toml"),
        "[package]\nname = \"core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("lib manifest");
    fs::write(root.join("lib/src/lib.rs"), "").expect("lib src");
    fs::create_dir_all(root.join("mid/src")).expect("mid src");
    fs::write(
        root.join("mid/Cargo.toml"),
        "[package]\nname = \"mid\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ncore = { path = \"../lib\", version = \"0.1.0\" }\n",
    )
    .expect("mid manifest");
    fs::write(root.join("mid/src/lib.rs"), "").expect("mid src");
    fs::create_dir_all(root.join("app/src")).expect("app src");
    fs::write(
        root.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nmid = { path = \"../mid\", version = \"=0.1.0\" }\n",
    )
    .expect("app manifest");
    fs::write(root.join("app/src/lib.rs"), "").expect("app src");
    fs::write(root.join("knope.toml"), "").expect("knope");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(root.join(".changeset/feat.md"), "---\ncore: minor\n---\n").expect("bump");
    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("knope maps a pending feature on a pre-1.0 package to patch"),
        "{stdout}"
    );
    assert!(!stdout.contains("unexpected difference"), "{stdout}");
}

#[test]
fn quoted_rewrite_is_listed_as_pending() {
    let root = temp_repo("pending-rewrite");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\n\"core\": minor\n---\nnote\n",
    )
    .expect("bump");
    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("pending:"), "{stdout}");
    assert!(stdout.contains("rewrite .changeset/feat.md"), "{stdout}");
    let body = fs::read_to_string(root.join(".changeset/feat.md")).expect("bump");
    assert_eq!(body, "---\ncore: minor\n---\nnote\n");
}

#[test]
fn mixed_known_and_unknown_packages_are_kept() {
    let root = temp_repo("mixed-unknown");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\ncore: patch\n\"ghost\": minor\n---\nnote\n",
    )
    .expect("bump");
    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("unknown package `ghost` in `.changeset/feat.md`"),
        "{stdout}"
    );
    let body = fs::read_to_string(root.join(".changeset/feat.md")).expect("bump");
    assert_eq!(body, "---\ncore: patch\nghost: minor\n---\nnote\n");
}

#[test]
fn bump_files_without_packages_are_unverified() {
    let root = temp_repo("unverified");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\n\"core\": minor\n---\nnote\n",
    )
    .expect("bump");
    let output = migrate(&root);
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("plan comparison skipped: no packages discovered"),
        "{stdout}"
    );
    assert!(!stdout.contains("unknown package"), "{stdout}");
    assert!(stdout.contains("remaining"), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("migrated files were kept"), "{stderr}");
    assert!(config_path(&root).is_file());
    let body = fs::read_to_string(root.join(".changeset/feat.md")).expect("bump");
    assert_eq!(body, "---\ncore: minor\n---\nnote\n");
}

#[test]
fn bumpy_files_without_packages_are_unverified() {
    let root = temp_repo("unverified-bumpy");
    fs::create_dir(root.join(".bumpy")).expect("dir");
    fs::write(root.join(".bumpy/_config.json"), "{}").expect("config");
    fs::write(
        root.join(".bumpy/feat.md"),
        "---\n\"core\": minor\n---\nnote\n",
    )
    .expect("bump");
    let output = migrate(&root);
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("plan comparison skipped: no packages discovered"),
        "{stdout}"
    );
    assert!(stdout.contains("remaining"), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(config_path(&root).is_file());
}

#[test]
fn tool_version_mismatch_refuses() {
    let root = temp_repo("toolver");
    fs::create_dir(root.join(".changeset")).expect("changeset");
    fs::write(config_path(&root), "tool-version = \"9.9.9\"\n").expect("config");
    let output = migrate(&root);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("tool-version"), "{stderr}");
    assert!(stderr.contains("upgrade"), "{stderr}");
}

#[test]
fn non_tty_proceeds_without_reading_stdin() {
    let root = temp_repo("non-tty-stdin");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\n\"core\": minor\n---\nnote\n",
    )
    .expect("bump");
    fs::write(
        root.join(".changeset/config.json"),
        r#"{"changelog": "@changesets/cli/changelog"}"#,
    )
    .expect("config");
    let server = mock_checkout_latest();
    let mut child = oakum(&root)
        .args(["migrate"])
        .env("GITHUB_API_URL", server.base_url())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"n\n")
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait");
    assert_migrate_unverified_kept(&output, &root);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("Apply these changes?"),
        "non-TTY must not prompt: {stderr}"
    );
    assert!(config_path(&root).is_file());
}

#[cfg(unix)]
#[test]
fn tty_decline_leaves_repository_unchanged() {
    let root = temp_repo("tty-decline");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\n\"core\": minor\n---\nnote\n",
    )
    .expect("bump");
    fs::write(
        root.join(".changeset/config.json"),
        r#"{"changelog": "@changesets/cli/changelog"}"#,
    )
    .expect("config");
    let bump_before = fs::read(root.join(".changeset/feat.md")).expect("bump");
    let before = RepoState::capture(&root);
    let server = mock_checkout_latest();
    let output = migrate_on_tty(&root, &server.base_url(), &[], Some("n\n"));
    assert!(
        !output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("Apply these changes?"),
        "TTY must prompt: {combined}"
    );
    let prompt_at = combined
        .find("Apply these changes?")
        .expect("prompt position");
    let before_prompt = &combined[..prompt_at];
    assert!(before_prompt.contains("pending:"), "{combined}");
    assert!(
        before_prompt.contains("rewrite .changeset/feat.md"),
        "{combined}"
    );
    assert!(
        combined.contains("migration cancelled"),
        "decline must name cancellation: {combined}"
    );
    RepoState::assert_unchanged(&before, &root, "TTY decline");
    assert!(!config_path(&root).exists());
    assert_eq!(
        fs::read(root.join(".changeset/feat.md")).expect("bump"),
        bump_before
    );
}

#[cfg(unix)]
#[test]
fn tty_yes_skips_prompt_and_migrates() {
    let root = temp_repo("tty-yes");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\n\"core\": minor\n---\nnote\n",
    )
    .expect("bump");
    fs::write(
        root.join(".changeset/config.json"),
        r#"{"changelog": "@changesets/cli/changelog"}"#,
    )
    .expect("config");
    let server = mock_checkout_latest();
    let output = migrate_on_tty(&root, &server.base_url(), &["--yes"], None);
    assert_migrate_unverified_kept(&output, &root);
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !combined.contains("Apply these changes?"),
        "TTY --yes must skip the prompt: {combined}"
    );
    assert!(config_path(&root).is_file());
    let body = fs::read_to_string(root.join(".changeset/feat.md")).expect("bump");
    assert_eq!(body, "---\ncore: minor\n---\nnote\n");
}

#[test]
fn yes_flag_migrates_on_non_tty() {
    let root = temp_repo("yes-flag");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\n\"core\": minor\n---\nnote\n",
    )
    .expect("bump");
    fs::write(
        root.join(".changeset/config.json"),
        r#"{"changelog": "@changesets/cli/changelog"}"#,
    )
    .expect("config");
    let output = migrate_args(&root, &["--yes"]);
    assert_migrate_unverified_kept(&output, &root);
    assert!(config_path(&root).is_file());
}

#[cfg(unix)]
fn migrate_with_path(root: &Path, path_prefix: &Path) -> std::process::Output {
    let server = mock_checkout_latest();
    let path = format!(
        "{}:{}",
        path_prefix.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    oakum(root)
        .args(["migrate", "--yes"])
        .env("GITHUB_API_URL", server.base_url())
        .env("PATH", path)
        .output()
        .expect("oakum migrate")
}

#[cfg(unix)]
#[test]
fn bumpy_source_plan_shim_exits_verified() {
    let root = temp_repo("bumpy-shim-ok");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".bumpy")).expect("dir");
    fs::write(root.join(".bumpy/_config.json"), "{}").expect("config");
    fs::write(root.join(".bumpy/feat.md"), "---\ncore: minor\n---\nnote\n").expect("bump");
    let shim_dir = sibling(&root, "shim");
    fs::create_dir_all(&shim_dir).expect("shim");
    install_executable(
        &shim_dir.join("bumpy"),
        r#"#!/bin/sh
if [ "$1" = status ] && [ "$2" = --json ]; then
  printf '%s\n' '{"releases":[{"name":"core","type":"minor","oldVersion":"0.1.0","newVersion":"0.2.0"}],"packageNames":["core"],"bumpFiles":[]}'
  exit 0
fi
exit 1
"#,
    );
    let output = migrate_with_path(&root, &shim_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stdout.contains("before-plan from bumpy"), "{stdout}");
    assert!(!stderr.contains("unverified"), "{stderr}");
    assert!(config_path(&root).is_file());
}

#[cfg(unix)]
#[test]
fn bumpy_broken_shim_exits_unverified() {
    let root = temp_repo("bumpy-shim-bad");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".bumpy")).expect("dir");
    fs::write(root.join(".bumpy/_config.json"), "{}").expect("config");
    fs::write(root.join(".bumpy/feat.md"), "---\ncore: minor\n---\nnote\n").expect("bump");
    let shim_dir = sibling(&root, "shim");
    fs::create_dir_all(&shim_dir).expect("shim");
    install_executable(
        &shim_dir.join("bumpy"),
        r"#!/bin/sh
echo 'not-json' >&1
exit 0
",
    );
    let output = migrate_with_path(&root, &shim_dir);
    assert_migrate_unverified_kept(&output, &root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("source tool bumpy not runnable"),
        "{stdout}"
    );
}

#[cfg(unix)]
#[test]
fn changesets_source_plan_shim_exits_verified() {
    let root = temp_repo("changeset-shim-ok");
    cargo_package(&root, "core", "1.0.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/config.json"),
        r#"{"changelog": "@changesets/cli/changelog"}"#,
    )
    .expect("config");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\ncore: patch\n---\nnote\n",
    )
    .expect("bump");
    let shim_dir = sibling(&root, "shim");
    fs::create_dir_all(&shim_dir).expect("shim");
    install_executable(
        &shim_dir.join("changeset"),
        r#"#!/bin/sh
out=""
while [ $# -gt 0 ]; do
  case "$1" in
    --output) out="$2"; shift 2 ;;
    *) shift ;;
  esac
done
if [ -z "$out" ]; then
  echo "missing --output" >&2
  exit 1
fi
printf '%s\n' '{"releases":[{"name":"core","type":"patch","oldVersion":"1.0.0","newVersion":"1.0.1"}]}' > "$out"
exit 0
"#,
    );
    let output = migrate_with_path(&root, &shim_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stdout.contains("before-plan from changesets"), "{stdout}");
    assert!(!stderr.contains("unverified"), "{stderr}");
}

#[cfg(unix)]
#[test]
fn knope_source_plan_shim_expected_fallout_exits_verified() {
    let root = temp_repo("knope-shim-ok");
    cargo_package(&root, "core", "0.1.0");
    fs::write(root.join("knope.toml"), "").expect("knope");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(root.join(".changeset/feat.md"), "---\ncore: minor\n---\n").expect("bump");
    let shim_dir = sibling(&root, "shim");
    fs::create_dir_all(&shim_dir).expect("shim");
    // Real knope maps 0.x feature → patch; oakum after → minor.
    // Shim uses knope ≥0.23 `version = …` form.
    install_executable(
        &shim_dir.join("knope"),
        r#"#!/bin/sh
echo "Would add the following to Cargo.toml: version = 0.1.1"
exit 0
"#,
    );
    let output = migrate_with_path(&root, &shim_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stdout.contains("before-plan from knope"), "{stdout}");
    assert!(
        stdout.contains("knope maps a pending feature on a pre-1.0 package to patch"),
        "{stdout}"
    );
    assert!(!stderr.contains("unverified"), "{stderr}");
}

#[cfg(unix)]
#[test]
fn knope_failed_exit_with_scrape_is_unverified() {
    let root = temp_repo("knope-shim-fail-exit");
    cargo_package(&root, "core", "0.1.0");
    fs::write(root.join("knope.toml"), "").expect("knope");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(root.join(".changeset/feat.md"), "---\ncore: patch\n---\n").expect("bump");
    let shim_dir = sibling(&root, "shim");
    fs::create_dir_all(&shim_dir).expect("shim");
    install_executable(
        &shim_dir.join("knope"),
        r#"#!/bin/sh
echo "Would add the following to Cargo.toml: version = 0.1.1"
exit 1
"#,
    );
    let output = migrate_with_path(&root, &shim_dir);
    assert_migrate_unverified_kept(&output, &root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("source tool knope not runnable"),
        "{stdout}"
    );
}

#[cfg(unix)]
#[test]
fn knope_empty_scrape_is_unverified() {
    let root = temp_repo("knope-shim-empty");
    cargo_package(&root, "core", "0.1.0");
    fs::write(root.join("knope.toml"), "").expect("knope");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(root.join(".changeset/feat.md"), "---\ncore: patch\n---\n").expect("bump");
    let shim_dir = sibling(&root, "shim");
    fs::create_dir_all(&shim_dir).expect("shim");
    install_executable(
        &shim_dir.join("knope"),
        r#"#!/bin/sh
echo "Would delete: .changeset/feat.md"
exit 0
"#,
    );
    let output = migrate_with_path(&root, &shim_dir);
    assert_migrate_unverified_kept(&output, &root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("source tool knope not runnable"),
        "{stdout}"
    );
}

#[cfg(unix)]
#[test]
fn bumpy_source_plan_unexpected_diff_is_hard_failure() {
    let root = temp_repo("bumpy-shim-mismatch");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".bumpy")).expect("dir");
    fs::write(root.join(".bumpy/_config.json"), "{}").expect("config");
    fs::write(root.join(".bumpy/feat.md"), "---\ncore: minor\n---\nnote\n").expect("bump");
    let shim_dir = sibling(&root, "shim");
    fs::create_dir_all(&shim_dir).expect("shim");
    // Wrong newVersion vs oakum after (0.2.0): Source path must hard-fail, not unverified.
    install_executable(
        &shim_dir.join("bumpy"),
        r#"#!/bin/sh
if [ "$1" = status ] && [ "$2" = --json ]; then
  printf '%s\n' '{"releases":[{"name":"core","type":"minor","oldVersion":"0.1.0","newVersion":"0.1.9"}],"packageNames":["core"],"bumpFiles":[]}'
  exit 0
fi
exit 1
"#,
    );
    let output = migrate_with_path(&root, &shim_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stdout.contains("before-plan from bumpy"), "{stdout}");
    assert!(
        stdout.contains("plan comparison: unexpected difference"),
        "{stdout}"
    );
    assert!(stderr.contains("migrated files were kept"), "{stderr}");
    assert!(
        !stderr.contains("unverified"),
        "Source unexpected diffs are hard failures: {stderr}"
    );
    assert!(config_path(&root).is_file());
}

#[cfg(unix)]
#[test]
fn bumpy_empty_releases_exit_one_is_verified() {
    let root = temp_repo("bumpy-shim-empty");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".bumpy")).expect("dir");
    fs::write(root.join(".bumpy/_config.json"), "{}").expect("config");
    let shim_dir = sibling(&root, "shim");
    fs::create_dir_all(&shim_dir).expect("shim");
    install_executable(
        &shim_dir.join("bumpy"),
        r#"#!/bin/sh
if [ "$1" = status ] && [ "$2" = --json ]; then
  printf '%s\n' '{"releases":[],"packageNames":["core"],"bumpFiles":[]}'
  exit 1
fi
exit 2
"#,
    );
    let output = migrate_with_path(&root, &shim_dir);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "stdout={stdout}\nstderr={stderr}");
    assert!(stdout.contains("before-plan from bumpy"), "{stdout}");
    assert!(!stderr.contains("unverified"), "{stderr}");
}
