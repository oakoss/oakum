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
#[cfg(unix)]
use support::fixture::sibling;
use support::fixture::{
    cargo_package, commit, git_repo, oakum, plain_repo, private_workspace, tag_members_at_version,
    Fixture,
};
use support::repo_state::RepoState;

use httpmock::prelude::*;
use serde_json::json;

const BINARY_VERSION: &str = env!("CARGO_PKG_VERSION");
const CHECKOUT_PIN: &str = "v9.9.9";
const PNPM_SETUP_PIN: &str = "v8.8.8";

fn mock_checkout_latest() -> MockServer {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/actions/checkout/releases/latest");
        then.status(200)
            .json_body(json!({ "tag_name": CHECKOUT_PIN }));
    });
    server.mock(|when, then| {
        when.method(GET)
            .path("/repos/pnpm/action-setup/releases/latest");
        then.status(200)
            .json_body(json!({ "tag_name": PNPM_SETUP_PIN }));
    });
    server
}

/// The `.git` here is an empty directory, not a repository, so every git
/// command inside one fails. Tag-shape assertions therefore belong on
/// `git_repo`: a negative one written here would pass because the read failed,
/// not because the shape was refused.
fn temp_repo(label: &str) -> Fixture {
    let root = plain_repo("migrate", label);
    fs::create_dir(root.join(".git")).expect("fixture .git");
    root
}

fn migrate(root: &Path) -> std::process::Output {
    let server = mock_checkout_latest();
    oakum(root)
        .args(["migrate", "--yes"])
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
    migrate_on_tty_touching(root, api_url, migrate_args, answer, None)
}

/// Like [`migrate_on_tty`], writing `touch_before_answer` (a path and its
/// body) once the prompt is up and before the answer goes in, to exercise
/// the look `migrate` takes again after the prompt.
#[cfg(unix)]
fn migrate_on_tty_touching(
    root: &Path,
    api_url: &str,
    migrate_args: &[&str],
    answer: Option<&str>,
    touch_before_answer: Option<(&Path, &str)>,
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
        touch = os.environ.get("OAKUM_TEST_TOUCH_PATH")
        if touch:
            with open(touch, "w") as handle:
                handle.write(os.environ.get("OAKUM_TEST_TOUCH_BODY", ""))
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
    if let Some((path, body)) = touch_before_answer {
        command
            .env("OAKUM_TEST_TOUCH_PATH", path)
            .env("OAKUM_TEST_TOUCH_BODY", body);
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
    assert!(stdout.contains("leave `access` behind"), "{stdout}");
    assert!(stdout.contains("not carried over: `access`"), "{stdout}");
    assert!(stdout.contains("leave `changelog` behind"), "{stdout}");
    assert!(stdout.contains("rewrote .changeset/feat.md"), "{stdout}");
    assert!(stdout.contains("remaining"), "{stdout}");
    assert!(
        stdout.contains("- publish: `oakum release` only tags and creates the GitHub release"),
        "{stdout}"
    );
    assert!(
        stdout.contains("- the version PR opens on branch `oakum/version-packages`"),
        "{stdout}"
    );
    assert!(
        stdout.contains("plan comparison: 1 package(s) planned by the oakum simulation and by oakum; match (unverified: changesets did not run)"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "remove `.changeset/_schema.json`, `.changeset/README.md`, and `.changeset/_config.toml` to uninstall"
        ),
        "{stdout}"
    );
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
        .args(["migrate", "--yes"])
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
    let output = migrate_args(&root, &["--versioning", "semver", "--yes"]);
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

/// Both source paths are probed on every migration, so one of them is normally
/// absent. Reporting that absence as a fault would tell every changesets user
/// a setting may have been dropped from a file that never existed — the
/// confusion this reporting exists to prevent, inverted.
#[test]
fn a_migration_with_no_stale_source_config_says_nothing_about_one() {
    let root = temp_repo("no-stale-source-config");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/config.json"),
        r#"{"access": "public"}"#,
    )
    .expect("config");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\ncore: minor\n---\nnote\n",
    )
    .expect("bump");
    let output = migrate(&root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("could not use") && !stderr.contains("could not use"),
        "an absent source config is not an unreadable one: stdout={stdout}\nstderr={stderr}"
    );
}

/// A broken symlink reports `NotFound` exactly as an absent file does. Reading
/// that as absence is the invariant's own failure: oakum looked, could not
/// resolve it, and would have said nothing while dropping whatever the target
/// set.
#[cfg(unix)]
#[test]
fn a_dangling_symlink_at_a_source_config_is_reported_not_read_as_absent() {
    let root = temp_repo("dangling-source-config");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    std::os::unix::fs::symlink("./nope.json", root.join(".changeset/config.json"))
        .expect("dangling symlink");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\ncore: minor\n---\nnote\n",
    )
    .expect("bump");
    let output = migrate(&root);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("is a symlink whose target does not exist"),
        "names what it found rather than treating it as absent: {stderr}"
    );
}

