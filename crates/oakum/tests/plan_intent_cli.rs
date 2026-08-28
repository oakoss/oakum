//! Plan intent loader: change files vs commits-only (ADR-0029 / `okm-64b.5`).

#![allow(clippy::disallowed_methods)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// A config whose `tool-version` always matches the binary under test. This
/// command is not behind the ADR-0007 gate; deriving the version keeps the
/// fixtures uniform with the suites that are.
fn versioned(rest: &str) -> String {
    format!("tool-version = \"{}\"\n{}", env!("CARGO_PKG_VERSION"), rest)
}

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oakum"))
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

fn git(root: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .expect("git");
    assert!(status.success(), "git {args:?} failed");
}

fn temp_git_repo(label: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("oakum-intent-{label}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp repo");
    git(&dir, &["init", "-b", "main"]);
    git(&dir, &["config", "user.email", "test@example.com"]);
    git(&dir, &["config", "user.name", "Test"]);
    dir
}

fn head_hash(root: &std::path::Path) -> String {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .expect("rev-parse");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn commits_only_plan_intent_from_conventional_scope() {
    let root = temp_git_repo("commits-only");
    cargo_package(&root, "demo");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        versioned("change-files = false\nconventional-commits = true\n"),
    )
    .expect("config");
    // Orphan bump file must be ignored when change-files is off.
    fs::write(
        root.join(".changeset/orphan.md"),
        "---\ndemo: major\n---\n\nshould not feed the plan\n",
    )
    .expect("orphan");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);
    let base = head_hash(&root);

    fs::write(root.join("src/lib.rs"), "// change\n").expect("edit");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "feat(demo): add thing"]);

    let output = bin()
        .current_dir(&root)
        .args(["plan-intent", "--from", &base])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"id\": \"commits\""), "stdout:\n{stdout}");
    assert!(stdout.contains("\"name\": \"demo\""), "stdout:\n{stdout}");
    assert!(stdout.contains("\"level\": \"minor\""), "stdout:\n{stdout}");
    assert!(
        !stdout.contains("orphan") && !stdout.contains("major"),
        "orphan bump file must not feed commits-only plan:\n{stdout}"
    );
}

#[test]
fn change_files_plan_intent_ignores_commits() {
    let root = temp_git_repo("files-only");
    cargo_package(&root, "demo");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        versioned("change-files = true\nconventional-commits = true\n"),
    )
    .expect("config");
    fs::write(
        root.join(".changeset/hand.md"),
        "---\ndemo: patch\n---\n\nhand written\n",
    )
    .expect("hand");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);
    let base = head_hash(&root);

    fs::write(root.join("src/lib.rs"), "// change\n").expect("edit");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "feat(demo): should not appear"]);

    let output = bin()
        .current_dir(&root)
        .args(["plan-intent", "--from", &base])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"id\": \"hand.md\""), "stdout:\n{stdout}");
    assert!(stdout.contains("\"level\": \"patch\""), "stdout:\n{stdout}");
    assert!(
        !stdout.contains("commits") && !stdout.contains("minor"),
        "commits must not feed the plan when change-files is on:\n{stdout}"
    );
}

#[test]
fn both_intent_mechanisms_off_is_an_error() {
    let root = temp_git_repo("both-off");
    cargo_package(&root, "demo");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        versioned("change-files = false\nconventional-commits = false\n"),
    )
    .expect("config");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);

    let output = bin()
        .current_dir(&root)
        .args(["plan-intent", "--from", "HEAD"])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("change-files") && err.contains("conventional-commits"),
        "stderr: {err}"
    );
}

#[test]
fn change_files_on_commits_off_still_reads_files() {
    let root = temp_git_repo("files-commits-off");
    cargo_package(&root, "demo");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        versioned("change-files = true\nconventional-commits = false\n"),
    )
    .expect("config");
    fs::write(
        root.join(".changeset/hand.md"),
        "---\ndemo: patch\n---\n\nhand written\n",
    )
    .expect("hand");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);
    let base = head_hash(&root);

    fs::write(root.join("src/lib.rs"), "// change\n").expect("edit");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "feat(demo): should not appear"]);

    let output = bin()
        .current_dir(&root)
        .args(["plan-intent", "--from", &base])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"id\": \"hand.md\""), "stdout:\n{stdout}");
    assert!(stdout.contains("\"level\": \"patch\""), "stdout:\n{stdout}");
    assert!(
        !stdout.contains("\"id\": \"commits\""),
        "commits must not feed the plan:\n{stdout}"
    );
}

#[test]
fn commits_only_empty_range_is_empty_plan() {
    let root = temp_git_repo("empty-range");
    cargo_package(&root, "demo");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        versioned("change-files = false\nconventional-commits = true\n"),
    )
    .expect("config");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);

    let output = bin()
        .current_dir(&root)
        .args(["plan-intent", "--from", "HEAD"])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert_eq!(stdout, "[]", "stdout:\n{stdout}");
}

