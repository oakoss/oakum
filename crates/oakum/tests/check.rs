//! `oakum check` is the shared readiness path (ADR-0020).

#![allow(clippy::disallowed_methods)]

mod support;

use std::fs;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Command;

use httpmock::prelude::*;
use support::fixture::{
    cargo_package, commit, git, git_repo, oakum, oakum_output, sibling, Fixture,
};
#[cfg(unix)]
use support::fixture::{install_executable, recording_fake_ssh};

const BINARY_VERSION: &str = env!("CARGO_PKG_VERSION");
/// A pin that can never equal the binary's version, so a "mismatch" fixture
/// stays a mismatch at every release. Measured: literal `1.2.3` collided with
/// the floating `tool-version` once the binary reached 1.2.3.
const MISMATCHED_PIN: &str = "99999.0.0";

fn temp_git_repo(label: &str) -> Fixture {
    git_repo("check", label)
}

/// Sibling paths must stay in the container so Drop reclaims them.
#[cfg(unix)]
fn assert_sibling_in_container(root: &Fixture, path: &Path) {
    assert!(
        path.starts_with(root.container()),
        "{} must stay under the fixture container {}",
        path.display(),
        root.container().display()
    );
}

/// A config whose `tool-version` always matches the binary under test. The
/// install-pin fixtures are compared against the config's own `tool-version`,
/// so both sides must move with the binary together.
fn versioned(rest: &str) -> String {
    format!("tool-version = \"{BINARY_VERSION}\"\n{rest}")
}

/// A package a registry would refuse, which is what makes `private-packages`
/// decide whether oakum manages it.
fn private_npm_package(root: &Path, name: &str, version: &str) {
    std::fs::write(
        root.join("package.json"),
        format!(
            "{{\n  \"name\": \"{name}\",\n  \"version\": \"{version}\",\n  \"private\": true\n}}\n"
        ),
    )
    .expect("package.json");
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

fn write_workflow_pin(root: &Path, name: &str, body: &str) {
    fs::create_dir_all(root.join(".github/workflows")).expect("workflows");
    fs::write(root.join(".github/workflows").join(name), body).expect("workflow");
}

fn write_pinned_config(root: &Path, version: &str, extra: &str) {
    write_config(root, &format!("tool-version = \"{version}\"\n{extra}"));
    write_install_pin(root, version);
}

fn check(root: &Path) -> (bool, String, String) {
    oakum_output(root, &["check"])
}

#[test]
fn matching_manifest_is_clean() {
    let root = temp_git_repo("match");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

/// A workspace whose only package is private, with `private-packages` left at
/// its default, version-manages nothing. Every plan is then empty and a release
/// does nothing while reporting success, so `check` says so instead of passing.
/// Measured shape: a repository migrated from a tool that had opted private
/// packages in, where the setting was not carried.
#[test]
fn a_config_that_manages_nothing_is_unverified_not_clean() {
    let root = temp_git_repo("manages-nothing");
    write_pinned_config(&root, BINARY_VERSION, "");
    private_npm_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a config that manages nothing must not pass: {stdout}");
    assert!(
        stderr.contains("manages no package on either axis"),
        "names the condition: {stderr}"
    );
    assert!(
        stderr.contains("private-packages.version = true"),
        "names the fix: {stderr}"
    );
}

/// The management look reports without preempting the others: an unfinished
/// write means a file may be stale, and that diagnosis may not be hidden
/// behind a config-level one.
#[test]
fn a_config_that_manages_nothing_does_not_hide_the_other_looks() {
    let root = temp_git_repo("manages-nothing-and-pin");
    write_pinned_config(&root, BINARY_VERSION, "");
    private_npm_package(&root, "demo", "0.1.0");
    fs::write(
        root.join(".changeset/.foo.oakum-write.123.456.0"),
        "partial",
    )
    .expect("staging file");
    commit(&root, "init");
    let (ok, _stdout, stderr) = check(&root);
    assert!(!ok, "expected a refusal");
    assert!(
        stderr.contains("every selected package is private"),
        "reports the management fault: {stderr}"
    );
    assert!(
        stderr.contains("oakum staging file"),
        "and still names the unfinished write: {stderr}"
    );
}

/// A stale install pin is the more fundamental diagnosis, since a different
/// oakum may not have the other looks at all — but it must not be the only one
/// a run reports. Both faults live behind different early returns, so this pins
/// the pair the management test cannot reach.
#[test]
fn a_stale_install_pin_does_not_hide_an_unfinished_write() {
    let root = temp_git_repo("pin-and-staging");
    write_config(&root, &format!("tool-version = \"{BINARY_VERSION}\"\n"));
    write_install_pin(&root, "99999.0.0");
    cargo_package(&root, "demo", "0.1.0");
    fs::write(root.join(".changeset/.x.oakum-write.1.2.0"), "partial").expect("staging file");
    commit(&root, "init");
    let (ok, _stdout, stderr) = check(&root);
    assert!(!ok, "expected a refusal");
    assert!(
        stderr.contains("install pin is 99999.0.0"),
        "names the stale pin: {stderr}"
    );
    assert!(
        stderr.contains("oakum staging file"),
        "and still names the unfinished write: {stderr}"
    );
}

/// The pin look lives one level down, inside the tag evaluation, so binding
/// the looks in `run` alone left it hiding the coverage refusal.
#[test]
fn a_stale_install_pin_does_not_hide_an_uncovered_change() {
    let root = temp_git_repo("pin-and-coverage");
    write_config(&root, &format!("tool-version = \"{BINARY_VERSION}\"\n"));
    write_install_pin(&root, "99999.0.0");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    fs::write(root.join("src/lib.rs"), "// changed\n").expect("edit");
    commit(&root, "chore: touch demo");
    let (ok, _stdout, stderr) = oakum_output(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert!(!ok, "expected a refusal");
    assert!(
        stderr.contains("install pin is 99999.0.0"),
        "names the stale pin: {stderr}"
    );
    assert!(
        stderr.contains("changed with no covering intent"),
        "and still names the uncovered change: {stderr}"
    );
}

/// A version-only opt-in is managed. Without this, dropping the version axis
/// from the predicate leaves the whole suite green: the both-axes test is
/// carried by `tag_managed` alone.
#[test]
fn opting_only_the_version_axis_in_clears_the_management_refusal() {
    let root = temp_git_repo("manages-version-only");
    write_pinned_config(
        &root,
        BINARY_VERSION,
        "private-packages = { version = true, tag = false }\n",
    );
    private_npm_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    let (ok, _stdout, stderr) = check(&root);
    assert!(ok, "a version-only opt-in is managed: {stderr}");
}

/// One managed member is enough. Every other fixture reaching this gate holds a
/// single package, where `any` and `all` agree — so without a mixed workspace,
/// a predicate that demanded every member be managed would refuse the ordinary
/// monorepo shape (public packages beside a private tooling crate) and the
/// suite would not notice.
#[test]
fn a_mixed_workspace_passes_when_one_member_is_publishable() {
    let root = temp_git_repo("manages-mixed-workspace");
    write_pinned_config(&root, BINARY_VERSION, "");
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
        .expect("member Cargo.toml");
        fs::write(path.join("src/lib.rs"), "").expect("lib.rs");
    }
    commit(&root, "init");
    let (ok, _stdout, stderr) = check(&root);
    assert!(ok, "one managed member is enough: {stderr}");
}

/// The stated-exclusion contract, named rather than covered by accident. Two
/// unrelated tests happen to use fixtures whose selection is empty, so a
/// fixture change in either would remove this coverage silently.
#[test]
fn a_selection_emptied_by_exclude_stays_silent() {
    let root = temp_git_repo("manages-excluded-silent");
    write_pinned_config(&root, BINARY_VERSION, "exclude = [\"demo\"]\n");
    private_npm_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    let (ok, stdout, stderr) = check(&root);
    assert!(
        ok,
        "an exclusion is a decision oakum does not argue with: {stderr}"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

/// A workspace with one publishable member is every existing user's shape, and
/// nothing else asserts the gate stays silent for it.
#[test]
fn a_workspace_with_one_publishable_member_passes_the_management_look() {
    let root = temp_git_repo("manages-mixed");
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

/// Tagging alone is work, and ADR-0027 makes the axes independent, so a
/// repository that only tags its private packages is not the silent no-op the
/// refusal exists to catch.
#[test]
fn opting_only_the_tag_axis_in_clears_the_management_refusal() {
    let root = temp_git_repo("manages-tag-only");
    write_pinned_config(
        &root,
        BINARY_VERSION,
        "private-packages = { version = false, tag = true }\n",
    );
    private_npm_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    let (ok, _stdout, stderr) = check(&root);
    assert!(ok, "a tag-only opt-in is managed: {stderr}");
}

/// The same workspace with the packages opted in is clean again, so the refusal
/// tracks the config rather than the manifest being private.
#[test]
fn opting_private_packages_in_clears_the_management_refusal() {
    let root = temp_git_repo("manages-private");
    write_pinned_config(
        &root,
        BINARY_VERSION,
        "private-packages = { version = true, tag = true }\n",
    );
    private_npm_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
}

#[test]
fn never_released_is_clean() {
    let root = temp_git_repo("bootstrap");
    write_pinned_config(&root, BINARY_VERSION, "");
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
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.2.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "expected drift");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("0.2.0"), "{stderr}");
    assert!(stderr.contains("0.1.0"), "{stderr}");
    assert!(
        stderr.contains("(local tags; run `git fetch --tags` if the remote is ahead)"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("never released"),
        "tagged-ahead must not also look untagged: {stderr}"
    );
}

#[test]
fn shallow_clone_is_unverified() {
    let src = temp_git_repo("shallow-src");
    write_pinned_config(&src, BINARY_VERSION, "");
    cargo_package(&src, "demo", "0.1.0");
    commit(&src, "init");
    git(&src, &["tag", "v0.1.0"]);
    fs::write(src.join("src/lib.rs"), "// two\n").expect("second commit");
    commit(&src, "two");
    let dest = sibling(&src, "shallow");
    git(
        &src,
        &[
            "-c",
            "protocol.file.allow=always",
            "clone",
            "--depth=1",
            "--no-local",
            src.to_str().expect("utf-8 path"),
            dest.to_str().expect("utf-8 dest"),
        ],
    );
    let (ok, stdout, stderr) = check(&dest);
    assert!(!ok, "shallow clone must not look like never-released");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("shallow"), "{stderr}");
}

#[test]
fn leftover_tag_is_unverified() {
    let root = temp_git_repo("leftover");
    write_pinned_config(&root, BINARY_VERSION, "");
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
    write_pinned_config(&root, BINARY_VERSION, "");
    cargo_package(&root, "demo", "0.2.0");
    commit(&root, "init");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "untagged 0.2.0 must not look never-released");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("0.2.0"), "{stderr}");
    assert!(stderr.contains("never released"), "{stderr}");
    assert!(stderr.contains("1 package"), "{stderr}");
    let drift_run = oakum_output(&root, &["tag-drift"]);
    assert_eq!(ok, drift_run.0);
    assert_eq!(stderr, drift_run.2);
}

#[test]
fn tool_version_mismatch_does_not_block_check() {
    let root = temp_git_repo("pin");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "tool-version = \"9.9.9\"\n");
    write_install_pin(&root, "9.9.9");
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(!stderr.contains("upgrade"), "{stderr}");
}