/// Without a pin every later command refuses, so a reader who installed
/// globally would meet that refusal with the migration already applied.
#[test]
fn an_unpinned_repository_is_told_to_pin_among_the_remaining_steps() {
    let root = temp_repo("unpinned-remaining-step");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/config.json"),
        r#"{"access": "public"}"#,
    )
    .expect("config");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\ncore: minor\n---\nnote\n",
    )
    .expect("bump");
    let output = migrate(&root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("- pin the same version as `tool-version`"),
        "names the pin among the remaining steps: {stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "cargo binstall --no-confirm oakum@{BINARY_VERSION}"
        )),
        "quoting the command for the ecosystem it detected: {stdout}"
    );
    assert!(
        !stdout.contains("pnpm add -D"),
        "and not the other ecosystem's, which this repository cannot run: {stdout}"
    );
}

/// The workflow this same run prints installs through npm, so a pin step
/// quoting `cargo binstall` would contradict it two screens down (`okm-404.8`).
#[test]
fn an_npm_workspace_is_told_to_pin_with_the_npm_command() {
    let root = temp_repo("unpinned-npm-remaining-step");
    fs::write(
        root.join("package.json"),
        "{\"name\": \"demo\", \"version\": \"0.1.0\"}\n",
    )
    .expect("package.json");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/config.json"),
        r#"{"access": "public"}"#,
    )
    .expect("config");
    let output = migrate_args(&root, &["--yes"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(&format!("pnpm add -D @oakoss/oakum@{BINARY_VERSION}")),
        "quoting the npm command: {stdout}"
    );
    assert!(
        !stdout.contains("cargo binstall"),
        "and not cargo's, which this repository cannot run: {stdout}"
    );
}

/// A repository that already pins oakum is not told to pin it again.
#[test]
fn a_pinned_repository_is_not_told_to_pin_again() {
    let root = temp_repo("pinned-no-remaining-step");
    cargo_package(&root, "core", "0.1.0");
    fs::write(
        root.join(".mise.toml"),
        format!(
            "[tools]\n\"cargo:oakum\" = \"{}\"\n",
            env!("CARGO_PKG_VERSION")
        ),
    )
    .expect("mise pin");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/config.json"),
        r#"{"access": "public"}"#,
    )
    .expect("config");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\ncore: minor\n---\nnote\n",
    )
    .expect("bump");
    let output = migrate(&root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("remaining (oakum does not perform these):"),
        "the section the negative assertion depends on: {stdout}"
    );
    assert!(
        !stdout.contains("- pin the same version as `tool-version`"),
        "an existing pin needs no step: {stdout}"
    );
}

/// A pin source oakum cannot read answers "not pinned", so the step is printed.
/// The other direction would drop it from exactly the repository least able to
/// notice, and nothing else exercises the error path.
#[cfg(unix)]
#[test]
fn an_unreadable_pin_source_still_gets_the_pin_step() {
    let root = git_repo("migrate", "pin-source-unreadable");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir_all(root.join(".github")).expect("github dir");
    std::os::unix::fs::symlink("nowhere", root.join(".github/workflows")).expect("dangling");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/config.json"),
        r#"{"access": "public"}"#,
    )
    .expect("config");
    commit(&root, "seed");
    let output = migrate(&root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("- pin the same version as `tool-version`"),
        "an unreadable pin source is not a pin: {stdout}"
    );
}

/// Most of this file's tests traverse the unread arm through a fake `.git`
/// without asserting it: replacing that arm with `NoTags` left the whole suite
/// green. Collapsing "we did not look" into "never released" needs an
/// assertion of its own.
#[test]
fn a_repository_whose_tags_cannot_be_read_says_so() {
    let root = git_repo("migrate", "tags-unreadable");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/config.json"),
        r#"{"access": "public"}"#,
    )
    .expect("config");
    commit(&root, "seed");
    // A file where git expects its directory: the read fails rather than
    // finding an empty history.
    fs::remove_dir_all(root.join(".git")).expect("remove git dir");
    fs::write(root.join(".git"), "not a repository\n").expect("git file");
    let output = migrate(&root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("not derived: `tag-format` (could not read the existing tags:"),
        "names the look that failed: {stdout}"
    );
    assert!(
        !stdout.contains("carried over: `tag-format"),
        "and derives nothing from a history it never read: {stdout}"
    );
}

