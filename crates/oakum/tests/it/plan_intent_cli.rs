//! Plan intent loader: change files vs commits-only (ADR-0029 / `okm-64b.5`).

use crate::support;

use std::fs;

use support::fixture::{
    cargo_package, git, git_repo, git_stdout, oakum, pin_oakum, plain_repo, Fixture,
    TOOL_VERSION_WARNING,
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
    assert_eq!(output.status.code(), Some(0), "{stderr}");
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

/// A commit asking for a release whose scope names no package, and whose
/// files are under none, `status` and `check` name rather than drop.
fn commit_with_unresolved_scope(label: &str, message: &str) -> (Fixture, String) {
    let root = commits_only_workspace(label, &["alpha", "beta"]);
    let base = head_hash(&root);
    fs::write(root.join("README.md"), "docs\n").expect("readme");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", message]);
    (root, base)
}

fn assert_scope_reported(label: &str, message: &str, scope: &str) {
    let (root, base) = commit_with_unresolved_scope(label, message);
    let commit = head_hash(&root);

    let output = oakum(&root)
        .args(["status", "--json", "--from", &base])
        .output()
        .expect("status");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    let state: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(
        state["unresolved_scopes"],
        serde_json::json!([{ "commit": commit, "scope": scope }]),
        "{stdout}"
    );

    let line = format!("`{scope}`, which names no workspace package");
    let output = oakum(&root)
        .args(["status", "--from", &base])
        .output()
        .expect("status");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains(&line), "{stdout}");

    let output = oakum(&root)
        .args(["check", "--strict", "--from", &base])
        .output()
        .expect("check");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(&line), "{stderr}");
    assert_eq!(output.status.code(), Some(0), "{stderr}");
}

#[test]
fn a_releasing_scope_naming_no_package_is_reported() {
    assert_scope_reported("scope-unknown", "fix(gamma): docs typo", "gamma");
}

#[test]
fn a_scope_naming_two_packages_is_reported_not_split() {
    assert_scope_reported("scope-two", "fix(alpha,beta): docs typo", "alpha,beta");
}

/// The paths name a package, so it is that package's report, not the scope's.
#[test]
fn an_unknown_scope_on_an_unversioned_package_names_the_package() {
    let root = commits_only_workspace("scope-unversioned", &["alpha", "beta"]);
    fs::write(
        root.join("beta/Cargo.toml"),
        "[package]\nname = \"beta\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n",
    )
    .expect("private beta");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: beta is private"]);
    let base = head_hash(&root);
    fs::write(root.join("beta/src/lib.rs"), "// fix\n").expect("edit");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "fix(gamma): a bug"]);

    let output = oakum(&root)
        .args(["status", "--json", "--from", &base])
        .output()
        .expect("status");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let state: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(
        state["unresolved_scopes"],
        serde_json::json!([]),
        "{stdout}"
    );
    assert_eq!(state["unmanaged"][0]["name"], "beta", "{stdout}");
}

/// Nothing to release is nothing dropped.
#[test]
fn a_non_releasing_scope_naming_no_package_is_not_reported() {
    let (root, base) = commit_with_unresolved_scope("scope-chore", "chore(gamma): tidy");

    let output = oakum(&root)
        .args(["status", "--json", "--from", &base])
        .output()
        .expect("status");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    let state: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(
        state["unresolved_scopes"],
        serde_json::json!([]),
        "{stdout}"
    );
}

/// The scope names nothing, but the paths do: the commit is attributed, so
/// nothing was dropped.
#[test]
fn an_unknown_scope_attributed_by_path_is_not_reported() {
    let root = commits_only_workspace("scope-by-path", &["alpha"]);
    let base = head_hash(&root);
    fs::write(root.join("alpha/src/lib.rs"), "// fix\n").expect("edit");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "fix(gamma): a bug"]);

    let output = oakum(&root)
        .args(["status", "--json", "--from", &base])
        .output()
        .expect("status");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let state: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(
        state["unresolved_scopes"],
        serde_json::json!([]),
        "{stdout}"
    );
    assert_eq!(state["packages"][0]["name"], "alpha", "{stdout}");
}