#[test]
fn both_intent_mechanisms_off_fails() {
    let root = temp_git_repo("no-intent");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(
        &root,
        BINARY_VERSION,
        "change-files = false\nconventional-commits = false\n",
    );
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "both mechanisms off must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("change-files"), "{stderr}");
    assert!(stderr.contains("conventional-commits"), "{stderr}");
    let drift_run = oakum_output(&root, &["tag-drift"]);
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
        BINARY_VERSION,
        "change-files = false\nconventional-commits = true\n",
    );
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
    let drift_run = oakum_output(&root, &["tag-drift"]);
    assert_eq!(ok, drift_run.0);
    assert_eq!(stderr, drift_run.2);

    write_pinned_config(
        &root,
        BINARY_VERSION,
        "change-files = true\nconventional-commits = false\n",
    );
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
    let drift_run = oakum_output(&root, &["tag-drift"]);
    assert_eq!(ok, drift_run.0);
    assert_eq!(stderr, drift_run.2);
}

#[test]
fn check_and_tag_drift_share_tag_evaluation() {
    let root = temp_git_repo("share");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "tool-version = \"9.9.9\"\n");
    write_install_pin(&root, "9.9.9");
    let check_run = check(&root);
    let drift_run = oakum_output(&root, &["tag-drift"]);
    assert!(check_run.0, "{}", check_run.2);
    assert_eq!(check_run.0, drift_run.0);
    assert_eq!(check_run.2, drift_run.2);
    assert!(!check_run.2.contains("upgrade"), "{}", check_run.2);
}

#[test]
fn mismatched_tool_version_still_runs_tag_drift() {
    let root = temp_git_repo("drift-toolver");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, "tool-version = \"9.9.9\"\n");
    write_install_pin(&root, "9.9.9");
    let (ok, _stdout, stderr) = oakum_output(&root, &["tag-drift"]);
    assert!(ok, "{stderr}");
    assert!(!stderr.contains("upgrade"), "{stderr}");
}

#[test]
fn matching_install_pin_is_ready() {
    let root = temp_git_repo("pin-match");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn matching_binstall_version_flag_pin_is_ready() {
    let root = temp_git_repo("pin-binstall-ver");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, &versioned(""));
    write_workflow_pin(
        &root,
        "ci.yml",
        &format!("run: cargo binstall oakum --version {BINARY_VERSION}\n"),
    );
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn mismatched_binstall_version_flag_pin_is_unverified() {
    let root = temp_git_repo("pin-binstall-ver-mismatch");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, &versioned(""));
    write_workflow_pin(
        &root,
        "ci.yml",
        &format!("run: cargo binstall oakum --version {MISMATCHED_PIN}\n"),
    );
    let (ok, stdout, stderr) = check(&root);
    assert!(
        !ok,
        "a mismatched binstall --version pin must not look ready"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains(MISMATCHED_PIN), "{stderr}");
}

#[test]
fn matching_cargo_install_oakum_at_pin_is_ready() {
    let root = temp_git_repo("pin-cargo-at");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, &versioned(""));
    write_workflow_pin(
        &root,
        "ci.yml",
        &format!("run: cargo install oakum@{BINARY_VERSION}\n"),
    );
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn mismatched_cargo_install_oakum_at_pin_is_unverified() {
    let root = temp_git_repo("pin-cargo-at-mismatch");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, &versioned(""));
    write_workflow_pin(
        &root,
        "ci.yml",
        &format!("run: cargo install oakum@{MISMATCHED_PIN}\n"),
    );
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a mismatched cargo install@ pin must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains(MISMATCHED_PIN), "{stderr}");
}

#[test]
fn matching_cargo_install_version_flag_pin_is_ready() {
    let root = temp_git_repo("pin-cargo-ver");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, &versioned(""));
    write_workflow_pin(
        &root,
        "ci.yml",
        &format!("run: cargo install oakum --version {BINARY_VERSION}\n"),
    );
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn mismatched_cargo_install_version_flag_pin_is_unverified() {
    let root = temp_git_repo("pin-cargo-ver-mismatch");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, &versioned(""));
    write_workflow_pin(
        &root,
        "ci.yml",
        &format!("run: cargo install oakum --version {MISMATCHED_PIN}\n"),
    );
    let (ok, stdout, stderr) = check(&root);
    assert!(
        !ok,
        "a mismatched cargo install --version pin must not look ready"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains(MISMATCHED_PIN), "{stderr}");
}

#[test]
fn matching_install_action_tool_pin_is_ready() {
    let root = temp_git_repo("pin-install-action");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, &versioned(""));
    write_workflow_pin(
        &root,
        "ci.yml",
        &format!("- uses: oakoss/install-action@v1\n  with:\n    tool: oakum@{BINARY_VERSION}\n"),
    );
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn unversioned_install_action_tool_pin_is_unverified() {
    let root = temp_git_repo("pin-install-action-unversioned");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, &versioned(""));
    write_workflow_pin(
        &root,
        "ci.yml",
        "- uses: oakoss/install-action@v1\n  with:\n    tool: oakum\n",
    );
    let (ok, stdout, stderr) = check(&root);
    assert!(
        !ok,
        "an unversioned install-action tool pin must not look ready"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("without a version"), "{stderr}");
    assert!(
        !stderr.contains("no oakum install pin"),
        "unversioned install-action tool must not read as a missing pin: {stderr}"
    );
    assert!(
        stderr.contains(".github/workflows/ci.yml"),
        "unversioned pin must name the workflow: {stderr}"
    );
}

#[test]
fn unversioned_binstall_oakum_pin_is_unverified() {
    let root = temp_git_repo("pin-binstall-unversioned");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, &versioned(""));
    write_workflow_pin(&root, "ci.yml", "run: cargo binstall oakum\n");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "an unversioned binstall oakum pin must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("without a version"), "{stderr}");
    assert!(
        !stderr.contains("no oakum install pin"),
        "unversioned binstall must not read as a missing pin: {stderr}"
    );
}

#[test]
fn unversioned_cargo_install_oakum_pin_is_unverified() {
    let root = temp_git_repo("pin-cargo-install-unversioned");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, &versioned(""));
    write_workflow_pin(&root, "ci.yml", "run: cargo install oakum\n");
    let (ok, stdout, stderr) = check(&root);
    assert!(
        !ok,
        "an unversioned cargo install oakum pin must not look ready"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("without a version"), "{stderr}");
    assert!(
        !stderr.contains("no oakum install pin"),
        "unversioned cargo install must not read as a missing pin: {stderr}"
    );
}

#[test]
fn mismatched_install_action_tool_pin_is_unverified() {
    let root = temp_git_repo("pin-install-action-mismatch");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, &versioned(""));
    write_workflow_pin(
        &root,
        "ci.yml",
        &format!("- uses: oakoss/install-action@v1\n  with:\n    tool: oakum@{MISMATCHED_PIN}\n"),
    );
    let (ok, stdout, stderr) = check(&root);
    assert!(
        !ok,
        "a mismatched install-action tool pin must not look ready"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains(MISMATCHED_PIN), "{stderr}");
}

#[test]
fn missing_install_pin_is_unverified() {
    let root = temp_git_repo("pin-missing");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, &versioned(""));
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a missing pin must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("no oakum install pin"), "{stderr}");
    assert!(
        !stderr.contains("without a version"),
        "a missing pin must not read as unversioned: {stderr}"
    );
}

#[test]
fn matching_workspace_oakum_member_is_ready() {
    let root = temp_git_repo("pin-ws-oakum");
    write_workspace_oakum_member(&root, BINARY_VERSION);
    commit(&root, "init");
    git(&root, &["tag", &format!("v{BINARY_VERSION}")]);
    write_config(&root, &versioned(""));
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn mismatched_workspace_oakum_member_is_unverified() {
    let root = temp_git_repo("pin-ws-mismatch");
    write_workspace_oakum_member(&root, MISMATCHED_PIN);
    commit(&root, "init");
    write_config(&root, &versioned(""));
    let (ok, stdout, stderr) = check(&root);
    assert!(
        !ok,
        "a mismatched workspace oakum version must not look ready"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains(MISMATCHED_PIN), "{stderr}");
    assert!(stderr.contains(BINARY_VERSION), "{stderr}");
}

#[test]
fn mismatched_install_pin_is_unverified() {
    let root = temp_git_repo("pin-mismatch");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, &versioned(""));
    write_install_pin(&root, MISMATCHED_PIN);
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a mismatched pin must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains(MISMATCHED_PIN), "{stderr}");
    assert!(stderr.contains(BINARY_VERSION), "{stderr}");
}

#[test]
fn a_matching_pin_does_not_forgive_a_mismatch() {
    let root = temp_git_repo("pin-conflict");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::write(
        root.join(".github/workflows/ci.yml"),
        format!("run: cargo binstall --no-confirm oakum@{MISMATCHED_PIN}\n"),
    )
    .expect("ci workflow");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a second mismatched pin must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains(MISMATCHED_PIN), "{stderr}");
}

#[test]
fn a_matching_binstall_at_pin_does_not_forgive_mismatched_version_flag() {
    let root = temp_git_repo("pin-binstall-ver-conflict");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    write_workflow_pin(
        &root,
        "ci.yml",
        &format!("run: cargo binstall oakum --version {MISMATCHED_PIN}\n"),
    );
    let (ok, stdout, stderr) = check(&root);
    assert!(
        !ok,
        "a mismatched binstall --version pin must not be forgiven by a matching binstall@ pin"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains(MISMATCHED_PIN), "{stderr}");
}

#[test]
fn a_matching_binstall_at_pin_does_not_forgive_mismatched_cargo_install_version_flag() {
    let root = temp_git_repo("pin-cargo-ver-conflict");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    write_workflow_pin(
        &root,
        "ci.yml",
        &format!("run: cargo install oakum --version {MISMATCHED_PIN}\n"),
    );
    let (ok, stdout, stderr) = check(&root);
    assert!(
        !ok,
        "a mismatched cargo install --version pin must not be forgiven by a matching binstall@ pin"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains(MISMATCHED_PIN), "{stderr}");
}

#[test]
fn a_matching_binstall_at_pin_does_not_forgive_mismatched_cargo_install_at() {
    let root = temp_git_repo("pin-cargo-at-conflict");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    write_workflow_pin(
        &root,
        "ci.yml",
        &format!("run: cargo install oakum@{MISMATCHED_PIN}\n"),
    );
    let (ok, stdout, stderr) = check(&root);
    assert!(
        !ok,
        "a mismatched cargo install@ pin must not be forgiven by a matching binstall@ pin"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains(MISMATCHED_PIN), "{stderr}");
}

#[test]
fn a_matching_binstall_at_pin_does_not_forgive_mismatched_install_action_tool() {
    let root = temp_git_repo("pin-install-action-conflict");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    write_workflow_pin(
        &root,
        "ci.yml",
        &format!("- uses: oakoss/install-action@v1\n  with:\n    tool: oakum@{MISMATCHED_PIN}\n"),
    );
    let (ok, stdout, stderr) = check(&root);
    assert!(
        !ok,
        "a mismatched install-action tool pin must not be forgiven by a matching binstall@ pin"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains(MISMATCHED_PIN), "{stderr}");
}