/// The unreadable-history test above breaks `.git` outright, so it reaches the
/// unread arm without ever running the completeness guard `read_tag_names`
/// calls first. Measured: deleting that call left all 2156 tests green. A
/// suppressed clone lists tags successfully over a set git never fetched,
/// which is the case only this guard catches. `reachable_tags.rs` owns the
/// clone-shaped fixtures; this covers migrate's wiring to the same guard.
#[test]
fn a_tag_suppressed_clone_is_not_read_as_a_complete_history() {
    let root = tagged_monorepo("tags-suppressed", &[("pr-kit", "0.1.0")]);
    support::fixture::git(
        &root,
        &["config", "--local", "remote.origin.tagOpt", "--no-tags"],
    );
    let output = migrate(&root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("not derived: `tag-format` (could not read the existing tags:"),
        "a suppressed clone is a look that failed, not an absent history: {stdout}"
    );
    assert!(
        stdout.contains("tagOpt --no-tags"),
        "and the reason names the condition: {stdout}"
    );
    let config = fs::read_to_string(config_path(&root)).expect("config");
    assert!(
        !config.contains("tag-format"),
        "nothing is derived from tags oakum could not trust: {config}"
    );
}

/// A bare shape with several tag-managed packages is the failure this whole
/// derivation exists to prevent: `release` reads such a tag as leftover
/// ambiguity, so writing it would hand the reader a config the next command
/// refuses. Measured to be reachable when the two readers of the tag-managed
/// count disagree, which is why one value feeds both.
#[test]
fn bare_tags_with_several_tag_managed_packages_write_no_config_line() {
    let root = tagged_monorepo("bare-multi-managed", &[]);
    for version in ["0.1.0", "0.2.0"] {
        let tag = format!("v{version}");
        support::fixture::git(&root, &["tag", "-a", &tag, "-m", &tag]);
    }
    let output = migrate(&root);
    let config = fs::read_to_string(config_path(&root)).expect("config");
    assert!(
        !config.contains("tag-format"),
        "a bare shape cannot name which package a tag belongs to: {config}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("names no package"),
        "and the remaining steps say why: {stdout}"
    );
}

/// Nothing else writes a config carrying both lines, so nothing else would
/// notice them colliding or swapping.
#[test]
fn a_config_carrying_both_a_tag_format_and_private_packages_writes_both() {
    let root = tagged_monorepo(
        "both-config-lines",
        &[("pr-kit", "0.1.0"), ("prose", "0.1.0")],
    );
    migrate(&root);
    let config = fs::read_to_string(config_path(&root)).expect("config");
    assert!(
        config.contains("tag-format = \"{{ package }}@{{ version }}\""),
        "the derived shape: {config}"
    );
    assert!(
        config.contains("private-packages = { version = true, tag = true }"),
        "and the carried opt-in beside it: {config}"
    );
}

/// A workspace with several tag-managed packages already tagged in oakum's own
/// default shape writes no `tag-format`: a key that restates a default is what
/// ADR-0004 keeps out. Nothing else exercises a tag-managed count above one.
#[test]
fn several_tag_managed_packages_at_the_default_shape_write_no_config_line() {
    let root = tagged_monorepo(
        "default-shape-multi",
        &[("pr-kit", "0.1.0"), ("prose", "0.1.0")],
    );
    support::fixture::git(&root, &["tag", "-d", "pr-kit@0.1.0"]);
    support::fixture::git(&root, &["tag", "-d", "prose@0.1.0"]);
    for member in ["pr-kit", "prose"] {
        let tag = format!("{member}/v0.1.0");
        support::fixture::git(&root, &["tag", "-a", &tag, "-m", &tag]);
    }
    let output = migrate(&root);
    let config = fs::read_to_string(config_path(&root)).expect("config");
    assert!(
        !config.contains("tag-format"),
        "the derived shape is the default already: {config}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("carried over: `tag-format"),
        "and nothing claims otherwise: {stdout}"
    );
}

/// Two sources contributing different axes are unioned into one written line.
/// A per-file report that quoted a whole config line would name a line neither
/// file produced, which is the report disagreeing with the write.
#[test]
fn two_sources_contributing_different_axes_report_what_each_gave() {
    let root = temp_repo("two-source-axes");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/config.json"),
        r#"{"privatePackages": {"version": true}}"#,
    )
    .expect("changesets config");
    fs::create_dir(root.join(".bumpy")).expect("dir");
    fs::write(
        root.join(".bumpy/_config.json"),
        r#"{"privatePackages": {"tag": true}}"#,
    )
    .expect("bumpy config");
    fs::write(root.join(".bumpy/feat.md"), "---\ncore: minor\n---\nnote\n").expect("bump");
    let output = migrate(&root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("carry `privatePackages.version` from `.changeset/config.json`"),
        "names what that file gave: {stdout}"
    );
    assert!(
        stdout.contains("carry `privatePackages.tag` from `.bumpy/_config.json`"),
        "names what the other gave: {stdout}"
    );
    let config = fs::read_to_string(config_path(&root)).expect("oakum config");
    assert!(
        config.contains("private-packages = { version = true, tag = true }"),
        "the write is their union: {config}"
    );
    assert!(
        stdout.contains("write `private-packages = { version = true, tag = true }`"),
        "and the report names that union once: {stdout}"
    );
}

/// Both source tools load their config through JSON5-tolerant readers, so a
/// `//` note or a trailing comma is a file they accept and `serde_json` does
/// not. A stale config from the other tool must not stop a migration it plays
/// no part in; it is reported and skipped, because a dropped setting has to be
/// visible rather than assumed absent.
#[test]
fn an_unparseable_source_config_is_reported_and_the_migration_continues() {
    let root = temp_repo("stale-source-config");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/config.json"),
        r#"{"access": "public"}"#,
    )
    .expect("config");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\n\"core\": minor\n---\nnote\n",
    )
    .expect("bump");
    fs::create_dir(root.join(".bumpy")).expect("dir");
    fs::write(
        root.join(".bumpy/_config.json"),
        "{\n  // a note someone left\n  \"baseBranch\": \"main\"\n}\n",
    )
    .expect("stale config");
    let output = migrate(&root);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("could not use `.bumpy/_config.json`"),
        "names the file it skipped: {stderr}"
    );
    assert!(
        stderr.contains("no settings carried from it"),
        "says what the skip cost: {stderr}"
    );
    // The summary copy is the record a reader scrolls back to; without this the
    // line can be deleted and every migrate test still passes.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("could not use `.bumpy/_config.json`"),
        "and the closing summary keeps it: {stdout}"
    );
    assert!(
        config_path(&root).exists(),
        "the migration still wrote its config"
    );
}

