//! `oakum check` is the shared readiness path (ADR-0020).

#![allow(clippy::disallowed_methods)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oakum"))
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
        .join(format!("oakum-check-{label}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp repo");
    git(&dir, &["init", "-b", "main"]);
    git(&dir, &["config", "user.email", "test@example.com"]);
    git(&dir, &["config", "user.name", "Test"]);
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
    git(root, &["commit", "--no-verify", "-m", message]);
}

fn oakum_args(root: &Path, args: &[&str]) -> (bool, String, String) {
    let out = bin().args(args).current_dir(root).output().expect("oakum");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn oakum(root: &Path, command: &str) -> (bool, String, String) {
    oakum_args(root, &[command])
}

fn check(root: &Path) -> (bool, String, String) {
    oakum(root, "check")
}

fn write_config(root: &Path, body: &str) {
    fs::create_dir_all(root.join(".changeset")).expect("changeset dir");
    fs::write(root.join(".changeset/_config.toml"), body).expect("config");
}

fn write_install_pin(root: &Path, version: &str) {
    fs::create_dir_all(root.join(".github/workflows")).expect("workflows");
    fs::write(
        root.join(".github/workflows/release.yml"),
        format!("run: cargo binstall --no-confirm oakum@{version}\n"),
    )
    .expect("workflow");
}

fn write_pinned_config(root: &Path, version: &str, extra: &str) {
    write_config(root, &format!("tool-version = \"{version}\"\n{extra}"));
    write_install_pin(root, version);
}

#[test]
fn matching_manifest_is_clean() {
    let root = temp_git_repo("match");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn never_released_is_clean() {
    let root = temp_git_repo("bootstrap");
    cargo_package(&root, "demo", "0.0.0");
    commit(&root, "init");
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn manifest_above_tag_is_drift() {
    let root = temp_git_repo("above");
    cargo_package(&root, "demo", "0.2.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "expected drift");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("0.2.0"), "{stderr}");
    assert!(stderr.contains("0.1.0"), "{stderr}");
    assert!(
        !stderr.contains("never released"),
        "tagged-ahead must not also look untagged: {stderr}"
    );
}

#[test]
fn shallow_clone_is_unverified() {
    let src = temp_git_repo("shallow-src");
    cargo_package(&src, "demo", "0.1.0");
    commit(&src, "init");
    git(&src, &["tag", "v0.1.0"]);
    fs::write(src.join("src/lib.rs"), "// two\n").expect("second commit");
    commit(&src, "two");
    let dest = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("oakum-check-shallow-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dest);
    let status = Command::new("git")
        .args([
            "-c",
            "protocol.file.allow=always",
            "clone",
            "--depth=1",
            "--no-local",
            src.to_str().expect("utf-8 path"),
            dest.to_str().expect("utf-8 dest"),
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .status()
        .expect("git clone");
    assert!(status.success(), "git clone --depth=1 failed");
    let (ok, stdout, stderr) = check(&dest);
    assert!(!ok, "shallow clone must not look like never-released");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("shallow"), "{stderr}");
}

#[test]
fn leftover_tag_is_unverified() {
    let root = temp_git_repo("leftover");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "other-v1.0.0"]);
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "leftover must not look clean");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("other-v1.0.0"), "{stderr}");
}

#[test]
fn untagged_manifest_above_0_1_0_is_not_bootstrap() {
    let root = temp_git_repo("clobber");
    cargo_package(&root, "demo", "0.2.0");
    commit(&root, "init");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "untagged 0.2.0 must not look never-released");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("0.2.0"), "{stderr}");
    assert!(stderr.contains("never released"), "{stderr}");
    assert!(stderr.contains("1 package"), "{stderr}");
    let drift_run = oakum(&root, "tag-drift");
    assert_eq!(ok, drift_run.0);
    assert_eq!(stderr, drift_run.2);
}

#[test]
fn tool_version_mismatch_fails() {
    let root = temp_git_repo("pin");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "tool-version = \"9.9.9\"\n");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "mismatch must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("tool-version"), "{stderr}");
    assert!(stderr.contains("upgrade"), "{stderr}");
}