#[test]
fn check_only_workflow_is_not_a_pin() {
    let root = temp_git_repo("pin-check-only");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, &versioned(""));
    fs::create_dir_all(root.join(".github/workflows")).expect("workflows");
    fs::write(root.join(".github/workflows/ci.yml"), "run: oakum check\n").expect("ci workflow");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a check-only workflow must not look pinned");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("no oakum install pin"), "{stderr}");
    assert!(
        !stderr.contains("without a version"),
        "check-only workflow must not read as unversioned: {stderr}"
    );
}

fn write_package_json_pin(root: &Path, version: &str) {
    fs::write(
        root.join("package.json"),
        format!(r#"{{"name":"demo","version":"0.1.0","devDependencies":{{"oakum":"{version}"}}}}"#),
    )
    .expect("package.json");
}

fn write_scoped_package_json_pin(root: &Path, spec: &str) {
    fs::write(
        root.join("package.json"),
        format!(
            r#"{{"name":"demo","version":"0.1.0","devDependencies":{{"@oakoss/oakum":"{spec}"}}}}"#
        ),
    )
    .expect("package.json");
}

#[test]
fn matching_scoped_package_json_pin_is_ready() {
    let root = temp_git_repo("pin-npm-scoped");
    write_scoped_package_json_pin(&root, BINARY_VERSION);
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, &versioned(""));
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn mismatched_scoped_package_json_pin_is_unverified() {
    let root = temp_git_repo("pin-npm-scoped-mismatch");
    write_scoped_package_json_pin(&root, MISMATCHED_PIN);
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, &versioned(""));
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a mismatched @oakoss/oakum pin must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains(MISMATCHED_PIN), "{stderr}");
    assert!(stderr.contains("package.json"), "{stderr}");
}

#[test]
fn a_range_on_the_scoped_package_is_unverified() {
    let root = temp_git_repo("pin-npm-scoped-range");
    write_scoped_package_json_pin(&root, "^0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, &versioned(""));
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a range is not a pin");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains("pins `@oakoss/oakum` as `^0.1.0`, which is not an exact version"),
        "{stderr}"
    );
}

#[test]
fn a_pnpm_add_workflow_line_is_a_pin() {
    let root = temp_git_repo("pin-pnpm-add");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, &versioned(""));
    fs::create_dir_all(root.join(".github/workflows")).expect("workflows");
    fs::write(
        root.join(".github/workflows/release.yml"),
        format!("run: pnpm add -D @oakoss/oakum@{BINARY_VERSION}\n"),
    )
    .expect("workflow");
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn a_mismatched_npx_workflow_line_is_unverified() {
    let root = temp_git_repo("pin-npx-mismatch");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, &versioned(""));
    fs::create_dir_all(root.join(".github/workflows")).expect("workflows");
    fs::write(
        root.join(".github/workflows/release.yml"),
        format!("run: npx @oakoss/oakum@{MISMATCHED_PIN} check --strict\n"),
    )
    .expect("workflow");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a mismatched npx pin must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains(MISMATCHED_PIN), "{stderr}");
}

#[test]
fn an_unversioned_npm_install_is_unverified() {
    let root = temp_git_repo("pin-npm-unversioned");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, &versioned(""));
    fs::create_dir_all(root.join(".github/workflows")).expect("workflows");
    fs::write(
        root.join(".github/workflows/release.yml"),
        "run: npm i -g @oakoss/oakum\n",
    )
    .expect("workflow");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a bare npm install must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("without a version"), "{stderr}");
    assert!(
        !stderr.contains("no oakum install pin"),
        "an unversioned install must not read as no install: {stderr}"
    );
}

#[test]
fn a_pnpm_exec_invocation_is_not_a_pin() {
    let root = temp_git_repo("pin-pnpm-exec");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, &versioned(""));
    fs::create_dir_all(root.join(".github/workflows")).expect("workflows");
    fs::write(
        root.join(".github/workflows/ci.yml"),
        "run: pnpm exec oakum check --strict\n",
    )
    .expect("workflow");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "an invocation is not a pin");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("no oakum install pin"), "{stderr}");
}

#[test]
fn a_mise_npm_backend_pin_is_ready() {
    let root = temp_git_repo("pin-mise-npm");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, &versioned(""));
    fs::write(
        root.join(".mise.toml"),
        format!("[tools]\n\"npm:@oakoss/oakum\" = \"{BINARY_VERSION}\"\n"),
    )
    .expect(".mise.toml");
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn a_changesets_changelog_title_is_unverified_and_names_the_fix() {
    let root = temp_git_repo("changelog-foreign-title");
    cargo_package(&root, "demo", "0.1.0");
    fs::write(
        root.join("CHANGELOG.md"),
        "# demo\n\n## 0.1.0\n\n### Patch Changes\n\n- first\n",
    )
    .expect("changelog");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a title version would refuse must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains(
            "CHANGELOG.md does not start with `# Changelog`; oakum will not append without a recognized heading; change the first line to `# Changelog` (the old title can stay as a line under it)\n"
        ),
        "{stderr}"
    );
    assert!(
        stderr.contains(
            "error: unverified: 1 changelog(s) `oakum version` would refuse to append to"
        ),
        "{stderr}"
    );
}

#[test]
fn a_changelog_titled_changelog_is_ready() {
    let root = temp_git_repo("changelog-ok-title");
    cargo_package(&root, "demo", "0.1.0");
    fs::write(
        root.join("CHANGELOG.md"),
        "# Changelog\n\ndemo\n\n## 0.1.0 (2026-09-01)\n\n### Added\n\n- first\n",
    )
    .expect("changelog");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn an_unmanaged_packages_changelog_title_is_not_checked() {
    let root = temp_git_repo("changelog-unmanaged");
    cargo_package(&root, "demo", "0.1.0");
    fs::write(root.join("CHANGELOG.md"), "# demo\n").expect("changelog");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "exclude = [\"demo\"]\n");
    let (ok, stdout, stderr) = check(&root);
    assert!(
        ok,
        "an excluded package never gets a version write: {stderr}"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        !stderr.contains("CHANGELOG.md"),
        "an excluded package's changelog is not oakum's concern: {stderr}"
    );
}

fn two_member_workspace(root: &Path, titles: [&str; 2]) {
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"alpha\", \"beta\"]\n",
    )
    .expect("workspace");
    for (name, title) in ["alpha", "beta"].into_iter().zip(titles) {
        let path = root.join(name);
        fs::create_dir_all(path.join("src")).expect("src");
        fs::write(
            path.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )
        .expect("member Cargo.toml");
        fs::write(path.join("src/lib.rs"), "").expect("lib.rs");
        fs::write(
            path.join("CHANGELOG.md"),
            format!("{title}\n\n## 0.1.0\n\n- first\n"),
        )
        .expect("changelog");
    }
}

#[test]
fn every_foreign_member_changelog_is_named_in_one_refusal() {
    let root = temp_git_repo("changelog-members");
    two_member_workspace(&root, ["# @scope/alpha", "# @scope/beta"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    commit(&root, "init");
    git(&root, &["tag", "alpha/v0.1.0"]);
    git(&root, &["tag", "beta/v0.1.0"]);
    let (ok, stdout, stderr) = check(&root);
    assert!(
        !ok,
        "member changelogs are checked where they live: {stderr}"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains("alpha/CHANGELOG.md does not start with `# Changelog`"),
        "{stderr}"
    );
    assert!(
        stderr.contains("beta/CHANGELOG.md does not start with `# Changelog`"),
        "{stderr}"
    );
    assert!(
        stderr.contains("unverified: 2 changelog(s) `oakum version` would refuse to append to"),
        "both members ride one refusal: {stderr}"
    );
}

#[test]
fn a_foreign_member_changelog_is_found_beside_a_clean_root() {
    let root = temp_git_repo("changelog-one-member");
    two_member_workspace(&root, ["# Changelog", "# @scope/beta"]);
    fs::write(root.join("CHANGELOG.md"), "# Changelog\n").expect("root changelog");
    write_pinned_config(&root, BINARY_VERSION, "");
    commit(&root, "init");
    git(&root, &["tag", "alpha/v0.1.0"]);
    git(&root, &["tag", "beta/v0.1.0"]);
    let (ok, _, stderr) = check(&root);
    assert!(!ok, "{stderr}");
    assert!(
        stderr.contains("beta/CHANGELOG.md does not start with"),
        "{stderr}"
    );
    assert!(stderr.contains("unverified: 1 changelog(s)"), "{stderr}");
    assert!(!stderr.contains("alpha/CHANGELOG.md"), "{stderr}");
}

#[test]
fn a_bom_changelog_names_its_own_fix() {
    let root = temp_git_repo("changelog-bom");
    cargo_package(&root, "demo", "0.1.0");
    fs::write(root.join("CHANGELOG.md"), "\u{FEFF}# Changelog\n").expect("changelog");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains(
            "CHANGELOG.md starts with a UTF-8 BOM; oakum will not splice a changelog it cannot recognize; remove the byte-order mark from the start of the file"
        ),
        "{stderr}"
    );
    assert!(stderr.contains("unverified: 1 changelog(s)"), "{stderr}");
    assert!(
        !stderr.contains("change the first line"),
        "the title fix does not apply to a BOM: {stderr}"
    );
}

#[test]
fn tag_drift_is_still_reported_beside_a_foreign_changelog() {
    let root = temp_git_repo("changelog-and-drift");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    cargo_package(&root, "demo", "0.2.0");
    fs::write(root.join("CHANGELOG.md"), "# demo\n").expect("changelog");
    commit(&root, "bump without tag");
    write_pinned_config(&root, BINARY_VERSION, "");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains("manifest 0.2.0 is above tagged 0.1.0"),
        "drift is printed even when the changelog also refuses: {stderr}"
    );
    assert!(
        stderr.contains("CHANGELOG.md does not start with `# Changelog`"),
        "{stderr}"
    );
    assert!(stderr.contains("unverified: 1 changelog(s)"), "{stderr}");
}

#[test]
fn a_stray_staging_file_is_unverified_and_named() {
    let root = temp_git_repo("staging-file");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::write(
        root.join(".changeset/.one.md.oakum-write.4242.123456.0"),
        "---\ndemo: patch\n---\n",
    )
    .expect("staging file");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a leftover staging file is not a clean tree: {stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains(
            "`.changeset/.one.md.oakum-write.4242.123456.0` is an oakum staging file; if no oakum run is in progress, remove it"
        ),
        "{stderr}"
    );
    assert!(
        stderr.contains("unverified: 1 oakum staging file(s) left behind"),
        "{stderr}"
    );
    assert!(
        root.join(".changeset/.one.md.oakum-write.4242.123456.0")
            .is_file(),
        "check names the file; it does not sweep it"
    );
}