#[test]
fn bumpy_private_packages_are_carried_into_the_oakum_config() {
    let root = temp_repo("bumpy-private-packages");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".bumpy")).expect("dir");
    fs::write(
        root.join(".bumpy/_config.json"),
        r#"{"privatePackages": {"version": true, "tag": true}, "baseBranch": "main"}"#,
    )
    .expect("config");
    fs::write(
        root.join(".bumpy/feat.md"),
        "---\n\"core\": minor\n---\nnote\n",
    )
    .expect("bump");
    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);
    let config = fs::read_to_string(config_path(&root)).expect("oakum config");
    assert!(
        config.contains("\nprivate-packages = { version = true, tag = true }\n"),
        "{config}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(
            "  carry `privatePackages.version` and `privatePackages.tag` from `.bumpy/_config.json`"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains("  write `private-packages = { version = true, tag = true }`"),
        "the pending line names the line the write produces: {stdout}"
    );
    assert!(
        stdout.contains(
            "carried over: `privatePackages.version` and `privatePackages.tag` from `.bumpy/_config.json`"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains("leave `baseBranch` behind in `.bumpy/_config.json`"),
        "{stdout}"
    );
    assert!(
        stdout.contains("not carried over: `baseBranch` (`.bumpy/_config.json` is untouched)"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("`privatePackages` (not an oakum"),
        "{stdout}"
    );
}

#[test]
fn changesets_private_packages_carry_one_axis_at_a_time() {
    let root = temp_repo("changesets-private-tag");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/config.json"),
        r#"{"privatePackages": {"tag": true}}"#,
    )
    .expect("config");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\ncore: minor\n---\nnote\n",
    )
    .expect("bump");
    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);
    let config = fs::read_to_string(config_path(&root)).expect("oakum config");
    assert!(
        config.contains("\nprivate-packages = { version = false, tag = true }\n"),
        "{config}"
    );
}

#[test]
fn a_source_config_without_private_packages_writes_no_such_key() {
    let root = temp_repo("bumpy-no-private-packages");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".bumpy")).expect("dir");
    fs::write(
        root.join(".bumpy/_config.json"),
        r#"{"baseBranch": "main"}"#,
    )
    .expect("config");
    fs::write(
        root.join(".bumpy/feat.md"),
        "---\n\"core\": minor\n---\nnote\n",
    )
    .expect("bump");
    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);
    let config = fs::read_to_string(config_path(&root)).expect("oakum config");
    assert!(!config.contains("private-packages"), "{config}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("carried over: `privatePackages`"),
        "{stdout}"
    );
    assert!(
        stdout.contains("not carried over: `baseBranch` (`.bumpy/_config.json` is untouched)"),
        "{stdout}"
    );
}

