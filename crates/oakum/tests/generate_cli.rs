//! `oakum generate` binary: commit scan, intent gate, and bump-file write.

#![allow(clippy::disallowed_methods)]

mod support;

use std::fs;
use std::process::Command;

use support::fixture::{git, git_env, git_repo, git_stdout, oakum, Fixture};

/// A config whose `tool-version` always matches the binary under test, so a
/// version bump cannot strand these fixtures behind the ADR-0007 gate.
fn versioned(rest: &str) -> String {
    format!("tool-version = \"{}\"\n{}", env!("CARGO_PKG_VERSION"), rest)
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

fn temp_git_repo(label: &str) -> Fixture {
    git_repo("generate", label)
}

fn head_hash(root: &std::path::Path) -> String {
    git_stdout(root, &["rev-parse", "HEAD"])
}

#[test]
fn generate_writes_from_conventional_scope() {
    let root = temp_git_repo("cc");
    cargo_package(&root, "demo");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);
    let base = head_hash(&root);

    fs::write(root.join("src/lib.rs"), "// change\n").expect("edit");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "feat(demo): add thing"]);

    let output = oakum(&root)
        .args(["generate", "--from", &base, "--name", "from-commits"])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = fs::read_to_string(root.join(".changeset/from-commits.md")).expect("read");
    assert!(
        body.contains("demo: minor"),
        "body should declare minor for demo, got:\n{body}"
    );
    assert!(
        body.contains("demo: add thing"),
        "body should include the commit summary, got:\n{body}"
    );
}

/// `--reverse` lists the range oldest-first and the note keeps that order —
/// a changelog reads history forward (measured mutant: dropping the flag
/// flips the note).
#[test]
fn the_note_reads_oldest_first() {
    let root = temp_git_repo("note-order");
    cargo_package(&root, "demo");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);
    let base = head_hash(&root);

    fs::write(root.join("src/lib.rs"), "// one\n").expect("edit");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "feat(demo): first thing"]);
    fs::write(root.join("src/lib.rs"), "// two\n").expect("edit");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "fix(demo): second thing"]);

    let output = oakum(&root)
        .args(["generate", "--from", &base, "--name", "ordered"])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = fs::read_to_string(root.join(".changeset/ordered.md")).expect("read");
    let first = body.find("demo: first thing").expect("first summary");
    let second = body.find("demo: second thing").expect("second summary");
    assert!(
        first < second,
        "the note must read oldest-first, got:\n{body}"
    );
}

/// `--from <base>..HEAD` is the branch's own commits: a base that advanced
/// after the branch point must not leak its commits into the note, which the
/// symmetric three-dot range would (measured mutant).
#[test]
fn commits_on_the_advanced_base_stay_out_of_the_note() {
    let root = temp_git_repo("range-two-dot");
    cargo_package(&root, "demo");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);

    git(&root, &["switch", "-c", "feature"]);
    fs::write(root.join("src/lib.rs"), "// branch\n").expect("edit");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "feat(demo): branch thing"]);

    git(&root, &["switch", "main"]);
    fs::write(root.join("src/base.rs"), "// base\n").expect("edit");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "feat(demo): main-only thing"]);
    git(&root, &["switch", "feature"]);

    let output = oakum(&root)
        .args(["generate", "--from", "main", "--name", "ranged"])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = fs::read_to_string(root.join(".changeset/ranged.md")).expect("read");
    assert!(body.contains("demo: branch thing"), "{body}");
    assert!(
        !body.contains("main-only thing"),
        "the advanced base's commit must stay out: {body}"
    );
}

#[test]
fn dry_run_writes_nothing() {
    let root = temp_git_repo("dry");
    cargo_package(&root, "demo");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);
    let base = head_hash(&root);

    fs::write(root.join("src/lib.rs"), "// x\n").expect("edit");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "fix(demo): bug"]);

    let output = oakum(&root)
        .args(["generate", "--from", &base, "--dry-run"])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !root.join(".changeset").exists()
            || root.join(".changeset").read_dir().unwrap().next().is_none()
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("demo: patch"), "dry-run stdout:\n{stdout}");
}

#[test]
fn refuses_when_conventional_commits_disabled() {
    let root = temp_git_repo("gate");
    cargo_package(&root, "demo");
    fs::create_dir_all(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/_config.toml"),
        versioned("change-files = true\nconventional-commits = false\n"),
    )
    .expect("config");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);

    let output = oakum(&root)
        .args(["generate", "--from", "HEAD"])
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
fn refuses_when_change_files_disabled() {
    let root = temp_git_repo("gate-files");
    cargo_package(&root, "demo");
    fs::create_dir_all(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/_config.toml"),
        versioned("change-files = false\nconventional-commits = true\n"),
    )
    .expect("config");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);

    let output = oakum(&root)
        .args(["generate", "--from", "HEAD"])
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
fn path_fallback_for_plain_message() {
    let root = temp_git_repo("paths");
    cargo_package(&root, "demo");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);
    let base = head_hash(&root);

    fs::write(root.join("src/lib.rs"), "// y\n").expect("edit");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "tweak implementation"]);

    let output = oakum(&root)
        .args(["generate", "--from", &base, "--name", "paths"])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = fs::read_to_string(root.join(".changeset/paths.md")).expect("read");
    assert!(body.contains("demo: patch"), "body:\n{body}");
}