/// `version` consumes the intent, so it says what it could not release.
/// `ci version-pr` stops before any token is needed when nothing releases.
#[test]
fn version_names_a_release_reaching_no_package() {
    let (root, base) = commit_with_unresolved_scope("scope-version", "fix(gamma): docs typo");
    for verb in [["version"].as_slice(), ["ci", "version-pr"].as_slice()] {
        let args = [verb, &["--from", base.as_str()]].concat();
        let output = oakum(&root).args(&args).output().expect("run");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(0), "{args:?}: {stderr}");
        assert!(
            stderr.contains("`gamma`, which names no workspace package"),
            "{args:?}: {stderr}"
        );
    }
}

/// A merge's paths are not read, so its own release under an unknown scope
/// reaches nothing: here nothing on the merged branch releases either.
#[test]
fn a_merge_under_an_unknown_scope_is_reported() {
    let root = commits_only_workspace("scope-merge", &["alpha"]);
    let base = head_hash(&root);
    git(&root, &["checkout", "-b", "topic"]);
    fs::write(root.join("alpha/src/lib.rs"), "// docs\n").expect("edit");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "docs(alpha): explain"]);
    git(&root, &["checkout", "-"]);
    fs::write(root.join("README.md"), "docs\n").expect("readme");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "docs: readme"]);
    git(
        &root,
        &["merge", "--no-ff", "topic", "-m", "fix(gamma): merge"],
    );

    let output = oakum(&root)
        .args(["status", "--json", "--from", &base])
        .output()
        .expect("status");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let state: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(
        state["unresolved_scopes"],
        serde_json::json!([{ "commit": head_hash(&root), "scope": "gamma" }]),
        "{stdout}"
    );
    assert_eq!(state["packages"], serde_json::json!([]), "{stdout}");

    // The merge changed alpha's files, so the line claims nothing about paths.
    let output = oakum(&root)
        .args(["check", "--from", &base])
        .output()
        .expect("check");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(
            "asks for a release under scope `gamma`, which names no workspace package, so nothing releases from it"
        ),
        "{stderr}"
    );
    assert!(!stderr.contains("paths"), "{stderr}");
}

/// Which ecosystem releases is not guessed; when nothing releases there is
/// nothing to guess, so the paths decide.
#[test]
fn an_ambiguous_scope_refuses_only_a_release() {
    let root = temp_git_repo("scope-ambiguous");
    cargo_package(&root, "shared", "0.1.0");
    fs::write(
        root.join("package.json"),
        "{\"name\": \"shared\", \"version\": \"0.1.0\"}\n",
    )
    .expect("package.json");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        "change-files = false\nconventional-commits = true\n",
    )
    .expect("config");
    pin_oakum(&root);
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);
    let base = head_hash(&root);
    fs::write(root.join("README.md"), "docs\n").expect("readme");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore(shared): tidy"]);

    for verb in ["status", "check"] {
        let output = oakum(&root)
            .args([verb, "--from", &base])
            .output()
            .expect("run");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(0), "{verb}: {stderr}");
    }

    fs::write(root.join("README.md"), "fix\n").expect("readme");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "fix(shared): a bug"]);
    let output = oakum(&root)
        .args(["status", "--from", &base])
        .output()
        .expect("run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("more than one workspace package"),
        "{stderr}"
    );
}

/// Each commit the plan reads, with the packages it names, from `plan-intent`
/// with no `--from`.
fn read_without_from(root: &Fixture, from: Option<&str>) -> Vec<(String, Vec<String>)> {
    let mut args = vec!["plan-intent"];
    if let Some(from) = from {
        args.extend(["--from", from]);
    }
    let output = oakum(root).args(&args).output().expect("plan-intent");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let files: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    files
        .as_array()
        .expect("array")
        .iter()
        .map(|file| {
            let names = file["entries"]
                .as_array()
                .expect("entries")
                .iter()
                .map(|entry| String::from(entry["name"].as_str().expect("name")))
                .collect();
            (String::from(file["id"].as_str().expect("id")), names)
        })
        .collect()
}