#[test]
fn every_staging_file_is_named_in_order_with_a_count() {
    let root = temp_git_repo("staging-two");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    for name in [
        ".zeta.md.oakum-write.1.2.0",
        "._config.toml.oakum-write.1.2.0",
    ] {
        fs::write(root.join(".changeset").join(name), "partial").expect("staging");
    }
    let (ok, _, stderr) = check(&root);
    assert!(!ok, "{stderr}");
    let config_at = stderr
        .find("._config.toml.oakum-write")
        .expect("config line");
    let zeta_at = stderr.find(".zeta.md.oakum-write").expect("zeta line");
    assert!(config_at < zeta_at, "sorted by name: {stderr}");
    assert!(
        stderr.contains("unverified: 2 oakum staging file(s) left behind"),
        "{stderr}"
    );
}

#[test]
fn a_staging_file_beside_a_manifest_is_unverified_too() {
    let root = temp_git_repo("staging-manifest");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::write(
        root.join(".Cargo.toml.oakum-write.4242.123456.0"),
        "partial",
    )
    .expect("staging");
    let (ok, _, stderr) = check(&root);
    assert!(!ok, "{stderr}");
    assert!(
        stderr.contains("`.Cargo.toml.oakum-write.4242.123456.0` is an oakum staging file"),
        "version stages beside the manifest, so check looks there: {stderr}"
    );
}

#[test]
fn a_staging_file_beside_a_declared_extra_file_is_unverified_too() {
    let root = temp_git_repo("staging-extra-file");
    cargo_package(&root, "demo", "0.1.0");
    fs::create_dir_all(root.join("docs")).expect("docs");
    fs::write(root.join("docs/app.json"), "{\"version\": \"0.1.0\"}\n").expect("extra file");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(
        &root,
        BINARY_VERSION,
        "[[packages.demo.extra-files]]\npath = \"/docs/app.json\"\nformat = \"json\"\nkey = \"version\"\n",
    );
    fs::write(
        root.join("docs/.app.json.oakum-write.4242.123456.0"),
        "partial",
    )
    .expect("staging");
    let (ok, _, stderr) = check(&root);
    assert!(!ok, "{stderr}");
    assert!(
        stderr.contains("`docs/.app.json.oakum-write.4242.123456.0` is an oakum staging file"),
        "version stages beside a declared extra file, so check looks there: {stderr}"
    );
}

#[test]
fn a_directory_named_like_a_staging_file_is_not_oakums() {
    let root = temp_git_repo("staging-dir");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::create_dir(root.join(".changeset/.one.md.oakum-write.4242.123456.0")).expect("dir");
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "the writer only creates regular files: {stderr}");
    assert!(stdout.is_empty() && stderr.is_empty(), "{stdout}{stderr}");
}

#[test]
fn a_staging_file_is_still_named_beside_a_foreign_changelog() {
    let root = temp_git_repo("staging-and-changelog");
    cargo_package(&root, "demo", "0.1.0");
    fs::write(root.join("CHANGELOG.md"), "# demo\n").expect("changelog");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::write(
        root.join(".changeset/.one.md.oakum-write.4242.123456.0"),
        "partial",
    )
    .expect("staging file");
    let (ok, _, stderr) = check(&root);
    assert!(!ok, "{stderr}");
    assert!(
        stderr.contains("CHANGELOG.md does not start with `# Changelog`"),
        "{stderr}"
    );
    assert!(
        stderr.contains("`.changeset/.one.md.oakum-write.4242.123456.0` is an oakum staging file"),
        "one unverified state does not hide another: {stderr}"
    );
}

#[test]
fn matching_package_json_pin_without_workflow_is_ready() {
    let root = temp_git_repo("pin-npm");
    write_package_json_pin(&root, BINARY_VERSION);
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, &versioned(""));
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
    write_pinned_config(&root, BINARY_VERSION, "");
    write_package_json_pin(&root, MISMATCHED_PIN);
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a mismatched package.json pin must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains(MISMATCHED_PIN), "{stderr}");
}

fn write_mise_pin(root: &Path, version: &str) {
    fs::write(
        root.join(".mise.toml"),
        format!("[tools]\noakum = \"{version}\"\n"),
    )
    .expect(".mise.toml");
}

fn write_workspace_oakum_member(root: &Path, version: &str) {
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/oakum\"]\n",
    )
    .expect("workspace Cargo.toml");
    fs::create_dir_all(root.join("crates/oakum/src")).expect("oakum src");
    fs::write(
        root.join("crates/oakum/Cargo.toml"),
        format!("[package]\nname = \"oakum\"\nversion = \"{version}\"\nedition = \"2021\"\n"),
    )
    .expect("oakum Cargo.toml");
    fs::write(root.join("crates/oakum/src/lib.rs"), "").expect("lib.rs");
}

#[test]
fn matching_mise_pin_without_workflow_is_ready() {
    let root = temp_git_repo("pin-mise");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, &versioned(""));
    write_mise_pin(&root, BINARY_VERSION);
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
    write_pinned_config(&root, BINARY_VERSION, "");
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
    write_pinned_config(&root, BINARY_VERSION, "");
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
    write_config(&root, &versioned(""));
    fs::write(
        root.join("mise.toml"),
        format!("[tools]\noakum = \"{BINARY_VERSION}\"\n"),
    )
    .expect("mise.toml");
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
    write_pinned_config(&root, BINARY_VERSION, "");
    write_mise_pin(&root, MISMATCHED_PIN);
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a mismatched mise pin must not look ready");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains(MISMATCHED_PIN), "{stderr}");
}

#[test]
fn yaml_extension_workflow_pin_is_ready() {
    let root = temp_git_repo("pin-yaml");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_config(&root, &versioned(""));
    fs::create_dir_all(root.join(".github/workflows")).expect("workflows");
    fs::write(
        root.join(".github/workflows/release.yaml"),
        format!("run: cargo binstall --no-confirm oakum@{BINARY_VERSION}\n"),
    )
    .expect("yaml workflow");
    let (ok, stdout, stderr) = check(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

fn repo_with_followup_change(label: &str) -> Fixture {
    let root = temp_git_repo(label);
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::write(root.join("src/lib.rs"), "// changed\n").expect("edit");
    commit(&root, "chore: touch demo");
    root
}

#[test]
fn default_check_reports_uncovered_without_failing() {
    let root = repo_with_followup_change("cover-advisory");
    let (ok, stdout, stderr) = oakum_output(&root, &["check", "--from", "HEAD~1"]);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("demo"), "{stderr}");
    assert!(stderr.contains("no covering intent"), "{stderr}");
}

#[test]
fn a_malformed_bump_file_fails_check_and_is_named() {
    let root = temp_git_repo("broken-md");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    write_pinned_config(
        &root,
        BINARY_VERSION,
        "change-files = true\nconventional-commits = false\n",
    );
    fs::write(root.join(".changeset/broken.md"), "not a bump file\n").expect("broken");
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "a malformed bump file must not pass: {stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains("`broken.md` is not a bump file: bump file must start with --- on line 1"),
        "{stderr}"
    );
}

#[test]
fn no_config_is_unverified_and_names_the_commands_that_write_one() {
    let root = temp_git_repo("no-config");
    cargo_package(&root, "demo", "0.1.0");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    let (ok, stdout, stderr) = check(&root);
    assert!(!ok, "defaults must not pass as verified: {stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains(
            "unverified: `.changeset/_config.toml` not found; run `oakum init` or `oakum migrate`"
        ),
        "{stderr}"
    );
}

#[test]
fn strict_fails_when_a_changed_package_has_no_bump_file() {
    let root = repo_with_followup_change("cover-strict-miss");
    let (ok, stdout, stderr) = oakum_output(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert!(!ok, "strict must fail when a changed package is uncovered");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("demo"), "{stderr}");
    assert!(stderr.contains("no covering intent"), "{stderr}");
    assert!(
        stderr.contains("add a bump file (or `none` / empty frontmatter under --strict)"),
        "{stderr}"
    );
}