#[test]
fn path_fallback_preserves_unscoped_feat_level() {
    let root = temp_git_repo("unscoped-feat");
    cargo_package(&root, "demo");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);
    let base = head_hash(&root);

    fs::write(root.join("src/lib.rs"), "// feat\n").expect("edit");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "feat: add thing"]);

    let output = oakum(&root)
        .args(["generate", "--from", &base, "--name", "unscoped"])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = fs::read_to_string(root.join(".changeset/unscoped.md")).expect("read");
    assert!(
        body.contains("demo: minor"),
        "unscoped feat must keep minor via path fallback, got:\n{body}"
    );
}

fn cargo_workspace_member(root: &std::path::Path, member: &str, name: &str) {
    fs::write(
        root.join("Cargo.toml"),
        format!("[workspace]\nmembers = [\"{member}\"]\n"),
    )
    .expect("root Cargo.toml");
    let dir = root.join(member);
    fs::create_dir_all(dir.join("src")).expect("member src");
    fs::write(
        dir.join("Cargo.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
    )
    .expect("member Cargo.toml");
    fs::write(dir.join("src/lib.rs"), "").expect("lib.rs");
}

// Non-ASCII triggers git's default core.quotePath quoting; the quoted form
// starts with a double quote and stops prefix-matching the package
// directory. Only `-z` output carries the path byte-for-byte.
#[test]
fn path_fallback_attributes_quoted_unicode_paths() {
    let root = temp_git_repo("quoted");
    cargo_workspace_member(&root, "crates/demo", "demo");
    // Pin the default so the regression guard discriminates even on a
    // machine whose global config disables quoting.
    git(&root, &["config", "core.quotePath", "true"]);
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);
    let base = head_hash(&root);

    fs::write(
        root.join("crates/demo/src/na\u{ef}ve module.rs"),
        "// \u{e9}\n",
    )
    .expect("unicode file");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "tweak unicode handling"]);

    let output = oakum(&root)
        .args(["generate", "--from", &base, "--name", "quoted"])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = fs::read_to_string(root.join(".changeset/quoted.md")).expect("read");
    assert!(
        body.contains("demo: patch"),
        "a quoted unicode path must still attribute to demo, got:\n{body}"
    );
}

#[cfg(unix)]
#[test]
fn path_fallback_attributes_newline_and_whitespace_paths() {
    let root = temp_git_repo("weird-paths");
    cargo_workspace_member(&root, "crates/demo", "demo");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);
    let base = head_hash(&root);

    fs::write(root.join("crates/demo/src/we\nird.txt"), "x").expect("newline file");
    fs::write(root.join("crates/demo/src/trail .txt"), "x").expect("trailing-space file");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "tweak odd filenames"]);

    let output = oakum(&root)
        .args(["generate", "--from", &base, "--name", "weird"])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = fs::read_to_string(root.join(".changeset/weird.md")).expect("read");
    assert!(
        body.contains("demo: patch"),
        "newline and whitespace paths must attribute to demo, got:\n{body}"
    );
}

// diff-tree without -m already emits nothing for merges, which is exactly
// why this pin exists: the parent-count guard looks redundant until someone
// adds -m to "fix" merge handling, at which point base-branch-only files
// would be credited to packages.
#[test]
fn merge_commits_are_excluded_from_path_attribution() {
    let root = temp_git_repo("merge-excluded");
    cargo_workspace_member(&root, "crates/demo", "demo");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);

    git(&root, &["checkout", "-b", "feature"]);
    fs::write(root.join("README.md"), "readme\n").expect("readme");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "describe the project"]);

    git(&root, &["checkout", "main"]);
    fs::write(root.join("crates/demo/src/lib.rs"), "// main\n").expect("edit");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore(demo): housekeeping"]);

    git(&root, &["checkout", "feature"]);
    git(&root, &["merge", "--no-ff", "--no-edit", "main"]);

    // `main..HEAD` holds only the feature commit and the merge itself —
    // main's demo commit is excluded as reachable from main, so the only way
    // demo could be attributed is the merge commit's own diff.
    let output = oakum(&root)
        .args(["generate", "--from", "main", "--name", "merged"])
        .output()
        .expect("run");
    assert!(
        !output.status.success(),
        "the merge must not path-attribute demo's files: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("no package bumps"), "stderr: {err}");
}