fn commit_touching(root: &Fixture, package: &str, message: &str) -> String {
    fs::write(
        root.join(package).join("src/lib.rs"),
        format!("// {message}\n"),
    )
    .expect("edit");
    git(root, &["add", "."]);
    git(root, &["commit", "-m", message]);
    head_hash(root)
}

/// Two packages tagged at different commits, on `main`, with no `--from`: each
/// reads only the commits after its own tag.
#[test]
fn each_package_reads_from_its_own_tag() {
    let root = commits_only_workspace("since-own-tag", &["alpha", "beta"]);
    git(&root, &["tag", "alpha/v0.1.0"]);
    let early = commit_touching(&root, "beta", "fix(beta): before its tag");
    git(&root, &["tag", "beta/v0.1.0"]);
    let alpha = commit_touching(&root, "alpha", "fix(alpha): after its tag");
    let beta = commit_touching(&root, "beta", "fix(beta): after its tag");

    let read = read_without_from(&root, None);
    assert_eq!(
        read,
        [
            (alpha, vec![String::from("alpha")]),
            (beta, vec![String::from("beta")]),
        ],
        "{early} is behind beta's tag"
    );

    let output = oakum(&root)
        .args(["status", "--json"])
        .output()
        .expect("status");
    let state: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    let planned: Vec<&str> = state["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .map(|pkg| pkg["name"].as_str().expect("name"))
        .collect();
    assert_eq!(planned, ["alpha", "beta"], "{state}");
}

/// `--from` still reads one range for every package.
#[test]
fn an_explicit_from_reads_one_range() {
    let root = commits_only_workspace("explicit-from", &["alpha", "beta"]);
    let base = head_hash(&root);
    git(&root, &["tag", "alpha/v0.1.0"]);
    let early = commit_touching(&root, "beta", "fix(beta): before its tag");
    git(&root, &["tag", "beta/v0.1.0"]);

    let read = read_without_from(&root, Some(&base));
    assert_eq!(read, [(early, vec![String::from("beta")])]);
}

/// A package with no tag has never been released, so all of its history is
/// pending.
#[test]
fn a_never_tagged_package_reads_all_history() {
    let root = commits_only_workspace("never-tagged", &["alpha", "gamma"]);
    let gamma = commit_touching(&root, "gamma", "fix(gamma): long ago");
    git(&root, &["tag", "alpha/v0.1.0"]);
    let alpha = commit_touching(&root, "alpha", "fix(alpha): after its tag");

    let read = read_without_from(&root, None);
    assert!(
        read.contains(&(gamma, vec![String::from("gamma")])),
        "{read:?}"
    );
    assert!(
        read.contains(&(alpha.clone(), vec![String::from("alpha")])),
        "{read:?}"
    );
    let alpha_reads: Vec<&String> = read
        .iter()
        .filter(|(_, names)| names.contains(&String::from("alpha")))
        .map(|(id, _)| id)
        .collect();
    assert_eq!(alpha_reads, [&alpha], "{read:?}");
}

/// `version` released alpha and the tag is not there yet, as when the version
/// pull request has merged and `release` has not run: the same commits must not
/// plan the release again.
#[test]
fn a_package_awaiting_its_tag_plans_nothing() {
    let root = commits_only_workspace("awaiting-tag", &["alpha"]);
    git(&root, &["tag", "alpha/v0.1.0"]);
    commit_touching(&root, "alpha", "fix(alpha): a bug");
    let output = oakum(&root).arg("version").output().expect("version");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore(release): version packages"]);

    let output = oakum(&root)
        .args(["status", "--json"])
        .output()
        .expect("status");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    let state: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(state["packages"], serde_json::json!([]), "{stdout}");
    assert_eq!(
        state["awaiting_tag"],
        serde_json::json!([{ "name": "alpha", "ecosystem": "cargo" }]),
        "{stdout}"
    );
    let output = oakum(&root).arg("status").output().expect("status");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Released, awaiting its tag: alpha (`cargo`)"),
        "{stdout}"
    );

    // The plan waits for the tag; coverage reads the branch, so its own fix
    // still covers its change.
    git(&root, &["checkout", "-q", "-b", "feature"]);
    commit_touching(&root, "alpha", "fix(alpha): on a branch");
    assert_eq!(read_without_from(&root, None), []);
    let output = oakum(&root)
        .args(["check", "--strict"])
        .output()
        .expect("check");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("no covering intent"), "{stderr}");
    let output = oakum(&root).arg("status").output().expect("status");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(!stdout.contains("Uncovered"), "{stdout}");
    git(&root, &["checkout", "-q", "main"]);

    // Tagged, the range moves to the new tag.
    git(&root, &["tag", "alpha/v0.1.1"]);
    let next = commit_touching(&root, "alpha", "fix(alpha): the next bug");
    assert_eq!(
        read_without_from(&root, None),
        [(next, vec![String::from("alpha")])]
    );
}