#[test]
fn both_intent_mechanisms_off_fails() {
    let root = temp_git_repo("no-intent");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(
        &root,
        "0.0.0",
        "change-files = false\nconventional-commits = false\n",
    );
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "both mechanisms off must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("change-files"), "{stderr}");
    assert!(stderr.contains("conventional-commits"), "{stderr}");
    let drift_run = oakum(&root, "tag-drift");
    assert_eq!(ok, drift_run.0);
    assert_eq!(stderr, drift_run.2);
}

#[test]
fn one_intent_mechanism_on_is_ready() {
    let root = temp_git_repo("commits-only");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(
        &root,
        "0.0.0",
        "change-files = false\nconventional-commits = true\n",
    );
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
    let drift_run = oakum(&root, "tag-drift");
    assert_eq!(ok, drift_run.0);
    assert_eq!(stderr, drift_run.2);

    write_pinned_config(
        &root,
        "0.0.0",
        "change-files = true\nconventional-commits = false\n",
    );
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
    let drift_run = oakum(&root, "tag-drift");
    assert_eq!(ok, drift_run.0);
    assert_eq!(stderr, drift_run.2);
}

#[test]
fn check_and_tag_drift_share_the_tool_version_gate() {
    let root = temp_git_repo("share");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "tool-version = \"9.9.9\"\n");
    let check_run = check(&root);
    let drift_run = oakum(&root, "tag-drift");
    assert_eq!(check_run.0, drift_run.0);
    assert_eq!(check_run.2, drift_run.2);
}

#[test]
fn matching_install_pin_is_ready() {
    let root = temp_git_repo("pin-match");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, "0.0.0", "");
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn missing_install_pin_is_unverified() {
    let root = temp_git_repo("pin-missing");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "tool-version = \"0.0.0\"\n");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a missing pin must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("no oakum install pin"), "{stderr}");
}

#[test]
fn mismatched_install_pin_is_unverified() {
    let root = temp_git_repo("pin-mismatch");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "tool-version = \"0.0.0\"\n");
    write_install_pin(&root, "1.2.3");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a mismatched pin must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("1.2.3"), "{stderr}");
    assert!(stderr.contains("0.0.0"), "{stderr}");
}

#[test]
fn a_matching_pin_does_not_forgive_a_mismatch() {
    let root = temp_git_repo("pin-conflict");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, "0.0.0", "");
    fs::write(
        root.join(".github/workflows/ci.yml"),
        "run: cargo binstall --no-confirm oakum@1.2.3\n",
    )
    .expect("ci workflow");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a second mismatched pin must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("1.2.3"), "{stderr}");
}

#[test]
fn check_only_workflow_is_not_a_pin() {
    let root = temp_git_repo("pin-check-only");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "tool-version = \"0.0.0\"\n");
    fs::create_dir_all(root.join(".github/workflows")).expect("workflows");
    fs::write(root.join(".github/workflows/ci.yml"), "run: oakum check\n").expect("ci workflow");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a check-only workflow must not look pinned");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("no oakum install pin"), "{stderr}");
}

fn write_package_json_pin(root: &Path, version: &str) {
    fs::write(
        root.join("package.json"),
        format!(r#"{{"name":"demo","version":"0.1.0","devDependencies":{{"oakum":"{version}"}}}}"#),
    )
    .expect("package.json");
}

#[test]
fn matching_package_json_pin_without_workflow_is_ready() {
    let root = temp_git_repo("pin-npm");
    write_package_json_pin(&root, "0.0.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "tool-version = \"0.0.0\"\n");
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn matching_workflow_does_not_hide_a_mismatched_package_json_pin() {
    let root = temp_git_repo("pin-npm-mismatch");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, "0.0.0", "");
    write_package_json_pin(&root, "1.2.3");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a mismatched package.json pin must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("1.2.3"), "{stderr}");
}

fn write_mise_pin(root: &Path, version: &str) {
    fs::write(
        root.join(".mise.toml"),
        format!("[tools]\noakum = \"{version}\"\n"),
    )
    .expect(".mise.toml");
}