#[test]
fn strict_passes_when_a_bump_file_names_the_package() {
    let root = repo_with_followup_change("cover-strict-hit");
    fs::write(
        root.join(".changeset/cover.md"),
        "---\ndemo: patch\n---\n\ncover\n",
    )
    .expect("bump file");
    let (ok, stdout, stderr) = oakum_output(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

/// Nested unmanaged packages keep longest-prefix ownership; filtering the
/// managed set before attribution would steal those paths onto the parent.
#[test]
fn coverage_does_not_attribute_excluded_nested_paths_to_the_parent() {
    let root = temp_git_repo("cover-nested-exclude");
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"parent\", \"parent/nested\"]\n",
    )
    .expect("workspace");
    for (dir, name) in [("parent", "parent"), ("parent/nested", "nested")] {
        let path = root.join(dir);
        fs::create_dir_all(path.join("src")).expect("src");
        fs::write(
            path.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )
        .expect("member Cargo.toml");
        fs::write(path.join("src/lib.rs"), "").expect("lib.rs");
    }
    write_pinned_config(&root, BINARY_VERSION, "exclude = [\"nested\"]\n");
    commit(&root, "init");
    git(&root, &["tag", "parent/v0.1.0"]);
    fs::write(root.join("parent/nested/src/lib.rs"), "// nested only\n").expect("edit nested");
    commit(&root, "chore: touch nested");
    let (ok, stdout, stderr) = oakum_output(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert!(
        ok,
        "excluded nested change must not uncover parent: {stderr}"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn coverage_ignores_changes_in_an_excluded_package() {
    let root = temp_git_repo("cover-exclude-package");
    cargo_package(&root, "demo", "0.1.0");
    write_pinned_config(&root, BINARY_VERSION, "exclude = [\"demo\"]\n");
    commit(&root, "init");
    git(&root, &["tag", "v0.1.0"]);
    fs::write(root.join("src/lib.rs"), "// changed\n").expect("edit");
    commit(&root, "chore: touch demo");
    let (ok, stdout, stderr) = oakum_output(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert!(ok, "excluded package must not need coverage: {stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

/// `--from <base>` diffs `base...HEAD` — from the merge-base — so a base
/// branch that advanced after the branch point cannot pull its own packages
/// into the branch's coverage. A two-dot diff would (measured mutant).
#[test]
fn coverage_ignores_paths_the_base_changed_after_the_branch_point() {
    let root = temp_git_repo("cover-three-dot");
    let listed = "\"alpha\", \"beta\"";
    fs::write(
        root.join("Cargo.toml"),
        format!("[workspace]\nresolver = \"2\"\nmembers = [{listed}]\n"),
    )
    .expect("workspace");
    for name in ["alpha", "beta"] {
        let dir = root.join(name);
        fs::create_dir_all(dir.join("src")).expect("src");
        fs::write(
            dir.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )
        .expect("member Cargo.toml");
        fs::write(dir.join("src/lib.rs"), "").expect("lib.rs");
    }
    write_pinned_config(&root, BINARY_VERSION, "");
    commit(&root, "init");

    git(&root, &["switch", "-c", "feature"]);
    fs::write(root.join("alpha/src/lib.rs"), "// branch\n").expect("edit alpha");
    fs::write(
        root.join(".changeset/branch.md"),
        "---\nalpha: patch\n---\n\nbranch work\n",
    )
    .expect("bump file");
    commit(&root, "chore: branch work");

    git(&root, &["switch", "main"]);
    fs::write(root.join("beta/src/lib.rs"), "// base advanced\n").expect("edit beta");
    commit(&root, "chore: base advance");
    git(&root, &["switch", "feature"]);

    let (ok, stdout, stderr) = oakum_output(&root, &["check", "--strict", "--from", "main"]);
    assert!(
        ok,
        "beta changed only on the advanced base and must not need coverage: {stderr}"
    );
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
    let (ok, stdout, stderr) = oakum_output(&root, &["check", "--strict", "--from", "HEAD~1"]);
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
    let (ok, stdout, stderr) = oakum_output(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn tag_drift_skips_coverage() {
    let root = repo_with_followup_change("cover-tag-drift");
    let (ok, stdout, stderr) = oakum_output(&root, &["tag-drift"]);
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
        BINARY_VERSION,
        "change-files = false\nconventional-commits = true\n",
    );
    fs::write(root.join("src/lib.rs"), "// changed\n").expect("edit");
    commit(&root, "feat(demo): add thing");
    let (ok, stdout, stderr) = oakum_output(&root, &["check", "--strict", "--from", "HEAD~1"]);
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
    write_pinned_config(&root, BINARY_VERSION, "");
    fs::write(root.join("src/lib.rs"), "// changed\n").expect("edit");
    commit(&root, "feat(demo): add thing");
    let (ok, stdout, stderr) = oakum_output(&root, &["check", "--strict", "--from", "HEAD~1"]);
    assert!(
        !ok,
        "feat(demo) must not cover when change files are on, got: {stderr}"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.contains("demo"), "{stderr}");
    assert!(stderr.contains("no covering intent"), "{stderr}");
    assert!(stderr.contains("bump file"), "{stderr}");
}

fn clone_of(origin: &Fixture, label: &str) -> PathBuf {
    let dest = sibling(origin, label);
    let src_s = origin.to_str().expect("utf-8 path");
    let dest_s = dest.to_str().expect("utf-8 dest");
    // Remote-lookback needs a normal clone (hardlinks), not `--no-local`.
    git(origin, &["clone", src_s, dest_s]);
    dest
}

#[test]
#[should_panic(expected = "one path segment")]
fn clone_of_rejects_nested_dest_names() {
    let origin = temp_git_repo("clone-reject");
    let _ = clone_of(&origin, "a/b");
}

fn tagged_cargo(label: &str, versions: &[&str]) -> Fixture {
    let root = temp_git_repo(label);
    write_pinned_config(&root, BINARY_VERSION, "");
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
    let (ok, stdout, stderr) = oakum_output(&dest, &["check", "--remote"]);
    assert!(!ok, "missing remote tag must be unverified, got: {stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("v0.1.0"), "{stderr}");
}

#[test]
fn remote_reports_unverified_when_advertised_tags_are_missing_locally() {
    let origin = tagged_cargo("remote-missing-origin", &["0.1.0"]);
    let dest = clone_of(&origin, "remote-missing-dest");
    git(&dest, &["tag", "-d", "v0.1.0"]);
    let (ok, stdout, stderr) = oakum_output(&dest, &["check", "--remote"]);
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
    let (ok, stdout, stderr) = oakum_output(&dest, &["check", "--remote"]);
    assert!(ok, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn remote_fails_closed_without_a_remote() {
    let root = tagged_cargo("remote-none", &["0.1.0"]);
    let (ok, stdout, stderr) = oakum_output(&root, &["check", "--remote"]);
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
    let (ok, stdout, stderr) = oakum_output(&dest, &["check", "--remote"]);
    assert!(
        ok,
        "default lookback of 3 should skip v0.1.0, got: {stderr}"
    );
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
    let (ok, stdout, stderr) =
        oakum_output(&dest, &["check", "--remote", "--remote-lookback", "4"]);
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
    let (ok, stdout, stderr) = oakum_output(&dest, &["check", "--remote"]);
    assert!(ok, "semver lookback of 3 should skip v0.9.0, got: {stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn remote_lookback_zero_is_rejected_by_clap() {
    let origin = tagged_cargo("remote-lookback-zero-origin", &["0.1.0"]);
    let dest = clone_of(&origin, "remote-lookback-zero-dest");
    let (ok, stdout, stderr) =
        oakum_output(&dest, &["check", "--remote", "--remote-lookback", "0"]);
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
    let (ok, stdout, stderr) = oakum_output(&dest, &["check", "--remote-lookback", "4"]);
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
    let (ok, stdout, stderr) = oakum_output(&dest, &["check", "--remote"]);
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
    let (ok, stdout, stderr) = oakum_output(&dest, &["check", "--remote"]);
    assert!(!ok, "failed ls-remote must be unverified, got: {stdout}");
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("ls-remote"), "{stderr}");
    assert!(
        !stderr.contains("git push --tags"),
        "must not name push when the look failed: {stderr}"
    );
}

#[cfg(unix)]
#[test]
fn remote_read_over_ssh_refuses_to_prompt() {
    let root = tagged_cargo("remote-ssh-batchmode", &["0.1.0"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "git@example.invalid:demo/demo.git",
        ],
    );
    let log = root.join("ssh-args.log");
    let script = recording_fake_ssh(&root, &log);

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env("GIT_SSH_COMMAND", script.to_str().expect("utf-8"))
        .output()
        .expect("oakum");

    let recorded = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        recorded.contains("-o\nBatchMode=yes\n"),
        "git did not pass BatchMode to ssh; recorded: {recorded:?}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "an unreachable remote must not pass");
    assert!(
        stderr.contains("unverified"),
        "a remote read that failed is unverified, got: {stderr}"
    );
}

/// A user's own ssh command is composed with, not replaced: the fake ssh still
/// runs, and still receives `BatchMode`.
#[cfg(unix)]
#[test]
fn a_user_ssh_command_keeps_its_own_arguments() {
    let root = tagged_cargo("remote-ssh-compose", &["0.1.0"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "git@example.invalid:demo/demo.git",
        ],
    );
    let log = root.join("ssh-args.log");
    let script = recording_fake_ssh(&root, &log);

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env(
            "GIT_SSH_COMMAND",
            format!("{} -i /dev/null", script.to_str().expect("utf-8")),
        )
        .output()
        .expect("oakum");

    let recorded = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        recorded.contains("-i\n/dev/null\n") && recorded.contains("-o\nBatchMode=yes\n"),
        "user arguments must survive alongside BatchMode; recorded: {recorded:?}"
    );
    assert!(!out.status.success(), "an unreachable remote must not pass");
}

/// `core.sshCommand` is tier two of git's precedence and no test reached it
/// while every fixture set `GIT_SSH_COMMAND`, which short-circuits the read.
#[cfg(unix)]
#[test]
fn a_config_ssh_command_is_composed_with_not_replaced() {
    let root = tagged_cargo("remote-ssh-config", &["0.1.0"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "git@example.invalid:demo/demo.git",
        ],
    );
    let log = root.join("ssh-args.log");
    let script = recording_fake_ssh(&root, &log);
    git(
        &root,
        &["config", "core.sshCommand", script.to_str().expect("utf-8")],
    );

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env_remove("GIT_SSH_COMMAND")
        .output()
        .expect("oakum");

    let recorded = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !recorded.is_empty(),
        "the user's core.sshCommand was replaced, not composed with; recorded nothing"
    );
    assert!(
        recorded.contains("-o\nBatchMode=yes\n"),
        "core.sshCommand did not receive BatchMode; recorded: {recorded:?}"
    );
    assert!(!out.status.success(), "an unreachable remote must not pass");
    // A transport oakum could compose has nothing to warn about.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("cannot refuse ssh prompts"),
        "a composed transport needs no note: {stderr}"
    );
}

/// The sandboxed `GIT_CONFIG_GLOBAL` preserves the global tier production
/// reads; a local `core.sshCommand` alone would not prove that tier.
#[cfg(unix)]
#[test]
fn a_global_config_ssh_command_is_composed_with_not_replaced() {
    let root = tagged_cargo("remote-ssh-global", &["0.1.0"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "git@example.invalid:demo/demo.git",
        ],
    );
    let log = root.join("ssh-args.log");
    let script = recording_fake_ssh(&root, &log);
    git(
        &root,
        &[
            "config",
            "--global",
            "core.sshCommand",
            script.to_str().expect("utf-8"),
        ],
    );

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env_remove("GIT_SSH_COMMAND")
        .output()
        .expect("oakum");

    let recorded = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !recorded.is_empty(),
        "the user's global core.sshCommand was replaced, not composed with; recorded nothing"
    );
    assert!(
        recorded.contains("-o\nBatchMode=yes\n"),
        "global core.sshCommand did not receive BatchMode; recorded: {recorded:?}"
    );
    assert!(!out.status.success(), "an unreachable remote must not pass");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("cannot refuse ssh prompts"),
        "a composed transport needs no note: {stderr}"
    );
}

/// A probe that cannot run must not be read as "the key is absent":
/// `GIT_SSH_COMMAND` outranks `core.sshCommand`, so guessing would silently
/// replace a transport oakum merely failed to read.
#[cfg(unix)]
#[test]
fn an_unreadable_config_ssh_command_is_unverified() {
    let root = tagged_cargo("remote-ssh-unreadable", &["0.1.0"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "git@example.invalid:demo/demo.git",
        ],
    );
    // A git that fails only the core.sshCommand probe and passes everything else
    // through, so the failure under test is the probe and nothing else.
    let shim_dir = sibling(&root, "shim");
    assert_sibling_in_container(&root, &shim_dir);
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
            "#!/bin/sh\ncase \"$3\" in *sshcommand*) ;; *) exec {real} \"$@\" ;; esac\nif [ \"$1\" = config ] && [ \"$2\" = --get-regexp ]; then\n\
             echo 'fatal: unable to read config file: Permission denied' >&2\n exit 128\nfi\nexec {real} \"$@\"\n",
            real = real.trim()
        ),
    );

    let path = format!(
        "{}:{}",
        shim_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = oakum(&root)
        .args(["check", "--remote"])
        .env("PATH", &path)
        .env_remove("GIT_SSH_COMMAND")
        .env_remove("GIT_SSH_VARIANT")
        .output()
        .expect("oakum");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "an unreadable probe must not pass");
    assert!(
        stderr.contains("unverified") && stderr.contains("ssh configuration"),
        "a failed probe must be reported, not silently treated as unset; got: {stderr}"
    );
}

/// The measured failure: setting `GIT_SSH_COMMAND` alone used to leave the
/// unreadable-config error in place because the probe ran first. With both
/// env vars set the probe is skipped, so the remedy the message now
/// prescribes actually clears the condition.
#[cfg(unix)]
#[test]
fn an_unreadable_config_is_skipped_when_both_ssh_env_vars_are_set() {
    let root = tagged_cargo("remote-ssh-env-outranks", &["0.1.0"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "git@example.invalid:demo/demo.git",
        ],
    );
    let shim_dir = sibling(&root, "shim");
    assert_sibling_in_container(&root, &shim_dir);
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
            "#!/bin/sh\ncase \"$3\" in *sshcommand*) ;; *) exec {real} \"$@\" ;; esac\nif [ \"$1\" = config ] && [ \"$2\" = --get-regexp ]; then\n\
             echo 'fatal: unable to read config file: Permission denied' >&2\n exit 128\nfi\nexec {real} \"$@\"\n",
            real = real.trim()
        ),
    );

    let path = format!(
        "{}:{}",
        shim_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = oakum(&root)
        .args(["check", "--remote"])
        .env("PATH", path)
        .env(
            "GIT_SSH_COMMAND",
            "ssh -o BatchMode=yes -o ConnectTimeout=1",
        )
        .env("GIT_SSH_VARIANT", "ssh")
        .output()
        .expect("oakum");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("ssh configuration oakum could not read"),
        "both SSH env vars must skip the unreadable config probe; got: {stderr}"
    );
}

/// One env var is not enough: variant still falls back to config, so the probe
/// still runs and an unreadable config must still surface.
#[cfg(unix)]
#[test]
fn an_unreadable_config_still_fails_when_only_git_ssh_command_is_set() {
    let root = tagged_cargo("remote-ssh-env-command-only", &["0.1.0"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "git@example.invalid:demo/demo.git",
        ],
    );
    let shim_dir = sibling(&root, "shim");
    assert_sibling_in_container(&root, &shim_dir);
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
            "#!/bin/sh\ncase \"$3\" in *sshcommand*) ;; *) exec {real} \"$@\" ;; esac\nif [ \"$1\" = config ] && [ \"$2\" = --get-regexp ]; then\n\
             echo 'fatal: unable to read config file: Permission denied' >&2\n exit 128\nfi\nexec {real} \"$@\"\n",
            real = real.trim()
        ),
    );

    let path = format!(
        "{}:{}",
        shim_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = oakum(&root)
        .args(["check", "--remote"])
        .env("PATH", path)
        .env(
            "GIT_SSH_COMMAND",
            "ssh -o BatchMode=yes -o ConnectTimeout=1",
        )
        .env_remove("GIT_SSH_VARIANT")
        .output()
        .expect("oakum");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "command alone must not skip the probe"
    );
    assert!(
        stderr.contains("unverified") && stderr.contains("ssh configuration"),
        "command alone must still report the unreadable probe; got: {stderr}"
    );
}