#[cfg(target_os = "linux")]
#[test]
fn non_utf8_path_fails_loudly_end_to_end() {
    use std::os::unix::ffi::OsStrExt;

    let root = temp_git_repo("non-utf8");
    cargo_workspace_member(&root, "crates/demo", "demo");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);
    let base = head_hash(&root);

    let name = std::ffi::OsStr::from_bytes(b"\xff.bin");
    fs::write(root.join("crates/demo/src").join(name), "x").expect("non-utf8 file");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "tweak binary asset"]);

    let output = oakum(&root)
        .args(["generate", "--from", &base, "--name", "nonutf8"])
        .output()
        .expect("run");
    assert!(!output.status.success(), "non-UTF-8 path must fail loudly");
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("not valid UTF-8"), "stderr: {err}");
}

#[test]
fn multi_commit_highest_wins_in_cli() {
    let root = temp_git_repo("multi");
    cargo_package(&root, "demo");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);
    let base = head_hash(&root);

    fs::write(root.join("src/lib.rs"), "// a\n").expect("edit");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "fix(demo): a"]);

    fs::write(root.join("src/lib.rs"), "// b\n").expect("edit");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "feat(demo): b"]);

    let output = oakum(&root)
        .args(["generate", "--from", &base, "--name", "multi"])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = fs::read_to_string(root.join(".changeset/multi.md")).expect("read");
    assert!(body.contains("demo: minor"), "body:\n{body}");
    assert!(body.contains("demo: a"), "body:\n{body}");
    assert!(body.contains("demo: b"), "body:\n{body}");
}

#[test]
fn empty_intent_errors() {
    let root = temp_git_repo("empty");
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
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "chore: initial"]);
    let base = head_hash(&root);

    fs::write(root.join("README.md"), "hi\n").expect("readme");
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "docs: outside packages"]);

    let output = oakum(&root)
        .args(["generate", "--from", &base])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("no package bumps"), "stderr: {err}");
}

#[test]
fn tool_version_mismatch_refuses() {
    let root = temp_git_repo("toolver");
    cargo_package(&root, "demo");
    fs::create_dir_all(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/_config.toml"),
        "tool-version = \"9.9.9\"\n",
    )
    .expect("config");

    let output = oakum(&root)
        .args(["generate", "--name", "tv-gen"])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("tool-version") && err.contains("upgrade"),
        "stderr: {err}"
    );
}

/// Git does not promise a commit message is UTF-8. Its own `commit` transcodes
/// from the locale, so the raw bytes only arrive from a commit written verbatim
/// — an import, another tool, or a non-UTF-8 locale — and `git log` passes them
/// straight through. Reading the log must not fail on one stray byte.
#[test]
fn a_commit_message_that_is_not_utf8_is_still_read() {
    let root = temp_git_repo("non-utf8-message");
    cargo_package(&root, "demo");
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "--no-verify", "-m", "init"]);
    git(&root, &["tag", "v0.1.0"]);

    let tree = git_stdout(&root, &["write-tree"]);
    let parent = git_stdout(&root, &["rev-parse", "HEAD"]);
    // `commit-tree` still transcodes; only a verbatim object keeps the byte.
    let mut object = format!(
        "tree {tree}\nparent {parent}\nauthor T <t@e.com> 1700000000 +0000\n\
         committer T <t@e.com> 1700000000 +0000\n\nfeat(demo): raw "
    )
    .into_bytes();
    object.push(0xff);
    object.extend_from_slice(b" byte\n");

    let mut hash_object = Command::new("git");
    git_env(&mut hash_object, &root);
    let mut child = hash_object
        .args([
            "hash-object",
            "-t",
            "commit",
            "-w",
            "--literally",
            "--stdin",
        ])
        .current_dir(&root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("hash-object");
    std::io::Write::write_all(child.stdin.as_mut().expect("stdin"), &object).expect("write object");
    let written = child.wait_with_output().expect("hash-object");
    assert!(written.status.success(), "hash-object failed");
    let sha = String::from_utf8(written.stdout).expect("utf-8");
    git(&root, &["update-ref", "refs/heads/main", sha.trim()]);

    let out = oakum(&root)
        .args(["generate", "--from", "v0.1.0"])
        .output()
        .expect("generate");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "one stray byte must not stop the log being read: {stderr}"
    );
    let written = fs::read_dir(root.join(".changeset"))
        .expect("changeset dir")
        .filter_map(|entry| fs::read_to_string(entry.ok()?.path()).ok())
        .collect::<String>();
    assert!(
        written.contains("raw \u{fffd} byte"),
        "the undecodable byte should be replaced, not dropped: {written}"
    );
}