#[test]
fn a_non_boolean_private_packages_axis_is_reported_and_skipped() {
    let root = temp_repo("bumpy-private-packages-bad");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".bumpy")).expect("dir");
    fs::write(
        root.join(".bumpy/_config.json"),
        r#"{"privatePackages": {"version": "yes"}}"#,
    )
    .expect("config");
    fs::write(
        root.join(".bumpy/feat.md"),
        "---\n\"core\": minor\n---\nnote\n",
    )
    .expect("bump");
    let output = migrate(&root);
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains(
            "`privatePackages.version` in `.bumpy/_config.json` is `\"yes\"`, not a boolean"
        ),
        "names the value it could not read: {err}"
    );
    assert!(
        err.contains("no settings carried from it"),
        "says what the skip cost: {err}"
    );
    assert!(
        config_path(&root).exists(),
        "an unusable source setting does not stop the migration"
    );
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
    let output = migrate_args(&root, &["--versioning", "semver", "--yes"]);
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("plan comparison: unexpected difference"),
        "{stdout}"
    );
    assert!(
        stdout.contains("core (cargo)"),
        "unexpected banner should name a package under compare: {stdout}"
    );
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
    assert!(
        !root.join(".changeset/_schema.json").exists(),
        "schema written on refusal"
    );
    assert!(
        !root.join(".changeset/README.md").exists(),
        "readme written on refusal"
    );
}

#[test]
fn non_tty_without_yes_refuses_after_the_plan_and_writes_nothing() {
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
    let before = RepoState::capture(&root);
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
        .write_all(b"y\n")
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait");
    assert!(
        !output.status.success(),
        "non-TTY without --yes must refuse"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("pending:"),
        "the plan is still shown: {stdout}"
    );
    assert!(stdout.contains("rewrite .changeset/feat.md"), "{stdout}");
    assert!(
        stderr.contains("stdin is not a terminal; rerun with --yes to apply the changes above"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("Apply these changes?"),
        "non-TTY must not prompt: {stderr}"
    );
    RepoState::assert_unchanged(&before, &root, "non-TTY refusal");
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
    assert!(
        stdout.contains("plan comparison: 1 package(s) planned by changesets and by oakum; match"),
        "{stdout}"
    );
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

#[test]
fn npm_workspace_template_provisions_pnpm_before_every_oakum_step() {
    let root = temp_repo("npm");
    fs::write(
        root.join("package.json"),
        "{\"name\": \"demo\", \"version\": \"0.1.0\"}\n",
    )
    .expect("package.json");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/config.json"),
        r#"{"changelog": "@changesets/cli/changelog", "access": "public"}"#,
    )
    .expect("config");
    let output = migrate_args(&root, &["--yes"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout
            .matches(&format!(
                "      - uses: pnpm/action-setup@{PNPM_SETUP_PIN}\n        with:\n          version: "
            ))
            .count(),
        3,
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains(
            "      - run: oakum check --strict\n        if: github.head_ref != 'oakum/version-packages'\n"
        ),
        "{stdout}"
    );
    assert!(config_path(&root).is_file());
}

#[test]
fn an_existing_readme_is_kept_and_named() {
    let root = temp_repo("keep-readme");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(root.join(".changeset/README.md"), "# changesets\n").expect("readme");
    fs::write(root.join(".changeset/config.json"), "{}").expect("config");
    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("skipped by oakum and by @changesets/cli v3 and left in place"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "write .changeset/_config.toml and write .changeset/_schema.json (keeping the existing .changeset/README.md)"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains("kept .changeset/README.md (oakum did not write it; left as is)"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "remove `.changeset/_schema.json` and `.changeset/_config.toml` to uninstall"
        ),
        "{stdout}"
    );
    assert!(
        !stdout.contains("`.changeset/README.md` to uninstall"),
        "{stdout}"
    );
    assert_eq!(
        fs::read_to_string(root.join(".changeset/README.md")).expect("readme"),
        "# changesets\n"
    );
}

#[test]
fn a_rerun_restores_missing_owned_files() {
    let root = temp_repo("restore-owned");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(root.join(".changeset/config.json"), "{}").expect("config");
    let first = migrate(&root);
    assert_migrate_unverified_kept(&first, &root);
    fs::remove_file(root.join(".changeset/README.md")).expect("rm readme");
    fs::remove_file(root.join(".changeset/_schema.json")).expect("rm schema");
    let config_before = fs::read_to_string(config_path(&root)).expect("config");
    let output = migrate(&root);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("pending:\n  write .changeset/_schema.json and .changeset/README.md"),
        "{stdout}"
    );
    assert!(
        stdout.contains("created .changeset/_schema.json"),
        "{stdout}"
    );
    assert!(stdout.contains("created .changeset/README.md"), "{stdout}");
    assert!(stdout.contains("already migrated"), "{stdout}");
    assert!(root.join(".changeset/README.md").is_file());
    assert!(root.join(".changeset/_schema.json").is_file());
    assert_eq!(
        fs::read_to_string(config_path(&root)).expect("config"),
        config_before
    );
}

#[test]
fn a_single_quoted_scoped_key_keeps_its_quotes() {
    let root = temp_repo("single-quoted");
    fs::write(
        root.join("package.json"),
        "{\"name\": \"@acme/core\", \"version\": \"0.1.0\"}\n",
    )
    .expect("package.json");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\n'@acme/core': minor\n---\nnote\n",
    )
    .expect("bump");
    fs::write(root.join(".changeset/config.json"), "{}").expect("config");
    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);
    let body = fs::read_to_string(root.join(".changeset/feat.md")).expect("bump");
    assert_eq!(body, "---\n'@acme/core': minor\n---\nnote\n");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("rewrote"), "{stdout}");
}