/// A signal leaves both streams empty, and the probe rendered its error from
/// stderr alone: the message became `could not read the ssh configuration ()`.
/// The `Op` path already said `terminated by a signal`; this is the same
/// rendering, shared rather than re-derived.
#[cfg(unix)]
#[test]
fn a_signalled_config_probe_names_the_signal() {
    let root = tagged_cargo("remote-ssh-signalled", &["0.1.0"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "git@example.invalid:demo/demo.git",
        ],
    );
    let shim_dir = sibling(&root, "shim");
    assert_sibling_in_container(&root, &shim_dir);
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
            // Only the ssh probe: `Op::TagOptRemotes` runs `config --get-regexp`
            // too, and killing that one produces the same phrase from the other
            // code path, which would let this test pass for the wrong reason.
            "#!/bin/sh\ncase \"$3\" in *sshcommand*) kill -TERM $$ ;; esac\nexec {real} \"$@\"\n",
            real = real.trim()
        ),
    );

    let path = format!(
        "{}:{}",
        shim_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = oakum(&root)
        .args(["check", "--remote"])
        .env("PATH", path)
        .env_remove("GIT_SSH_COMMAND")
        .env_remove("GIT_SSH_VARIANT")
        .output()
        .expect("oakum");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "a killed probe must not pass");
    assert!(
        stderr.contains("terminated by a signal"),
        "a signalled probe must say so rather than render an empty detail; got: {stderr}"
    );
}

/// Git's trace channels write to stderr, and the probe reads a non-empty stderr
/// as a failure. Inherited from the caller's shell, an exported `GIT_TRACE`
/// stopped every remote operation with a message blaming the ssh configuration.
#[cfg(unix)]
#[test]
fn an_inherited_trace_is_not_read_as_a_broken_ssh_config() {
    let root = tagged_cargo("remote-traced", &["0.1.0"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "git@example.invalid:demo/demo.git",
        ],
    );
    for trace in ["GIT_TRACE", "GIT_TRACE_PACKET"] {
        let out = oakum(&root)
            .args(["check", "--remote"])
            .env(trace, "1")
            .env_remove("GIT_SSH_COMMAND")
            .output()
            .expect("oakum");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stderr.contains("ssh configuration"),
            "{trace} must not read as a broken ssh config; got: {stderr}"
        );
        assert!(
            !stderr.contains("trace:"),
            "{trace} must not reach oakum's diagnostics; got: {stderr}"
        );
    }
}

/// `GIT_TERMINAL_PROMPT=0` does not reach git's askpass chain: with prompts
/// disabled, git still runs an askpass helper for an https credential, and a
/// GUI helper blocks forever. Editors export `GIT_ASKPASS` routinely.
#[cfg(unix)]
#[test]
fn an_https_remote_does_not_reach_the_askpass_helper() {
    let root = tagged_cargo("remote-askpass", &["0.1.0"]);
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/demo/demo.git/info/refs");
        then.status(401)
            .header("WWW-Authenticate", "Basic realm=\"git\"");
    });
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            &format!("{}/demo/demo.git", server.base_url()),
        ],
    );

    let log = sibling(&root, "askpass-calls.log");

    assert_sibling_in_container(&root, &log);
    let _ = fs::remove_file(&log);
    let script = sibling(&root, "fake-askpass");
    assert_sibling_in_container(&root, &script);
    install_executable(
        &script,
        format!(
            "#!/bin/sh\nprintf 'ASKPASS %s\\n' \"$*\" >> {}\necho hunter2\n",
            log.display()
        ),
    );

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env("GIT_ASKPASS", script.to_str().expect("utf-8"))
        .env("GIT_TERMINAL_PROMPT", "1")
        .output()
        .expect("oakum");

    let calls = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        calls.is_empty(),
        "git reached the askpass helper, which can block indefinitely; calls: {calls:?}"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "a 401 remote must not pass");
    assert!(stderr.contains("unverified"), "got: {stderr}");
}

/// oakum disables git's prompt chain on every git child, so the credential
/// failure that produces is a state oakum caused: the report must say so and
/// name the remedy, with git's own text kept as the evidence.
#[test]
fn a_credential_starved_remote_read_names_the_fix() {
    let root = tagged_cargo("remote-starved", &["0.1.0"]);
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/demo/demo.git/info/refs");
        then.status(401)
            .header("WWW-Authenticate", "Basic realm=\"git\"");
    });
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            &format!("{}/demo/demo.git", server.base_url()),
        ],
    );
    let out = oakum(&root)
        .args(["check", "--remote"])
        .output()
        .expect("oakum");
    assert!(!out.status.success(), "a 401 remote must not pass");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("failed:"), "{stderr}");
    assert!(stderr.contains("credential helper"), "{stderr}");
    assert!(stderr.contains("gh auth setup-git"), "{stderr}");
}

/// The note is about ssh, but the transport resolves from the environment
/// before any remote URL is known. Gating on the transport alone prints it for
/// an `https://` remote, where ssh is never invoked and the prompt it describes
/// cannot occur.
#[cfg(unix)]
#[test]
fn an_https_remote_is_not_told_about_ssh_prompts() {
    let root = tagged_cargo("remote-https-quiet", &["0.1.0"]);
    let server = MockServer::start();
    let advertised = server.mock(|when, then| {
        when.method(GET).path("/demo/demo.git/info/refs");
        then.status(200)
            .header(
                "Content-Type",
                "application/x-git-upload-pack-advertisement",
            )
            .body("");
    });
    git(
        &root,
        &["remote", "add", "origin", &server.url("/demo/demo.git")],
    );

    let out = oakum(&root)
        .args(["check", "--remote"])
        // The same opaque transport the ssh case uses: what changes here is the
        // remote, not the transport.
        .env_remove("GIT_SSH_COMMAND")
        .env("GIT_SSH", "/usr/local/bin/my-ssh")
        .output()
        .expect("oakum");

    // Without this the test proves nothing: an absent note reads the same when
    // no remote child ran at all.
    advertised.assert();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("cannot refuse ssh prompts"),
        "an https remote must not be warned about ssh prompts: {stderr}"
    );
}

/// The gate decides from `git remote get-url`, and a URL oakum cannot read is
/// unestablished rather than not-ssh. The note is advisory, so withholding it
/// because the check itself failed is the quieter of the two wrong answers, and
/// the test below also holds it to saying which of the two it is.
#[cfg(unix)]
#[test]
fn a_remote_url_that_cannot_be_read_still_gets_the_ssh_note() {
    let root = tagged_cargo("remote-url-unreadable", &["0.1.0"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "git@example.invalid:demo/demo.git",
        ],
    );
    // Fails only the `remote -v` listing and passes everything else through,
    // so the remote operation itself still runs and only the reach read loses
    // its answer.
    let shim_dir = sibling(&root, "shim");
    assert_sibling_in_container(&root, &shim_dir);
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
            "#!/bin/sh\nif [ \"$1\" = remote ] && [ \"$2\" = -v ]; then\n\
             echo 'fatal: unreadable' >&2\n exit 2\nfi\nexec {real} \"$@\"\n",
            real = real.trim()
        ),
    );

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env(
            "PATH",
            format!(
                "{}:{}",
                shim_dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env_remove("GIT_SSH_COMMAND")
        .env("GIT_SSH", "/usr/local/bin/my-ssh")
        .output()
        .expect("oakum");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("could not establish what transport"),
        "an unreadable remote URL must not silence the note: {stderr}"
    );
    // The note is about prompts, not about the URL probe: a transport that can
    // take `BatchMode` has nothing to say however the URL read went.
    let composed = oakum(&root)
        .args(["check", "--remote"])
        .env(
            "PATH",
            format!(
                "{}:{}",
                shim_dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("GIT_SSH_COMMAND", "ssh -i /dev/null")
        .output()
        .expect("oakum");
    let composed = String::from_utf8_lossy(&composed.stderr).into_owned();
    // The unread hedge still prints under a composed transport — oakum cannot
    // rule a helper out — but without the transport rider.
    assert!(
        composed.contains("could not establish what transport"),
        "an unread listing hedges even when the transport composed: {composed}"
    );
    assert!(
        !composed.contains("could not be protected"),
        "a composed transport owes no transport rider: {composed}"
    );
    // And it must not read as a confident "this remote is ssh" either: the two
    // are different statements and only one of them was established.
    assert!(
        stderr.contains("could not read that remote's URL"),
        "the note must say the check itself failed: {stderr}"
    );
    assert!(
        stderr.contains("fatal: unreadable"),
        "the note must carry git's own reason: {stderr}"
    );
    assert!(
        stderr.contains("\"origin\""),
        "the note must name which remote it is about: {stderr}"
    );
}

/// The transport already chose `BatchMode`, and ssh takes the first value of a
/// repeated option — so oakum's append is inert and a prompt can still block.
/// The note is the only signal that happens, and it has to carry why while the
/// user's own ssh command still runs with its own arguments.
#[cfg(unix)]
#[test]
fn an_inert_batch_mode_still_runs_the_user_ssh_and_says_why() {
    let root = tagged_cargo("remote-ssh-inert", &["0.1.0"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "git@example.invalid:demo/demo.git",
        ],
    );
    let log = root.join("ssh-args.log");
    let script = recording_fake_ssh(&root, &log);

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env(
            "GIT_SSH_COMMAND",
            format!("{} -o BatchMode=no", script.display()),
        )
        .output()
        .expect("oakum");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cannot refuse ssh prompts"),
        "an inert append must still be reported: {stderr}"
    );
    let recorded = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        recorded.contains("BatchMode=no"),
        "the user's own ssh arguments must still reach ssh: {recorded}"
    );
    assert!(!out.status.success(), "an unreachable remote must not pass");
}

