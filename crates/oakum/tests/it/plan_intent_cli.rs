//! Plan intent loader: change files vs commits-only (ADR-0029 / `okm-64b.5`).

use crate::support;

use std::fs;

use support::fixture::{
    cargo_package, git, git_repo, git_stdout, oakum, pin_oakum, Fixture, TOOL_VERSION_WARNING,
};

fn temp_git_repo(label: &str) -> Fixture {
    git_repo("intent", label)
}

fn head_hash(root: &std::path::Path) -> String {
    git_stdout(root, &["rev-parse", "HEAD"])
}

#[test]
fn commits_only_plan_intent_from_conventional_scope() {
    let root = temp_git_repo("commits-only");
    cargo_package(&root, "demo", "0.1.0");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        "change-files = false\nconventional-commits = true\n",
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

    let output = oakum(&root)
        .args(["plan-intent", "--from", &base])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let id = format!("\"id\": \"{}\"", head_hash(&root));
    assert!(stdout.contains(&id), "stdout:\n{stdout}");
    assert!(stdout.contains("\"name\": \"demo\""), "stdout:\n{stdout}");
    assert!(stdout.contains("\"level\": \"minor\""), "stdout:\n{stdout}");
    assert!(
        !stdout.contains("orphan") && !stdout.contains("major"),
        "orphan bump file must not feed commits-only plan:\n{stdout}"
    );
}

/// The JSON document is the answer; a reader who went away before it arrived
/// did not get one, so the run is unverified rather than ok.
#[test]
fn a_document_nobody_can_receive_is_not_success() {
    let root = temp_git_repo("dead-stdout");
    cargo_package(&root, "demo", "0.1.0");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        "change-files = false\nconventional-commits = true\n",
    )
    .expect("config");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);
    let base = head_hash(&root);
    let writer = support::dead_stdout();
    let output = oakum(&root)
        .args(["plan-intent", "--from", &base])
        .stdout(writer)
        .output()
        .expect("run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("report could not be delivered to stdout"),
        "{stderr}"
    );
}

#[test]
fn change_files_plan_intent_ignores_commits() {
    let root = temp_git_repo("files-only");
    cargo_package(&root, "demo", "0.1.0");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        "change-files = true\nconventional-commits = true\n",
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

    let output = oakum(&root)
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
        !stdout.contains(&head_hash(&root)) && !stdout.contains("minor"),
        "commits must not feed the plan when change-files is on:\n{stdout}"
    );
}

#[test]
fn both_intent_mechanisms_off_is_an_error() {
    let root = temp_git_repo("both-off");
    cargo_package(&root, "demo", "0.1.0");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        "change-files = false\nconventional-commits = false\n",
    )
    .expect("config");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);

    let output = oakum(&root)
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
    cargo_package(&root, "demo", "0.1.0");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        "change-files = true\nconventional-commits = false\n",
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

    let output = oakum(&root)
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
        !stdout.contains(&head_hash(&root)),
        "commits must not feed the plan:\n{stdout}"
    );
}

#[test]
fn commits_only_empty_range_is_empty_plan() {
    let root = temp_git_repo("empty-range");
    cargo_package(&root, "demo", "0.1.0");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        "change-files = false\nconventional-commits = true\n",
    )
    .expect("config");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);

    let output = oakum(&root)
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
        "change-files = false\nconventional-commits = true\n",
    )
    .expect("config");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);
    let base = head_hash(&root);

    fs::write(root.join("README.md"), "hi\n").expect("readme");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "docs: outside packages"]);

    let output = oakum(&root)
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
fn change_files_refuse_a_malformed_file_by_name() {
    let root = temp_git_repo("malformed");
    cargo_package(&root, "demo", "0.1.0");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        "change-files = true\nconventional-commits = false\n",
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

    let output = oakum(&root).args(["plan-intent"]).output().expect("run");
    assert!(
        !output.status.success(),
        "a malformed bump file must not produce a plan; stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("`bad.md` is not a bump file: bump file must start with --- on line 1"),
        "the malformed file is named with its parse error: {err}"
    );
    assert!(
        output.stdout.is_empty(),
        "no plan JSON beside a refusal: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn a_malformed_file_beside_an_unknown_package_names_both() {
    let root = temp_git_repo("malformed-and-unknown");
    cargo_package(&root, "demo", "0.1.0");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        "change-files = true\nconventional-commits = false\n",
    )
    .expect("config");
    fs::write(
        root.join(".changeset/ghost.md"),
        "---\nghost: major\n---\n\nno such package\n",
    )
    .expect("ghost");
    fs::write(root.join(".changeset/bad.md"), "not a bump file\n").expect("bad");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);

    let output = oakum(&root).args(["plan-intent"]).output().expect("run");
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("bump file `ghost.md` names package `ghost`, which is not in the workspace"),
        "{err}"
    );
    assert!(
        err.contains("; also `bad.md` is not a bump file: bump file must start with --- on line 1"),
        "the malformed file rides the unknown-package refusal in the same wording: {err}"
    );
}