#[test]
fn a_readme_that_is_a_directory_refuses_before_any_write() {
    let root = temp_repo("readme-dir");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir_all(root.join(".changeset/README.md")).expect("dir");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\n\"core\": minor\n---\nnote\n",
    )
    .expect("bump");
    fs::write(root.join(".changeset/config.json"), "{}").expect("config");
    let output = migrate(&root);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("`.changeset/README.md` exists and is not a regular file"),
        "{stderr}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("pending:"), "{stdout}");
    assert_eq!(
        fs::read_to_string(root.join(".changeset/feat.md")).expect("bump"),
        "---\n\"core\": minor\n---\nnote\n"
    );
    assert!(!root.join(".changeset/_schema.json").exists());
    assert!(!config_path(&root).exists());
}

#[test]
fn a_stale_schema_is_announced_as_replaced() {
    let root = temp_repo("stale-schema");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(root.join(".changeset/_schema.json"), "{\"stale\": true}\n").expect("schema");
    fs::write(root.join(".changeset/config.json"), "{}").expect("config");
    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(
            "write .changeset/_config.toml and .changeset/README.md, and replace the existing .changeset/_schema.json"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains("replaced .changeset/_schema.json"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("created .changeset/_schema.json"),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "remove `.changeset/_schema.json`, `.changeset/README.md`, and `.changeset/_config.toml` to uninstall"
        ),
        "{stdout}"
    );
    assert!(
        !fs::read_to_string(root.join(".changeset/_schema.json"))
            .expect("schema")
            .contains("stale"),
        "schema not replaced"
    );
}

#[test]
fn oakums_own_readme_left_by_an_interrupted_run_counts_as_written() {
    let root = temp_repo("own-readme");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(root.join(".changeset/config.json"), "{}").expect("config");
    let first = migrate(&root);
    assert_migrate_unverified_kept(&first, &root);
    fs::remove_file(config_path(&root)).expect("rm config");
    fs::remove_file(root.join(".changeset/_schema.json")).expect("rm schema");
    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("kept .changeset/README.md"), "{stdout}");
    assert!(
        stdout.contains(
            "remove `.changeset/_schema.json`, `.changeset/README.md`, and `.changeset/_config.toml` to uninstall"
        ),
        "{stdout}"
    );
}

#[test]
fn a_changesets_changelog_title_is_a_remaining_step() {
    let root = temp_repo("changelog-title");
    cargo_package(&root, "core", "0.1.0");
    fs::write(
        root.join("CHANGELOG.md"),
        "# @scope/core\n\n## 0.1.0\n\n### Patch Changes\n\n- first\n",
    )
    .expect("changelog");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(root.join(".changeset/config.json"), "{}").expect("config");
    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(
            "- CHANGELOG.md does not start with `# Changelog`; oakum will not append without a recognized heading; change the first line to `# Changelog` (the old title can stay as a line under it)"
        ),
        "{stdout}"
    );
    assert_eq!(
        fs::read_to_string(root.join("CHANGELOG.md")).expect("changelog"),
        "# @scope/core\n\n## 0.1.0\n\n### Patch Changes\n\n- first\n",
        "migrate reports the title; it does not rewrite a file it did not create"
    );
}

#[test]
fn a_private_packages_changelog_title_is_not_a_remaining_step() {
    let root = temp_repo("changelog-private");
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"internal\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n\n[workspace]\n",
    )
    .expect("Cargo.toml");
    fs::create_dir_all(root.join("src")).expect("src");
    fs::write(root.join("src/lib.rs"), "").expect("lib.rs");
    fs::write(root.join("CHANGELOG.md"), "# internal\n").expect("changelog");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(root.join(".changeset/config.json"), "{}").expect("config");
    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("CHANGELOG.md does not start with"),
        "version never writes a private package's changelog by default: {stdout}"
    );
}

#[test]
fn a_stray_staging_file_is_named_before_the_plan() {
    let root = temp_repo("staging-file");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(root.join(".changeset/config.json"), "{}").expect("config");
    fs::write(
        root.join(".changeset/.feat.md.oakum-write.4242.123456.0"),
        "partial",
    )
    .expect("staging file");
    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = "`.changeset/.feat.md.oakum-write.4242.123456.0` is an oakum staging file; if no oakum run is in progress, remove it";
    let named_at = stdout.find(line).unwrap_or_else(|| panic!("{stdout}"));
    let plan_at = stdout
        .find("pending:")
        .unwrap_or_else(|| panic!("{stdout}"));
    assert!(named_at < plan_at, "named before the plan: {stdout}");
    assert!(
        root.join(".changeset/.feat.md.oakum-write.4242.123456.0")
            .is_file(),
        "migrate names the file; it does not sweep it"
    );
}