/// An ssh configuration oakum cannot read stops a remote operation whether or
/// not that remote reaches ssh. Exercised on the fetch direction; the push
/// direction shares the one `map_err` in `Git::child`.
///
/// The note is advisory and is gated on the URL classifier; this is not. A
/// classifier wrong in the other direction lets the child run with no
/// `BatchMode`, and refusing a `file://` remote over ssh configuration it
/// cannot use is the cheaper mistake.
#[cfg(unix)]
#[test]
fn an_unreadable_ssh_config_stops_a_remote_read() {
    let root = tagged_cargo("remote-local-unreadable-ssh", &["0.1.0"]);
    let bare = sibling(&root, "remote-local.git");
    let _ = fs::remove_dir_all(&bare);
    git(&root, &["init", "--bare", bare.to_str().expect("utf-8")]);
    git(
        &root,
        &["remote", "add", "origin", bare.to_str().expect("utf-8")],
    );
    git(&root, &["push", "--tags", "origin", "HEAD"]);

    // Fails only the ssh-config probe, exactly as the ssh case does.
    let log = sibling(&root, "oakum-stops-read.log");
    assert_sibling_in_container(&root, &log);
    let _ = fs::remove_file(&log);
    let shim_dir = sibling(&root, "shim");
    assert_sibling_in_container(&root, &shim_dir);
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
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {log}\n\
             case \"$*\" in *sshcommand*)\n\
             echo 'fatal: unable to read config file: Permission denied' >&2\n exit 128 ;; esac\n\
             exec {real} \"$@\"\n",
            log = log.to_str().expect("utf-8 log"),
            real = real.trim()
        ),
    );

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env(
            "PATH",
            format!(
                "{}:{}",
                shim_dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env_remove("GIT_SSH_COMMAND")
        .output()
        .expect("oakum");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "an unreadable ssh config must refuse"
    );
    assert!(
        stderr.contains("ssh configuration oakum could not read"),
        "the refusal must name the cause: {stderr}"
    );
    // Every child needs the transport now, so the refusal names whichever
    // operation ran first — a local one here — rather than waiting for the
    // remote read.
    assert!(
        stderr.contains("git rev-parse --is-shallow-repository needs an ssh configuration"),
        "the refusal must name the operation it stopped: {stderr}"
    );
    assert!(
        stderr.contains("will not guess a transport"),
        "the refusal must say why it stops rather than guessing: {stderr}"
    );
    // Refusing after the spawn would read the same from stderr and still be the
    // prompt hang: the child would have run with no `BatchMode`.
    let argv = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !argv.lines().any(|line| line.starts_with("ls-remote")),
        "the refusal must come before any remote child:\n{argv}"
    );
}

/// The refusal comes before any remote child. A transport oakum could not read
/// must never reach `ls-remote` without `BatchMode`, which is the prompt hang
/// okm-6mz fixed — and a refusal issued after the spawn would look identical
/// from stderr.
#[cfg(unix)]
#[test]
fn a_transport_failure_with_an_unreadable_url_refuses_before_any_remote_child() {
    let root = tagged_cargo("remote-both-probes-fail", &["0.1.0"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "git@example.invalid:demo/demo.git",
        ],
    );
    let log = sibling(&root, "oakum-both-probes.log");
    assert_sibling_in_container(&root, &log);
    let _ = fs::remove_file(&log);
    let shim_dir = sibling(&root, "shim");
    assert_sibling_in_container(&root, &shim_dir);
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
    // Both probes fail; everything else, including the argv log, passes through.
    install_executable(
        &shim,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {log}\n\
             case \"$*\" in\n\
             *sshcommand*) echo 'fatal: unable to read config file' >&2; exit 128 ;;\n\
             esac\nexec {real} \"$@\"\n",
            log = log.to_str().expect("utf-8 log"),
            real = real.trim()
        ),
    );

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env(
            "PATH",
            format!(
                "{}:{}",
                shim_dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env_remove("GIT_SSH_COMMAND")
        .output()
        .expect("oakum");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "an unreadable ssh config must refuse"
    );
    assert!(
        stderr.contains("ssh configuration oakum could not read"),
        "the refusal must name the cause: {stderr}"
    );
    let argv = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !argv.lines().any(|line| line.starts_with("ls-remote")),
        "no remote child may run without a resolved transport:\n{argv}"
    );
}

/// The three deadline tests are the only ones in this file that assert on
/// elapsed wall clock, and libtest runs them alongside everything else. Held
/// one at a time they compete with the rest of the suite but not with each
/// other, which is the contention their own two-second budget cannot absorb.
///
/// A poisoned lock is recovered rather than turned into a second failure: the
/// panic that poisoned it already failed its own test.
#[cfg(unix)]
fn deadline_turn() -> std::sync::MutexGuard<'static, ()> {
    static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());
    TURN.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A credential helper runs with oakum's environment applied and blocks
/// anyway — `GIT_ASKPASS` and `GIT_TERMINAL_PROMPT` both reach it and neither
/// stops it — so only the wall-clock deadline bounds it. Expiry is the third
/// outcome, never a completed look.
#[cfg(unix)]
#[test]
fn a_blocking_credential_helper_meets_the_deadline() {
    let _turn = deadline_turn();
    let root = tagged_cargo("deadline-helper", &["0.1.0"]);
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/demo/demo.git/info/refs");
        then.status(401)
            .header("WWW-Authenticate", "Basic realm=\"git\"");
    });
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            &format!("{}/demo/demo.git", server.base_url()),
        ],
    );
    git(
        &root,
        &["config", "credential.helper", "!f() { sleep 60; }; f"],
    );

    let started = std::time::Instant::now();
    let out = oakum(&root)
        .args(["check", "--remote"])
        .env("OAKUM_REMOTE_DEADLINE", "2")
        .output()
        .expect("oakum");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(30),
        "the deadline must bound the helper's sleep"
    );
    assert!(!out.status.success(), "an expired deadline must not pass");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("gave up after 2s"), "{stderr}");
    assert!(stderr.contains("credential helper"), "{stderr}");
    assert!(stderr.contains("OAKUM_REMOTE_DEADLINE"), "{stderr}");
}

/// A rejected `OAKUM_REMOTE_DEADLINE` refuses loudly, naming the variable —
/// never a silent fall-back to the default that would quietly discard the
/// deadline the user asked for.
#[cfg(unix)]
#[test]
fn a_malformed_deadline_refuses_loudly() {
    let root = tagged_cargo("deadline-malformed", &["0.1.0"]);
    git(
        &root,
        &["remote", "add", "origin", "https://host.invalid/r.git"],
    );
    let out = oakum(&root)
        .args(["check", "--remote"])
        .env("OAKUM_REMOTE_DEADLINE", "abc")
        .output()
        .expect("oakum");
    assert!(!out.status.success(), "a rejected deadline must not pass");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("positive whole number"),
        "the refusal must name the constraint: {stderr}"
    );
    assert!(stderr.contains("OAKUM_REMOTE_DEADLINE"), "{stderr}");
    assert!(
        !stderr.contains("could not run git"),
        "a config mistake must not read as a spawn failure: {stderr}"
    );
}

/// The child can exit while something it spawned holds the pipes open; that
/// is not a kill, and the report says what actually happened — the exit
/// status in hand, the output uncollectable.
#[cfg(unix)]
#[test]
fn a_grandchild_holding_the_pipes_meets_the_deadline_without_a_kill_claim() {
    let _turn = deadline_turn();
    let root = tagged_cargo("deadline-drain", &["0.1.0"]);
    git(
        &root,
        &["remote", "add", "origin", "https://host.invalid/r.git"],
    );
    let shim_dir = sibling(&root, "shim");
    assert_sibling_in_container(&root, &shim_dir);
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
            "#!/bin/sh\ncase \"$1\" in ls-remote) sleep 60 & exit 0 ;; esac\nexec {real} \"$@\"\n",
            real = real.trim()
        ),
    );
    let out = oakum(&root)
        .args(["check", "--remote"])
        .env(
            "PATH",
            format!(
                "{}:{}",
                shim_dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("OAKUM_REMOTE_DEADLINE", "2")
        .output()
        .expect("oakum");
    assert!(!out.status.success(), "a stalled drain must not pass");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("still held its output open"), "{stderr}");
    assert!(stderr.contains("exit status: 0"), "{stderr}");
    assert!(
        !stderr.contains("killed"),
        "nothing was killed and the report must not claim it: {stderr}"
    );
}

/// An interactive `ProxyCommand` is spawned and waited on even under
/// `BatchMode=yes` (measured), so it is the other prompt source only the
/// deadline covers.
#[cfg(unix)]
#[test]
fn a_blocking_proxy_command_meets_the_deadline() {
    let _turn = deadline_turn();
    let root = tagged_cargo("deadline-proxy", &["0.1.0"]);
    git(
        &root,
        &["remote", "add", "origin", "ssh://git@host.invalid/demo.git"],
    );

    let started = std::time::Instant::now();
    let out = oakum(&root)
        .args(["check", "--remote"])
        .env("GIT_SSH_COMMAND", "ssh -o \"ProxyCommand=sleep 60\"")
        .env("OAKUM_REMOTE_DEADLINE", "2")
        .output()
        .expect("oakum");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(30),
        "the deadline must bound the proxy's sleep"
    );
    assert!(!out.status.success(), "an expired deadline must not pass");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("unverified"), "{stderr}");
    assert!(stderr.contains("gave up after 2s"), "{stderr}");
}

/// A partial clone's `diff` lazily fetches from the promisor remote, dialing
/// ssh from a child oakum types as local — so the composed transport must
/// ride every git child, not only the remote-classed ones.
#[cfg(unix)]
#[test]
fn a_local_child_carries_the_composed_transport() {
    let root = tagged_cargo("local-transport", &["0.1.0"]);
    let log = sibling(&root, "oakum-local-transport.log");
    assert_sibling_in_container(&root, &log);
    let _ = fs::remove_file(&log);
    let shim_dir = sibling(&root, "shim");
    assert_sibling_in_container(&root, &shim_dir);
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
            "#!/bin/sh\nprintf '%s|%s\\n' \"$1\" \"${{GIT_SSH_COMMAND-}}\" >> {log}\n\
             exec {real} \"$@\"\n",
            log = log.to_str().expect("utf-8 log"),
            real = real.trim()
        ),
    );

    let out = oakum(&root)
        .arg("check")
        .env(
            "PATH",
            format!(
                "{}:{}",
                shim_dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("GIT_SSH_COMMAND", "ssh -i /dev/null")
        .output()
        .expect("oakum");
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let spawns = fs::read_to_string(&log).unwrap_or_default();
    let local = spawns
        .lines()
        .filter(|line| {
            let verb = line.split('|').next().unwrap_or_default();
            !matches!(verb, "config" | "ls-remote" | "push")
        })
        .collect::<Vec<_>>();
    assert!(!local.is_empty(), "no local child ran:\n{spawns}");
    for line in local {
        assert!(
            line.ends_with("ssh -i /dev/null -o BatchMode=yes"),
            "a local child ran without the composed transport: {line}\n{spawns}"
        );
    }
}

/// `marker://addr` selects `git-remote-marker` exactly as `marker::addr`
/// does — same helper, same hazard — so the scheme spelling gets the same
/// note the `::` spelling does.
#[cfg(unix)]
#[test]
fn a_helper_named_by_url_scheme_gets_the_note_too() {
    let root = tagged_cargo("remote-scheme-helper", &["0.1.0"]);
    git(
        &root,
        &["remote", "add", "origin", "marker://addr/demo.git"],
    );

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env_remove("GIT_SSH_COMMAND")
        .env("GIT_SSH", "/usr/local/bin/my-ssh")
        .output()
        .expect("oakum");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("runs a helper command"),
        "a scheme-selected helper gets the helper note: {stderr}"
    );
    assert!(
        !stderr.contains("does not fetch over ssh") && !stderr.contains("does not push over ssh"),
        "a helper transport must not read as established safe: {stderr}"
    );
}