#[test]
fn matching_mise_pin_without_workflow_is_ready() {
    let root = temp_git_repo("pin-mise");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "tool-version = \"0.0.0\"\n");
    write_mise_pin(&root, "0.0.0");
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn matching_workflow_does_not_hide_an_inexact_mise_pin() {
    let root = temp_git_repo("pin-mise-latest");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, "0.0.0", "");
    write_mise_pin(&root, "latest");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "an inexact mise pin must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("latest"), "{stderr}");
}

#[test]
fn matching_workflow_does_not_hide_an_inexact_package_json_pin() {
    let root = temp_git_repo("pin-npm-latest");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, "0.0.0", "");
    write_package_json_pin(&root, "latest");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "an inexact package.json pin must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("latest"), "{stderr}");
}

#[test]
fn matching_undotted_mise_toml_pin_is_ready() {
    let root = temp_git_repo("pin-mise-toml");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "tool-version = \"0.0.0\"\n");
    fs::write(root.join("mise.toml"), "[tools]\noakum = \"0.0.0\"\n").expect("mise.toml");
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn matching_workflow_does_not_hide_a_mismatched_mise_pin() {
    let root = temp_git_repo("pin-mise-mismatch");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, "0.0.0", "");
    write_mise_pin(&root, "1.2.3");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a mismatched mise pin must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("1.2.3"), "{stderr}");
}

#[test]
fn yaml_extension_workflow_pin_is_ready() {
    let root = temp_git_repo("pin-yaml");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "tool-version = \"0.0.0\"\n");
    fs::create_dir_all(root.join(".github/workflows")).expect("workflows");
    fs::write(
        root.join(".github/workflows/release.yaml"),
        "run: cargo binstall --no-confirm oakum@0.0.0\n",
    )
    .expect("yaml workflow");
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

fn repo_with_followup_change(label: &str) -> PathBuf {
    let root = temp_git_repo(label);
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, "0.0.0", "");
    fs::write(root.join("src/lib.rs"), "// changed\n").expect("edit");
    commit(&root, "chore: touch demo");
    root
}

#[test]
fn default_check_reports_uncovered_without_failing() {
    let root = repo_with_followup_change("cover-advisory");
    let (ok, stdout, stderr) = oakum_args(&root, &["check", "--from", "HEAD~1"]);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("demo"), "{stderr}");
    assert!(stderr.contains("no covering intent"), "{stderr}");
}

#[test]
fn strict_fails_when_a_changed_package_has_no_bump_file() {
    let root = repo_with_followup_change("cover-strict-miss");
    let (ok, stdout, stderr) = oakum_args(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert!(!ok, "strict must fail when a changed package is uncovered");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("demo"), "{stderr}");
    assert!(stderr.contains("no covering intent"), "{stderr}");
}