#[test]
fn a_stray_staging_file_is_reported_when_already_migrated() {
    let root = temp_repo("staging-file-again");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(root.join(".changeset/config.json"), "{}").expect("config");
    let first = migrate(&root);
    assert_migrate_unverified_kept(&first, &root);
    fs::write(
        root.join(".changeset/.feat.md.oakum-write.4242.123456.0"),
        "partial",
    )
    .expect("staging file");
    let output = migrate(&root);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("`.changeset/.feat.md.oakum-write.4242.123456.0` is an oakum staging file"),
        "{stdout}"
    );
    assert!(stdout.contains("already migrated"), "{stdout}");
}

#[test]
fn a_current_schema_is_announced_as_unchanged() {
    let root = temp_repo("current-schema");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(root.join(".changeset/config.json"), "{}").expect("config");
    let first = migrate(&root);
    assert_migrate_unverified_kept(&first, &root);
    fs::remove_file(root.join(".changeset/_config.toml")).expect("drop config");
    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("unchanged .changeset/_schema.json"),
        "a byte-identical schema is not a replacement: {stdout}"
    );
    assert!(
        !stdout.contains("replaced .changeset/_schema.json"),
        "{stdout}"
    );
}

#[cfg(unix)]
#[test]
fn a_readme_that_appears_during_the_prompt_is_reported_and_kept() {
    let root = temp_repo("tty-readme-appears");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\n\"core\": minor\n---\nnote\n",
    )
    .expect("bump");
    fs::write(root.join(".changeset/config.json"), "{}").expect("config");
    let server = mock_checkout_latest();
    let readme = root.join(".changeset/README.md");
    let output = migrate_on_tty_touching(
        &root,
        &server.base_url(),
        &[],
        Some("y\n"),
        Some((&readme, "user readme\n")),
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains(
            "changed while waiting:\r\n  write .changeset/_config.toml and write .changeset/_schema.json (keeping the existing .changeset/README.md)"
        ) || combined.contains(
            "changed while waiting:\n  write .changeset/_config.toml and write .changeset/_schema.json (keeping the existing .changeset/README.md)"
        ),
        "the second look is reported: {combined}"
    );
    assert!(
        combined.contains("kept .changeset/README.md (oakum did not write it; left as is)"),
        "{combined}"
    );
    assert!(
        combined.contains(
            "remove `.changeset/_schema.json` and `.changeset/_config.toml` to uninstall"
        ),
        "a README that appeared as the user's is not listed: {combined}"
    );
    assert_eq!(
        fs::read_to_string(&readme).expect("readme"),
        "user readme\n",
        "left as is"
    );
}

#[cfg(unix)]
#[test]
fn a_readme_that_stops_being_oakums_during_the_prompt_is_reported_and_kept() {
    let root = temp_repo("tty-readme-edited");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\n\"core\": minor\n---\nnote\n",
    )
    .expect("bump");
    fs::write(root.join(".changeset/config.json"), "{}").expect("config");
    let readme = root.join(".changeset/README.md");
    let bundled = include_str!("../src/cli/changeset-readme.md");
    fs::write(&readme, bundled).expect("oakum's own readme");
    let edited = format!("{bundled}\nMy notes.\n");
    let server = mock_checkout_latest();
    let output = migrate_on_tty_touching(
        &root,
        &server.base_url(),
        &[],
        Some("y\n"),
        Some((&readme, &edited)),
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains(".changeset/README.md changed; it is left as is"),
        "the same sentence with a different owner is named: {combined}"
    );
    assert!(
        combined.contains(
            "remove `.changeset/_schema.json` and `.changeset/_config.toml` to uninstall"
        ),
        "a README that stopped being oakum's is not listed: {combined}"
    );
    assert_eq!(
        fs::read_to_string(&readme).expect("readme"),
        edited,
        "left as is"
    );
}

#[test]
fn a_schema_that_is_a_directory_refuses_before_any_write() {
    let root = temp_repo("schema-dir");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir_all(root.join(".changeset/_schema.json")).expect("dir");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\n\"core\": minor\n---\nnote\n",
    )
    .expect("bump");
    fs::write(root.join(".changeset/config.json"), "{}").expect("config");
    let output = migrate(&root);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("`.changeset/_schema.json` exists and is not a regular file"),
        "{stderr}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("pending:"), "{stdout}");
    assert_eq!(
        fs::read_to_string(root.join(".changeset/feat.md")).expect("bump"),
        "---\n\"core\": minor\n---\nnote\n",
        "refused before the bump files were rewritten"
    );
    assert!(!config_path(&root).exists());
}