/// A `<helper>::<address>` remote runs a command oakum cannot inspect, and an
/// `ext::` helper can invoke ssh itself — measured, one did, with no
/// `BatchMode`: it inherits `GIT_SSH_COMMAND` and applies none of it. So it is
/// unestablished, and calling it "does not reach ssh" asserts something untrue.
#[cfg(unix)]
#[test]
fn a_helper_remote_is_not_reported_as_free_of_ssh() {
    let root = tagged_cargo("remote-helper", &["0.1.0"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "ext::ssh -p 22 git@example.invalid %S demo.git",
        ],
    );

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env_remove("GIT_SSH_COMMAND")
        .env("GIT_SSH", "/usr/local/bin/my-ssh")
        .output()
        .expect("oakum");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("does not fetch over ssh") && !stderr.contains("does not push over ssh"),
        "a helper transport is unestablished, not established as safe: {stderr}"
    );
    assert!(
        stderr.contains("runs a helper command"),
        "a helper remote gets the helper note: {stderr}"
    );
    assert!(
        stderr.contains("cannot inspect"),
        "the note must say why it could not tell: {stderr}"
    );
    // oakum read this URL fine; what it could not establish is the transport.
    assert!(
        !stderr.contains("could not read that remote's URL"),
        "the note must not claim a read failed when it did not: {stderr}"
    );
}

/// The reach read costs one `remote -v` listing per run, cached for every
/// remote and direction — never a per-remote `get-url` child. Read even when
/// the transport composed, because a helper remote owes its note regardless.
#[cfg(unix)]
#[test]
fn the_reach_read_costs_one_listing_child_per_run() {
    let root = tagged_cargo("remote-composed-lazy", &["0.1.0"]);
    let bare = sibling(&root, "remote-composed-lazy.git");
    let _ = fs::remove_dir_all(&bare);
    git(&root, &["init", "--bare", bare.to_str().expect("utf-8")]);
    git(
        &root,
        &["remote", "add", "origin", bare.to_str().expect("utf-8")],
    );
    git(&root, &["push", "--tags", "origin", "HEAD"]);

    let log = sibling(&root, "oakum-lazy-url.log");

    assert_sibling_in_container(&root, &log);
    let _ = fs::remove_file(&log);
    let shim_dir = sibling(&root, "shim");
    assert_sibling_in_container(&root, &shim_dir);
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

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env(
            "PATH",
            format!(
                "{}:{}",
                shim_dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        // Composable, so `BatchMode` is appended and nothing is ever warned.
        .env("GIT_SSH_COMMAND", "ssh -i /dev/null")
        .output()
        .expect("oakum");

    let argv = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        argv.lines().any(|line| line.starts_with("ls-remote")),
        "the remote was never contacted, so this proves nothing:\n{argv}"
    );
    // The reach is read even under a composed transport — a helper remote
    // owes its note regardless — but it costs one `remote -v` listing for the
    // whole run, never a per-remote `get-url` child.
    assert_eq!(
        argv.lines().filter(|line| *line == "remote -v").count(),
        1,
        "one listing fills the reach cache for every remote:\n{argv}"
    );
    assert!(
        !argv.lines().any(|line| line.starts_with("remote get-url")),
        "no per-remote URL child:\n{argv}"
    );
}

/// The reach is read for the remote in hand, not for a name assumed to be
/// `origin` — measured: that hardcode passed the whole suite, because every
/// other ssh-note test names its remote `origin`. `preferred_remote` takes the
/// only remote there is, so a repository with just `upstream` drives this.
#[cfg(unix)]
#[test]
fn the_note_reads_the_remote_in_hand_not_one_named_origin() {
    let root = tagged_cargo("remote-ssh-upstream", &["0.1.0"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "upstream",
            "git@example.invalid:demo/demo.git",
        ],
    );

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env_remove("GIT_SSH_COMMAND")
        .env("GIT_SSH", "/usr/local/bin/my-ssh")
        .output()
        .expect("oakum");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("\"upstream\""),
        "the note must name the remote it read: {stderr}"
    );
    assert!(
        stderr.contains("A prompt can still block."),
        "the reach was established, so the note must not hedge: {stderr}"
    );
    assert!(
        !stderr.contains("could not establish whether ssh is involved"),
        "the reach was read for a real remote: {stderr}"
    );
}

/// The note is the only signal that oakum's prompt refusal does not apply, and
/// it must not repeat: `release` builds one remote child per tag plus two.
#[cfg(unix)]
#[test]
fn an_opaque_transport_is_reported_once() {
    let root = tagged_cargo("remote-ssh-opaque", &["0.1.0"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "git@example.invalid:demo/demo.git",
        ],
    );

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env_remove("GIT_SSH_COMMAND")
        .env("GIT_SSH", "/usr/local/bin/my-ssh")
        .output()
        .expect("oakum");

    let stderr = String::from_utf8_lossy(&out.stderr);
    let notes = stderr
        .lines()
        .filter(|line| line.contains("cannot refuse ssh prompts"))
        .count();
    assert_eq!(notes, 1, "expected exactly one note, got: {stderr}");
    assert!(
        stderr.contains("/usr/local/bin/my-ssh"),
        "the note must name the transport: {stderr}"
    );
}

/// A `git` that passes everything through except one subcommand, so a single
/// operation can be made to fail while the rest of the run proceeds normally.
#[cfg(unix)]
fn git_shim(root: &Fixture, matches: &str, script: &str) -> PathBuf {
    let dir = sibling(root, "shim");
    fs::create_dir_all(&dir).expect("shim dir");
    let real = String::from_utf8(
        Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .expect("which git")
            .stdout,
    )
    .expect("utf-8");
    let shim = dir.join("git");
    install_executable(
        &shim,
        format!(
            "#!/bin/sh\ncase \"$*\" in\n  {matches}) {script} ;;\nesac\nexec {} \"$@\"\n",
            real.trim()
        ),
    );
    dir
}

#[cfg(unix)]
fn with_shim(root: &Path, dir: &Path, args: &[&str]) -> (bool, String) {
    let out = oakum(root)
        .args(args)
        .env(
            "PATH",
            format!(
                "{}:{}",
                dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .output()
        .expect("oakum");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Git says "no match" as exit 1 with both streams empty. Any other non-zero is
/// a failure to look, and reading it as absence is the collapse AGENTS.md forbids.
#[cfg(unix)]
#[test]
fn a_diagnosed_config_failure_is_not_read_as_absence() {
    let root = tagged_cargo("shim-config-diagnosed", &["0.1.0"]);
    let dir = git_shim(
        &root,
        "*--get-regexp*tagopt*",
        "echo 'fatal: bad config line 9' >&2; exit 2",
    );
    let (ok, stderr) = with_shim(&root, &dir, &["check"]);
    assert!(!ok, "a config probe that failed must not read as clean");
    assert!(
        stderr.contains("unverified"),
        "a diagnosed failure must be unverified, got: {stderr}"
    );
}

/// A tag listing that exits zero while warning has not reported "no tags"; it
/// has reported that it could not finish looking.
#[cfg(unix)]
#[test]
fn a_warning_on_a_successful_look_is_not_an_empty_answer() {
    let root = tagged_cargo("shim-warned-look", &["0.1.0"]);
    let dir = git_shim(
        &root,
        "*for-each-ref*",
        "echo 'error: refs/tags: unable to read ref database' >&2; exit 0",
    );
    let (ok, stderr) = with_shim(&root, &dir, &["check"]);
    assert!(!ok, "a warned look must not pass as an empty tag list");
    assert!(
        stderr.contains("unverified"),
        "an incomplete look must be unverified, got: {stderr}"
    );
}

/// `GIT_SSH` is the usual Windows transport. Oakum cannot append `-o`.
#[cfg(windows)]
#[test]
fn git_ssh_on_windows_is_unprotected_and_is_not_given_dash_o() {
    let root = tagged_cargo("remote-git-ssh-windows", &["0.1.0"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "git@example.invalid:demo/demo.git",
        ],
    );
    let log = root.join("ssh-args.log");
    let bat = root.join("fake-ssh.cmd");
    fs::write(
        &bat,
        format!(
            "@echo off\r\necho %*>>\"{}\"\r\nexit /b 255\r\n",
            log.display()
        ),
    )
    .expect("fake ssh");

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env("GIT_SSH", &bat)
        .output()
        .expect("oakum");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cannot refuse ssh prompts"),
        "GIT_SSH must be reported as unprotected: {stderr}"
    );
    assert!(
        stderr.contains("GIT_SSH names"),
        "the note must name GIT_SSH: {stderr}"
    );
    let recorded = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !recorded.is_empty(),
        "git must have invoked GIT_SSH; log empty, stderr: {stderr}"
    );
    assert!(
        !recorded.to_ascii_lowercase().contains("batchmode"),
        "oakum must not inject -o into GIT_SSH: {recorded}"
    );
}

#[cfg(windows)]
#[test]
fn a_plink_path_in_git_ssh_is_named_in_the_note() {
    let root = tagged_cargo("remote-plink-windows", &["0.1.0"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "git@example.invalid:demo/demo.git",
        ],
    );

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env("GIT_SSH", r"C:\PuTTY\plink.exe")
        .output()
        .expect("oakum");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cannot refuse ssh prompts"),
        "a plink GIT_SSH must be reported: {stderr}"
    );
    assert!(
        stderr.contains(r"C:\PuTTY\plink.exe"),
        "the note must name the transport: {stderr}"
    );
}

#[test]
fn an_explicit_ssh_variant_is_unprotected() {
    let root = tagged_cargo("remote-variant-simple", &["0.1.0"]);
    git(
        &root,
        &[
            "remote",
            "add",
            "origin",
            "git@example.invalid:demo/demo.git",
        ],
    );
    git(&root, &["config", "ssh.variant", "simple"]);

    let out = oakum(&root)
        .args(["check", "--remote"])
        .env_remove("GIT_SSH_COMMAND")
        .env_remove("GIT_SSH_VARIANT")
        .env_remove("GIT_SSH")
        .output()
        .expect("oakum");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cannot refuse ssh prompts"),
        "ssh.variant must be reported as unprotected: {stderr}"
    );
    assert!(
        stderr.contains("ssh.variant is `simple`"),
        "the note must name the variant: {stderr}"
    );
}