#[test]
fn commits_only_path_fallback_feeds_plan() {
    let root = temp_git_repo("path-fallback");
    cargo_package(&root, "demo", "0.1.0");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        "change-files = false\nconventional-commits = true\n",
    )
    .expect("config");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);
    let base = head_hash(&root);

    fs::write(root.join("src/lib.rs"), "// change\n").expect("edit");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "feat: unscoped bump"]);

    let output = oakum(&root)
        .args(["plan-intent", "--from", &base])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let id = format!("\"id\": \"{}\"", head_hash(&root));
    assert!(stdout.contains(&id), "stdout:\n{stdout}");
    assert!(stdout.contains("\"name\": \"demo\""), "stdout:\n{stdout}");
    assert!(stdout.contains("\"level\": \"minor\""), "stdout:\n{stdout}");
}

#[test]
fn change_files_missing_changeset_dir_is_empty_plan() {
    let root = temp_git_repo("no-changeset-dir");
    cargo_package(&root, "demo", "0.1.0");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);
    // No `.changeset/` → default config (both on) → change-files plan → empty.
    // Hidden readers neither refuse on a missing config as `check` does
    // (ADR-0007) nor mention it.

    let output = oakum(&root).args(["plan-intent"]).output().expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "no defaults note on a hidden reader: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert_eq!(stdout, "[]", "stdout:\n{stdout}");
}

#[test]
fn change_files_empty_dir_is_empty_plan() {
    let root = temp_git_repo("empty-changeset");
    cargo_package(&root, "demo", "0.1.0");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        "change-files = true\nconventional-commits = false\n",
    )
    .expect("config");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);

    let output = oakum(&root).args(["plan-intent"]).output().expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert_eq!(stdout, "[]", "stdout:\n{stdout}");
}

#[test]
fn change_files_skips_directory_named_like_a_bump_file() {
    let root = temp_git_repo("dir-named-bump");
    cargo_package(&root, "demo", "0.1.0");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        "change-files = true\nconventional-commits = false\n",
    )
    .expect("config");
    fs::create_dir(root.join(".changeset/feat.md")).expect("directory named like a bump file");
    fs::write(
        root.join(".changeset/keep.md"),
        "---\ndemo: patch\n---\n\nkept beside a directory\n",
    )
    .expect("regular bump file");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);

    let output = oakum(&root).args(["plan-intent"]).output().expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"id\": \"keep.md\""), "stdout:\n{stdout}");
    assert!(
        !stdout.contains("\"id\": \"feat.md\""),
        "directory named like a bump file must not feed the plan:\n{stdout}"
    );
}

#[test]
fn change_files_sorted_by_filename() {
    let root = temp_git_repo("sort-order");
    cargo_package(&root, "demo", "0.1.0");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        "change-files = true\nconventional-commits = false\n",
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

    let output = oakum(&root).args(["plan-intent"]).output().expect("run");
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
    cargo_package(&root, "demo", "0.1.0");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        "change-files = true\nconventional-commits = false\n",
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

    let output = oakum(&root).args(["plan-intent"]).output().expect("run");
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
    cargo_package(&root, "demo", "0.1.0");
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

    let output = oakum(&root).args(["plan-intent"]).output().expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("hand.md"), "stdout:\n{stdout}");
    let err = String::from_utf8_lossy(&output.stderr);
    assert_eq!(err.trim_end(), TOOL_VERSION_WARNING);
}