#[cfg(unix)]
#[test]
fn a_schema_that_is_a_symlink_refuses_before_any_write() {
    let root = temp_repo("schema-symlink");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(root.join(".changeset/real-schema.json"), "{}\n").expect("target");
    std::os::unix::fs::symlink("real-schema.json", root.join(".changeset/_schema.json"))
        .expect("symlink");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\n\"core\": minor\n---\nnote\n",
    )
    .expect("bump");
    fs::write(root.join(".changeset/config.json"), "{}").expect("config");
    let output = migrate(&root);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("`.changeset/_schema.json` is a symlink"),
        "{stderr}"
    );
    assert_eq!(
        fs::read_to_string(root.join(".changeset/feat.md")).expect("bump"),
        "---\n\"core\": minor\n---\nnote\n"
    );
    assert!(!config_path(&root).exists());
}

/// A three-member all-private workspace tagged the way a changesets or bumpy
/// monorepo tags: `<name>@<version>`.
fn tagged_monorepo(label: &str, tags: &[(&str, &str)]) -> Fixture {
    const MEMBERS: [(&str, &str); 3] = [
        ("pr-kit", "0.1.0"),
        ("prose", "0.1.0"),
        ("review-cycle", "0.17.0"),
    ];
    let root = git_repo("migrate", label);
    private_workspace(&root, &MEMBERS);
    fs::create_dir(root.join(".changeset")).expect("dir");
    // `privatePackages` on: the members are all unpublishable, so without it
    // none is tag-managed and the count a bare shape turns on is zero.
    fs::write(
        root.join(".changeset/config.json"),
        r#"{"access": "public", "privatePackages": {"version": true, "tag": true}}"#,
    )
    .expect("config");
    commit(&root, "seed");
    tag_members_at_version(&root, tags);
    root
}

/// The tags were readable while `migrate` ran, so the config it writes renders
/// them. Without this the mismatch against oakum's default surfaces at the
/// first `release`, the last step of a cutover (`okm-404.19`).
#[test]
fn existing_tags_settle_the_written_tag_format() {
    let root = tagged_monorepo(
        "derived-tag-format",
        &[
            ("pr-kit", "0.1.0"),
            ("prose", "0.1.0"),
            ("review-cycle", "0.17.0"),
        ],
    );
    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);

    let config = fs::read_to_string(config_path(&root)).expect("config");
    assert!(
        config.contains("tag-format = \"{{ package }}@{{ version }}\"\n"),
        "{config}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(
            "  carry the existing tag shape as `tag-format = \"{{ package }}@{{ version }}\"`"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "carried over: `tag-format = \"{{ package }}@{{ version }}\"` (derived from the existing tags)"
        ),
        "{stdout}"
    );
}

/// Two shapes in one history derive nothing. The refusal `release` already
/// carries is the right outcome, and silence is not: the run says why the key
/// is unset.
#[test]
fn tags_that_disagree_leave_tag_format_unset() {
    let root = tagged_monorepo("undecided-tag-format", &[("pr-kit", "0.1.0")]);
    support::fixture::git(&root, &["tag", "-a", "prose/v0.1.0", "-m", "prose/v0.1.0"]);
    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);

    let config = fs::read_to_string(config_path(&root)).expect("config");
    assert!(!config.contains("tag-format"), "{config}");
    // Tags oakum read and could not explain are an action the reader owes, so
    // the line sits among the remaining steps rather than in the summary.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(
            "- set `tag-format` to match the existing tags (`pr-kit@0.1.0` and `prose/v0.1.0` are not the same shape)"
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains("`release` refuses at the first tag"),
        "and says what happens if they do not: {stdout}"
    );
    // Matched whole, newline to newline, so a menu that loses its line break or
    // changes length fails here. Bare is absent because the fixture has three
    // tag-managed packages, the rule that refused these tags in the first place.
    assert!(
        stdout.contains(
            "rather than writing a shape the repository does not use\n  oakum reads `{{ package }}@{{ version }}`, `{{ package }}/v{{ version }}`, `{{ package }}-v{{ version }}`\n"
        ),
        "names the shapes this repository could adopt, on its own line: {stdout}"
    );
}

/// The default for a repository with one tag-managed package is bare, and the
/// tags already render it. A config line that restates the default is noise.
#[test]
fn a_shape_that_matches_the_default_writes_no_config_line() {
    let root = git_repo("migrate", "default-tag-format");
    cargo_package(&root, "core", "0.1.0");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/config.json"),
        r#"{"access": "public"}"#,
    )
    .expect("config");
    commit(&root, "seed");
    support::fixture::git(&root, &["tag", "-a", "v0.1.0", "-m", "v0.1.0"]);

    let output = migrate(&root);
    assert_migrate_unverified_kept(&output, &root);
    let config = fs::read_to_string(config_path(&root)).expect("config");
    assert!(!config.contains("tag-format"), "{config}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("tag-format"), "{stdout}");
}