#[test]
fn commits_only_commits_without_package_bumps_is_empty_plan() {
    let root = temp_git_repo("empty-bumps");
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/demo\"]\n",
    )
    .expect("workspace");
    fs::create_dir_all(root.join("crates/demo/src")).expect("pkg");
    fs::write(
        root.join("crates/demo/Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("pkg toml");
    fs::write(root.join("crates/demo/src/lib.rs"), "").expect("lib");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        versioned("change-files = false\nconventional-commits = true\n"),
    )
    .expect("config");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);
    let base = head_hash(&root);

    fs::write(root.join("README.md"), "hi\n").expect("readme");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "docs: outside packages"]);

    let output = bin()
        .current_dir(&root)
        .args(["plan-intent", "--from", &base])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert_eq!(stdout, "[]", "stdout:\n{stdout}");
}

#[test]
fn change_files_skips_malformed_and_keeps_valid() {
    let root = temp_git_repo("malformed");
    cargo_package(&root, "demo");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        versioned("change-files = true\nconventional-commits = false\n"),
    )
    .expect("config");
    fs::write(
        root.join(".changeset/good.md"),
        "---\ndemo: patch\n---\n\nok\n",
    )
    .expect("good");
    fs::write(root.join(".changeset/bad.md"), "not a bump file\n").expect("bad");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);

    let output = bin()
        .current_dir(&root)
        .args(["plan-intent"])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("bad.md"),
        "malformed file should be named on stderr: {err}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"id\": \"good.md\""), "stdout:\n{stdout}");
    assert!(stdout.contains("\"level\": \"patch\""), "stdout:\n{stdout}");
    assert!(
        !stdout.contains("bad.md"),
        "malformed file must not appear in plan JSON:\n{stdout}"
    );
}

#[test]
fn commits_only_path_fallback_feeds_plan() {
    let root = temp_git_repo("path-fallback");
    cargo_package(&root, "demo");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        versioned("change-files = false\nconventional-commits = true\n"),
    )
    .expect("config");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);
    let base = head_hash(&root);

    fs::write(root.join("src/lib.rs"), "// change\n").expect("edit");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "feat: unscoped bump"]);

    let output = bin()
        .current_dir(&root)
        .args(["plan-intent", "--from", &base])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"id\": \"commits\""), "stdout:\n{stdout}");
    assert!(stdout.contains("\"name\": \"demo\""), "stdout:\n{stdout}");
    assert!(stdout.contains("\"level\": \"minor\""), "stdout:\n{stdout}");
}

#[test]
fn change_files_missing_changeset_dir_is_empty_plan() {
    let root = temp_git_repo("no-changeset-dir");
    cargo_package(&root, "demo");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);
    // No `.changeset/` → default config (both on) → change-files plan → empty.

    let output = bin()
        .current_dir(&root)
        .args(["plan-intent"])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert_eq!(stdout, "[]", "stdout:\n{stdout}");
}

#[test]
fn change_files_empty_dir_is_empty_plan() {
    let root = temp_git_repo("empty-changeset");
    cargo_package(&root, "demo");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        versioned("change-files = true\nconventional-commits = false\n"),
    )
    .expect("config");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);

    let output = bin()
        .current_dir(&root)
        .args(["plan-intent"])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert_eq!(stdout, "[]", "stdout:\n{stdout}");
}

#[test]
fn change_files_sorted_by_filename() {
    let root = temp_git_repo("sort-order");
    cargo_package(&root, "demo");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        versioned("change-files = true\nconventional-commits = false\n"),
    )
    .expect("config");
    fs::write(
        root.join(".changeset/b-early.md"),
        "---\ndemo: patch\n---\n\nsecond alphabetically\n",
    )
    .expect("b");
    fs::write(
        root.join(".changeset/a-late.md"),
        "---\ndemo: minor\n---\n\nfirst alphabetically\n",
    )
    .expect("a");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);

    let output = bin()
        .current_dir(&root)
        .args(["plan-intent"])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let a = stdout
        .find("\"id\": \"a-late.md\"")
        .expect("a-late.md missing");
    let b = stdout
        .find("\"id\": \"b-early.md\"")
        .expect("b-early.md missing");
    assert!(a < b, "expected a-late.md before b-early.md:\n{stdout}");
}

#[test]
fn change_files_unknown_package_is_fatal() {
    let root = temp_git_repo("unknown-pkg");
    cargo_package(&root, "demo");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        versioned("change-files = true\nconventional-commits = false\n"),
    )
    .expect("config");
    fs::write(
        root.join(".changeset/good.md"),
        "---\ndemo: patch\n---\n\nok\n",
    )
    .expect("good");
    fs::write(
        root.join(".changeset/bad.md"),
        "---\nghost: major\n---\n\nunknown\n",
    )
    .expect("bad");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);

    let output = bin()
        .current_dir(&root)
        .args(["plan-intent"])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("ghost") || err.contains("unknown") || err.contains("not in"),
        "stderr should name the unknown package: {err}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("\"id\": \"good.md\""),
        "must not print a partial plan on abort:\n{stdout}"
    );
}

#[test]
fn mismatched_tool_version_still_prints_plan_intent() {
    let root = temp_git_repo("toolver");
    cargo_package(&root, "demo");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        "tool-version = \"9.9.9\"\nchange-files = true\nconventional-commits = false\n",
    )
    .expect("config");
    fs::write(
        root.join(".changeset/hand.md"),
        "---\ndemo: patch\n---\n\nhand written\n",
    )
    .expect("hand");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);

    let output = bin()
        .current_dir(&root)
        .args(["plan-intent"])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("hand.md"), "stdout:\n{stdout}");
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(!err.contains("upgrade"), "{err}");
}