/// A selection naming no package is refused here as it is by every later step,
/// so the reported intent never rests on a config they reject.
#[test]
fn an_include_naming_no_package_is_refused() {
    let root = temp_git_repo("unknown-include");
    cargo_package(&root, "demo", "0.1.0");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        "include = [\"nope\"]\n",
    )
    .expect("config");
    fs::write(
        root.join(".changeset/hand.md"),
        "---\ndemo: patch\n---\n\nfix\n",
    )
    .expect("bump file");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);

    let output = oakum(&root).args(["plan-intent"]).output().expect("run");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("nope"), "{stderr}");
    assert!(stderr.contains(".changeset/_config.toml"), "{stderr}");
    assert!(stderr.contains("the workspace has: demo"), "{stderr}");
    assert!(
        output.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

/// Commits-only intent leaves out a package the config does not version, so a
/// commit touching only it plans nothing instead of blocking `version`.
#[test]
fn commits_only_intent_leaves_out_an_unselected_package() {
    let root = temp_git_repo("commits-only-unselected");
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"alpha\", \"beta\"]\n",
    )
    .expect("workspace");
    for name in ["alpha", "beta"] {
        fs::create_dir_all(root.join(name).join("src")).expect("member");
        fs::write(
            root.join(name).join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )
        .expect("manifest");
        fs::write(root.join(name).join("src/lib.rs"), "").expect("lib.rs");
    }
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        "change-files = false\nconventional-commits = true\ninclude = [\"alpha\"]\n",
    )
    .expect("config");
    pin_oakum(&root);
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);
    let base = head_hash(&root);
    fs::write(root.join("beta/src/lib.rs"), "// change\n").expect("edit");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "feat(beta): unselected"]);

    let intent = oakum(&root)
        .args(["plan-intent", "--from", &base])
        .output()
        .expect("plan-intent");
    assert!(
        intent.status.success(),
        "{}",
        String::from_utf8_lossy(&intent.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&intent.stdout).contains("beta"),
        "{}",
        String::from_utf8_lossy(&intent.stdout)
    );

    let version = oakum(&root)
        .args(["version", "--from", &base])
        .output()
        .expect("version");
    assert!(
        version.status.success(),
        "{}",
        String::from_utf8_lossy(&version.stderr)
    );
}

fn commits_only_workspace(label: &str, members: &[&str]) -> Fixture {
    let root = temp_git_repo(label);
    let names = members
        .iter()
        .map(|name| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(", ");
    fs::write(
        root.join("Cargo.toml"),
        format!("[workspace]\nresolver = \"2\"\nmembers = [{names}]\n"),
    )
    .expect("workspace");
    for name in members {
        fs::create_dir_all(root.join(name).join("src")).expect("member");
        fs::write(
            root.join(name).join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )
        .expect("manifest");
        fs::write(root.join(name).join("src/lib.rs"), "").expect("lib.rs");
    }
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        "change-files = false\nconventional-commits = true\n",
    )
    .expect("config");
    pin_oakum(&root);
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);
    root
}

#[test]
fn commits_only_notes_stay_with_their_package() {
    let root = commits_only_workspace("notes-per-package", &["alpha", "beta"]);
    let base = head_hash(&root);
    fs::write(root.join("alpha/src/lib.rs"), "// a\n").expect("edit");
    // Not plan input with change files off, so `version` must leave it.
    fs::write(
        root.join(".changeset/notes.md"),
        "---\nalpha: major\n---\n\nx\n",
    )
    .expect("notes");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "fix(alpha): alpha thing"]);
    fs::write(root.join("beta/src/lib.rs"), "// b\n").expect("edit");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "feat(beta): beta thing"]);
    fs::write(root.join("beta/src/lib.rs"), "// b2\n").expect("edit");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "fix(beta): beta fix"]);

    let version = oakum(&root)
        .args(["version", "--from", &base])
        .output()
        .expect("version");
    assert!(
        version.status.success(),
        "{}",
        String::from_utf8_lossy(&version.stderr)
    );
    let alpha = fs::read_to_string(root.join("alpha/CHANGELOG.md")).expect("alpha changelog");
    let beta = fs::read_to_string(root.join("beta/CHANGELOG.md")).expect("beta changelog");
    assert!(
        alpha.contains("alpha thing") && !alpha.contains("beta thing"),
        "{alpha}"
    );
    assert!(
        beta.contains("beta thing") && !beta.contains("alpha thing"),
        "{beta}"
    );
    assert!(
        beta.contains("### Added\n\n- beta thing") && beta.contains("### Fixed\n\n- beta fix"),
        "{beta}"
    );
    assert!(root.join(".changeset/notes.md").is_file());
}

/// A Cargo crate and an npm package named alike in one directory both take an
/// unscoped commit there, as coverage already attributes it to both.
#[test]
fn commits_only_path_fallback_plans_both_packages_sharing_a_name() {
    let root = temp_git_repo("polyglot-same-name");
    cargo_package(&root, "foo", "0.1.0");
    fs::write(
        root.join("package.json"),
        "{\"name\": \"foo\", \"version\": \"0.1.0\"}\n",
    )
    .expect("package.json");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        "change-files = false\nconventional-commits = true\n",
    )
    .expect("config");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);
    let base = head_hash(&root);
    fs::write(root.join("src/lib.rs"), "// change\n").expect("edit");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "fix: shared change"]);

    let output = oakum(&root)
        .args(["plan-intent", "--from", &base])
        .output()
        .expect("run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("\"ecosystem\": \"cargo\""), "{stdout}");
    assert!(stdout.contains("\"ecosystem\": \"npm\""), "{stdout}");
}