/// While a package awaits its tag, a branch change no commit names for it is
/// still uncovered.
#[test]
fn an_uncovered_change_to_a_package_awaiting_its_tag_is_reported() {
    let root = commits_only_workspace("awaiting-uncovered", &["alpha", "beta"]);
    git(&root, &["tag", "alpha/v0.1.0"]);
    git(&root, &["tag", "beta/v0.1.0"]);
    commit_touching(&root, "alpha", "fix(alpha): a bug");
    let output = oakum(&root).arg("version").output().expect("version");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore(release): version packages"]);
    git(&root, &["checkout", "-q", "-b", "feature"]);
    commit_touching(&root, "alpha", "fix(beta): names beta, changes alpha");

    let output = oakum(&root)
        .args(["check", "--strict"])
        .output()
        .expect("check");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("alpha (cargo): changed with no covering intent"),
        "{stderr}"
    );
}

/// Nothing in history stands in for the tag: a remote ref that lags the
/// version commit, a release cut on a branch, a later edit to the changelog or
/// a second `version` before the first is committed all read nothing until
/// the tag lands.
#[test]
fn a_release_awaiting_its_tag_reads_nothing_until_tagged() {
    let root = commits_only_workspace("awaiting-lagging-base", &["alpha"]);
    git(&root, &["tag", "alpha/v0.1.0"]);
    git(&root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(&root, &["checkout", "-q", "-b", "release"]);
    commit_touching(&root, "alpha", "fix(alpha): released");
    let output = oakum(&root).arg("version").output().expect("version");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest = fs::read_to_string(root.join("alpha/Cargo.toml")).expect("manifest");

    // Written, not committed: a second run releases nothing more.
    let output = oakum(&root).arg("version").output().expect("version");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(root.join("alpha/Cargo.toml")).expect("manifest"),
        manifest
    );

    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore(release): version packages"]);
    assert_eq!(read_without_from(&root, None), []);

    let changelog = root.join("alpha/CHANGELOG.md");
    let text = fs::read_to_string(&changelog).expect("changelog");
    fs::write(&changelog, format!("{text}\n")).expect("edit");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "docs(alpha): tidy the changelog"]);
    assert_eq!(read_without_from(&root, None), []);
}

/// A tag oakum cannot attribute might be the package's release, so its range
/// is unknown rather than all of history.
#[test]
fn an_unattributable_tag_is_unverified() {
    let root = commits_only_workspace("leftover-tag", &["alpha"]);
    git(&root, &["tag", "nonesuch/v0.1.0"]);
    commit_touching(&root, "alpha", "fix(alpha): a bug");

    let output = oakum(&root)
        .arg("plan-intent")
        .output()
        .expect("plan-intent");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("unverified: leftover tag ambiguity"),
        "{stderr}"
    );
}