#[test]
fn strict_passes_when_a_bump_file_names_the_package() {
    let root = repo_with_followup_change("cover-strict-hit");
    fs::write(
        root.join(".changeset/cover.md"),
        "---\ndemo: patch\n---\n\ncover\n",
    )
    .expect("bump file");
    let (ok, stdout, stderr) = oakum_args(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn strict_empty_frontmatter_covers_changed_packages() {
    let root = repo_with_followup_change("cover-empty");
    fs::write(
        root.join(".changeset/empty.md"),
        "---\n---\n\nintentional none\n",
    )
    .expect("empty bump");
    let (ok, stdout, stderr) = oakum_args(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn strict_none_entry_covers_the_named_package() {
    let root = repo_with_followup_change("cover-none");
    fs::write(
        root.join(".changeset/none.md"),
        "---\ndemo: none\n---\n\ncovered without a release\n",
    )
    .expect("none bump");
    let (ok, stdout, stderr) = oakum_args(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn tag_drift_skips_coverage() {
    let root = repo_with_followup_change("cover-tag-drift");
    let (ok, stdout, stderr) = oakum(&root, "tag-drift");
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn strict_commits_only_intent_covers_without_a_bump_file() {
    let root = temp_git_repo("cover-commits-only");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(
        &root,
        "0.0.0",
        "change-files = false\nconventional-commits = true\n",
    );
    fs::write(root.join("src/lib.rs"), "// changed\n").expect("edit");
    commit(&root, "feat(demo): add thing");
    let (ok, stdout, stderr) = oakum_args(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert!(
        ok,
        "commits-only --strict must treat feat(demo) as covering, got: {stderr}"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn strict_ignores_commits_when_change_files_are_on() {
    let root = temp_git_repo("cover-files-on-feat");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, "0.0.0", "");
    fs::write(root.join("src/lib.rs"), "// changed\n").expect("edit");
    commit(&root, "feat(demo): add thing");
    let (ok, stdout, stderr) = oakum_args(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert!(
        !ok,
        "feat(demo) must not cover when change files are on, got: {stderr}"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("demo"), "{stderr}");
    assert!(stderr.contains("no covering intent"), "{stderr}");
    assert!(stderr.contains("bump file"), "{stderr}");
}

fn git_clone(origin: &Path, dest: &Path) {
    let status = Command::new("git")
        .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
        .args([
            "clone",
            origin.to_str().expect("utf8 origin"),
            dest.to_str().expect("utf8 dest"),
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .status()
        .expect("git clone");
    assert!(status.success(), "git clone failed");
    let hooks = dest.join("no-hooks");
    fs::create_dir_all(&hooks).expect("no-hooks");
    git(dest, &["config", "core.hooksPath", "no-hooks"]);
}

fn clone_of(origin: &Path, label: &str) -> PathBuf {
    let dest = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("oakum-check-{label}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dest);
    git_clone(origin, &dest);
    dest
}

fn tagged_cargo(label: &str, versions: &[&str]) -> PathBuf {
    let root = temp_git_repo(label);
    cargo_package(&root, "demo", versions[0]);
    commit(&root, "init");
    git(
        &root,
        &["tag", "-a", &format!("v{}", versions[0]), "-m", versions[0]],
    );
    for version in &versions[1..] {
        cargo_package(&root, "demo", version);
        commit(&root, version);
        git(&root, &["tag", "-a", &format!("v{version}"), "-m", version]);
    }
    root
}

#[test]
fn remote_off_by_default_when_local_tags_are_missing_from_the_remote() {
    let origin = tagged_cargo("remote-default-origin", &["0.1.0"]);
    let dest = clone_of(&origin, "remote-default-dest");
    git(&origin, &["tag", "-d", "v0.1.0"]);
    let (ok, stdout, stderr) = check(&dest);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
    let (ok, stdout, stderr) = oakum_args(&dest, &["check", "--remote"]);
    assert!(!ok, "missing remote tag must be unverified, got: {stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("v0.1.0"), "{stderr}");
}

#[test]
fn remote_reports_unverified_when_advertised_tags_are_missing_locally() {
    let origin = tagged_cargo("remote-missing-origin", &["0.1.0"]);
    let dest = clone_of(&origin, "remote-missing-dest");
    git(&dest, &["tag", "-d", "v0.1.0"]);
    let (ok, stdout, stderr) = oakum_args(&dest, &["check", "--remote"]);
    assert!(
        !ok,
        "missing advertised tags must be unverified, got: {stdout}"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("git fetch --tags"), "{stderr}");
}

#[test]
fn remote_ok_when_advertised_tags_match_local() {
    let origin = tagged_cargo("remote-match-origin", &["0.1.0"]);
    let dest = clone_of(&origin, "remote-match-dest");
    let (ok, stdout, stderr) = oakum_args(&dest, &["check", "--remote"]);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn remote_fails_closed_without_a_remote() {
    let root = tagged_cargo("remote-none", &["0.1.0"]);
    let (ok, stdout, stderr) = oakum_args(&root, &["check", "--remote"]);
    assert!(!ok, "no remotes must be unverified, got: {stdout}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("no remotes"), "{stderr}");
}

#[test]
fn remote_lookback_ignores_older_missing_tags() {
    let origin = tagged_cargo(
        "remote-lookback-origin",
        &["0.1.0", "0.2.0", "0.3.0", "0.4.0"],
    );
    git(&origin, &["tag", "-d", "v0.1.0"]);
    let dest = clone_of(&origin, "remote-lookback-dest");
    git(&dest, &["tag", "v0.1.0", "HEAD~3"]);
    let (ok, stdout, stderr) = oakum_args(&dest, &["check", "--remote"]);
    assert!(
        ok,
        "default lookback of 3 should skip v0.1.0, got: {stderr}"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
    let (ok, stdout, stderr) = oakum_args(&dest, &["check", "--remote", "--remote-lookback", "4"]);
    assert!(!ok, "lookback 4 must see missing v0.1.0, got: {stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("v0.1.0"), "{stderr}");
    assert!(stderr.contains("git push --tags"), "{stderr}");
}

#[test]
fn remote_lookback_orders_by_semver_not_lexically() {
    let origin = tagged_cargo(
        "remote-semver-origin",
        &["0.9.0", "0.10.0", "0.11.0", "0.12.0"],
    );
    git(&origin, &["tag", "-d", "v0.9.0"]);
    let dest = clone_of(&origin, "remote-semver-dest");
    git(&dest, &["tag", "v0.9.0", "HEAD~3"]);
    let (ok, stdout, stderr) = oakum_args(&dest, &["check", "--remote"]);
    assert!(ok, "semver lookback of 3 should skip v0.9.0, got: {stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn remote_lookback_zero_is_rejected_by_clap() {
    let origin = tagged_cargo("remote-lookback-zero-origin", &["0.1.0"]);
    let dest = clone_of(&origin, "remote-lookback-zero-dest");
    let (ok, stdout, stderr) = oakum_args(&dest, &["check", "--remote", "--remote-lookback", "0"]);
    assert!(!ok, "lookback 0 must be a clap error, got: {stdout}");
    assert!(
        !stderr.contains("unverified"),
        "range rejection is not a verification outcome: {stderr}"
    );
}

#[test]
fn remote_lookback_without_remote_is_rejected_by_clap() {
    let origin = tagged_cargo("remote-lookback-requires-origin", &["0.1.0"]);
    let dest = clone_of(&origin, "remote-lookback-requires-dest");
    let (ok, stdout, stderr) = oakum_args(&dest, &["check", "--remote-lookback", "4"]);
    assert!(
        !ok,
        "--remote-lookback without --remote must be clap, got: {stdout}"
    );
    assert!(
        !stderr.contains("unverified"),
        "missing --remote is not a verification outcome: {stderr}"
    );
}

#[test]
fn remote_ls_remote_failure_is_unverified_when_local_tags_are_empty() {
    let origin = tagged_cargo("remote-ls-fail-origin", &["0.1.0"]);
    let dest = clone_of(&origin, "remote-ls-fail-dest");
    git(&dest, &["tag", "-d", "v0.1.0"]);
    git(
        &dest,
        &["remote", "set-url", "origin", "/no/such/oakum-remote"],
    );
    let (ok, stdout, stderr) = oakum_args(&dest, &["check", "--remote"]);
    assert!(!ok, "failed ls-remote must be unverified, got: {stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("ls-remote"), "{stderr}");
}

#[test]
fn remote_ls_remote_failure_is_unverified_when_local_tags_exist() {
    let origin = tagged_cargo("remote-ls-fail-tagged-origin", &["0.1.0"]);
    let dest = clone_of(&origin, "remote-ls-fail-tagged-dest");
    git(
        &dest,
        &["remote", "set-url", "origin", "/no/such/oakum-remote"],
    );
    let (ok, stdout, stderr) = oakum_args(&dest, &["check", "--remote"]);
    assert!(!ok, "failed ls-remote must be unverified, got: {stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("ls-remote"), "{stderr}");
    assert!(
        !stderr.contains("git push --tags"),
        "must not name push when the look failed: {stderr}"
    );
}