/// A commit naming a versioned and an unversioned package names the second
/// even when its net change over the range is nothing, so the diff alone
/// would not show it.
#[test]
fn a_commit_that_also_names_an_unversioned_package_still_names_it() {
    let root = commits_only_workspace("mixed-commit-unversioned", &["alpha", "beta"]);
    fs::write(
        root.join("beta/Cargo.toml"),
        "[package]\nname = \"beta\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n",
    )
    .expect("private beta");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: beta is private"]);
    let base = head_hash(&root);
    fs::write(root.join("alpha/src/lib.rs"), "// a\n").expect("edit");
    fs::write(root.join("beta/src/lib.rs"), "// b\n").expect("edit");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "fix: both"]);
    fs::write(root.join("alpha/src/lib.rs"), "// a2\n").expect("edit");
    fs::write(root.join("beta/src/lib.rs"), "").expect("revert");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "fix: both again"]);

    let output = oakum(&root)
        .args(["status", "--json", "--from", &base])
        .output()
        .expect("status");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let state: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(
        state["named_unmanaged"],
        serde_json::json!([{"ecosystem": "cargo", "name": "beta"}]),
        "{stdout}"
    );
}

/// Only `feat`, `fix` and a breaking change release, as knope and
/// release-please read the convention; any other type covers what it touched.
#[test]
fn commits_only_plans_from_feat_fix_and_breaking_alone() {
    let root = commits_only_workspace("release-types", &["alpha", "beta", "gamma"]);
    let base = head_hash(&root);
    for (file, message) in [
        ("alpha/src/lib.rs", "docs(alpha): explain"),
        ("alpha/src/lib.rs", "chore(alpha): tidy"),
        ("beta/src/lib.rs", "chore(beta)!: drop the old api"),
        ("gamma/src/lib.rs", "ci: touch gamma"),
    ] {
        fs::write(root.join(file), format!("// {message}\n")).expect("edit");
        git(&root, &["add", "."]);
        git(&root, &["commit", "-m", message]);
    }

    let output = oakum(&root)
        .args(["plan-intent", "--from", &base])
        .output()
        .expect("plan-intent");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let files: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let asked: Vec<(String, String)> = files
        .as_array()
        .expect("files")
        .iter()
        .flat_map(|file| file["entries"].as_array().expect("entries").iter())
        .map(|entry| {
            (
                entry["name"].as_str().expect("name").to_owned(),
                entry["level"].as_str().expect("level").to_owned(),
            )
        })
        .collect();
    let pair = |name: &str, level: &str| (String::from(name), String::from(level));
    assert_eq!(
        asked,
        vec![
            pair("alpha", "none"),
            pair("alpha", "none"),
            pair("beta", "major"),
            pair("gamma", "none"),
        ],
        "{stdout}"
    );
}

/// A package changed only by commits that release nothing is covered, not
/// missing intent: the commit type is the decision not to release.
#[test]
fn a_package_changed_only_by_a_chore_is_covered() {
    let root = commits_only_workspace("chore-covers", &["alpha"]);
    let base = head_hash(&root);
    fs::write(root.join("alpha/src/lib.rs"), "// tidy\n").expect("edit");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore(alpha): tidy"]);

    let output = oakum(&root)
        .args(["check", "--strict", "--from", &base])
        .output()
        .expect("check");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("no covering intent"), "{stderr}");
    assert_ne!(output.status.code(), Some(1), "{stderr}");
}

/// A commit that releases nothing is not intent the config dropped, so routine
/// work on an unversioned package is not reported.
#[test]
fn a_non_releasing_commit_to_an_unversioned_package_is_not_named() {
    let root = commits_only_workspace("chore-unversioned", &["alpha", "beta"]);
    fs::write(
        root.join("beta/Cargo.toml"),
        "[package]\nname = \"beta\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n",
    )
    .expect("private beta");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: beta is private"]);
    let base = head_hash(&root);
    fs::write(root.join("README.md"), "beta\n").expect("readme");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore(beta): tidy"]);

    let output = oakum(&root)
        .args(["status", "--json", "--from", &base])
        .output()
        .expect("status");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let state: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(state["named_unmanaged"], serde_json::json!([]), "{stdout}");
}