/// A package oakum versions but does not tag has no tag to read from, so it
/// reads this branch's commits alone, not all of history.
#[test]
fn an_untagged_versioned_package_reads_the_branch() {
    let root = commits_only_workspace("versioned-untagged", &["alpha", "beta"]);
    fs::write(
        root.join("beta/Cargo.toml"),
        "[package]\nname = \"beta\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n",
    )
    .expect("private beta");
    fs::write(
        root.join(".changeset/_config.toml"),
        "change-files = false\nconventional-commits = true\n\n[private-packages]\nversion = true\n",
    )
    .expect("config");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: beta is private"]);
    git(&root, &["tag", "alpha/v0.1.0"]);
    commit_touching(&root, "beta", "fix(beta): on main");
    git(&root, &["checkout", "-q", "-b", "feature"]);
    let branch = commit_touching(&root, "beta", "fix(beta): on the branch");

    assert_eq!(
        read_without_from(&root, None),
        [(branch, vec![String::from("beta")])]
    );
}

/// `version` never releases a package the config does not version, so one
/// tagged by hand above its tag is not waiting on `release`.
#[test]
fn a_tagged_unversioned_package_is_not_awaiting() {
    let root = commits_only_workspace("tagged-unversioned", &["alpha"]);
    fs::write(
        root.join("alpha/Cargo.toml"),
        "[package]\nname = \"alpha\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n",
    )
    .expect("private alpha");
    fs::write(
        root.join(".changeset/_config.toml"),
        "change-files = false\nconventional-commits = true\n\n[private-packages]\ntag = true\n",
    )
    .expect("config");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: alpha is private"]);
    git(&root, &["tag", "alpha/v0.1.0"]);
    fs::write(
        root.join("alpha/Cargo.toml"),
        "[package]\nname = \"alpha\"\nversion = \"0.1.1\"\nedition = \"2021\"\npublish = false\n",
    )
    .expect("hand bump");
    fs::write(
        root.join("alpha/CHANGELOG.md"),
        "# alpha\n\n## 0.1.1\n\n- hand\n",
    )
    .expect("log");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: hand release"]);

    let output = oakum(&root)
        .args(["status", "--json"])
        .output()
        .expect("status");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let state: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(state["awaiting_tag"], serde_json::json!([]), "{stdout}");
}

/// A shallow clone has no tags to read a range from, so the plan is a look
/// that could not happen, not an empty one.
#[test]
fn a_shallow_clone_is_unverified() {
    let root = commits_only_workspace("shallow-source", &["alpha"]);
    git(&root, &["tag", "alpha/v0.1.0"]);
    commit_touching(&root, "alpha", "fix(alpha): a bug");
    let parent = plain_repo("intent", "shallow-clone");
    let url = format!("file://{}", root.display());
    git(&parent, &["clone", "-q", "--depth", "1", &url, "clone"]);
    let clone = parent.join("clone");

    let output = oakum(&clone)
        .args(["status", "--json"])
        .output()
        .expect("status");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("shallow clone"), "{stderr}");
}

/// What changed is the branch's question and what is pending is the plan's:
/// `main`'s unreleased `fix(alpha)` covers alpha here, as a pending bump file
/// on `main` would with change files.
#[test]
fn check_reads_pending_intent_from_the_tag_and_changes_from_the_branch() {
    let root = commits_only_workspace("check-split", &["alpha", "beta"]);
    git(&root, &["tag", "alpha/v0.1.0"]);
    git(&root, &["tag", "beta/v0.1.0"]);
    commit_touching(&root, "alpha", "fix(alpha): on main");
    git(&root, &["checkout", "-q", "-b", "feature"]);
    commit_touching(&root, "alpha", "fix(beta): names beta, changes alpha");

    let output = oakum(&root)
        .args(["check", "--strict"])
        .output()
        .expect("check");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert!(!stderr.contains("no covering intent"), "{stderr}");
}
