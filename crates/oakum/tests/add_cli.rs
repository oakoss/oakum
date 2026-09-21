//! `oakum add` binary: flag gating, workspace validation, and file write.

#![allow(clippy::disallowed_methods)]

mod support;

use std::fs;
use std::process::{Command, Stdio};

use support::fixture::{cargo_package, oakum, plain_repo, Fixture};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oakum"))
}

fn temp_repo(label: &str) -> Fixture {
    let root = plain_repo("add", label);
    fs::create_dir(root.join(".git")).expect("fixture .git");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        format!("tool-version = \"{}\"\n", env!("CARGO_PKG_VERSION")),
    )
    .expect("config");
    root
}

#[test]
fn flagless_add_names_packages_and_interactive() {
    let output = bin().args(["add"]).output().expect("run oakum add");
    assert!(!output.status.success(), "flagless add must fail");
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("--packages")
            && err.contains("--interactive")
            && err.contains("--empty")
            && err.contains("--none"),
        "stderr should name the entry points, got: {err}"
    );
}

/// The path is what a caller captures. With nobody to receive it the write is
/// refused, and a command that wrote the file but could not say where must
/// not read as ok — the file exists, the exit code says the run did not finish,
/// and stderr names both. The reader is gone before the spawn so the first
/// write meets the refusal rather than racing it (`support::dead_stdout`).
#[test]
fn a_path_nobody_can_receive_is_not_success() {
    let root = temp_repo("dead-stdout");
    cargo_package(&root, "demo", "0.1.0");
    let writer = support::dead_stdout();
    let output = oakum(&root)
        .args([
            "add",
            "--packages",
            "demo:patch",
            "--message",
            "m",
            "--name",
            "dead",
        ])
        .stdout(writer)
        .stderr(Stdio::piped())
        .output()
        .expect("run");
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        root.join(".changeset/dead.md").is_file(),
        "the file was written"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr
            .contains("wrote `.changeset/dead.md`, but its path could not be delivered to stdout"),
        "{stderr}"
    );
}

#[test]
fn writes_empty_frontmatter() {
    let root = temp_repo("empty-fm");
    cargo_package(&root, "demo", "0.1.0");

    let output = oakum(&root)
        .args(["add", "--empty", "--message", "docs only", "--name", "docs"])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let path = root.join(".changeset/docs.md");
    let body = fs::read_to_string(&path).expect("read");
    assert_eq!(body, "---\n---\n\ndocs only\n");
}

#[test]
fn writes_none_level_packages() {
    let root = temp_repo("none");
    cargo_package(&root, "demo", "0.1.0");

    let output = oakum(&root)
        .args([
            "add",
            "--none",
            "--packages",
            "demo:none",
            "--message",
            "covered",
            "--name",
            "cover",
        ])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = fs::read_to_string(root.join(".changeset/cover.md")).expect("read");
    assert_eq!(body, "---\ndemo: none\n---\n\ncovered\n");
}

#[test]
fn none_rejects_non_none_levels() {
    let root = temp_repo("none-bad");
    cargo_package(&root, "demo", "0.1.0");

    let output = oakum(&root)
        .args(["add", "--none", "--packages", "demo:patch", "--name", "x"])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("--none") && err.contains("none"),
        "stderr: {err}"
    );
}

#[test]
fn none_without_packages_names_required_flag() {
    let output = bin().args(["add", "--none"]).output().expect("run");
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("--none") && err.contains("--packages") && err.contains("name:none"),
        "stderr: {err}"
    );
}

#[test]
fn packages_none_without_none_flag_writes_file() {
    let root = temp_repo("packages-none");
    cargo_package(&root, "demo", "0.1.0");

    let output = oakum(&root)
        .args([
            "add",
            "--packages",
            "demo:none",
            "--message",
            "covered",
            "--name",
            "cover",
        ])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = fs::read_to_string(root.join(".changeset/cover.md")).expect("read");
    assert_eq!(body, "---\ndemo: none\n---\n\ncovered\n");
}

#[test]
fn empty_conflicts_with_packages() {
    let output = bin()
        .args(["add", "--empty", "--packages", "demo:patch"])
        .output()
        .expect("run");
    assert!(!output.status.success());
}

#[test]
fn interactive_without_tty_names_packages_flags() {
    let output = bin()
        .args(["add", "--interactive"])
        .output()
        .expect("run oakum add --interactive");
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("--packages") && err.contains("terminal"),
        "stderr should name --packages and a terminal requirement, got: {err}"
    );
}

#[test]
fn interactive_conflicts_with_packages() {
    let output = bin()
        .args(["add", "--interactive", "--packages", "demo:patch"])
        .output()
        .expect("run oakum add");
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("cannot be used with") || err.contains("conflict"),
        "stderr should report a flag conflict, got: {err}"
    );
}

#[test]
fn writes_bump_file_for_workspace_package() {
    let root = temp_repo("write");
    cargo_package(&root, "demo", "0.1.0");

    let output = oakum(&root)
        .args([
            "add",
            "--packages",
            "demo:minor",
            "--message",
            "Adds the add command.",
            "--name",
            "adds-add",
        ])
        .output()
        .expect("run oakum add");
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let printed = String::from_utf8_lossy(&output.stdout);
    assert!(
        printed.contains(".changeset/adds-add.md"),
        "stdout path: {printed}"
    );

    let body = fs::read_to_string(root.join(".changeset/adds-add.md")).expect("read bump file");
    assert_eq!(body, "---\ndemo: minor\n---\n\nAdds the add command.\n");
}

#[test]
fn slugifies_name_and_generates_default_stem() {
    let root = temp_repo("slug");
    cargo_package(&root, "demo", "0.1.0");

    let named = oakum(&root)
        .args([
            "add",
            "--packages",
            "demo:patch",
            "--message",
            "x",
            "--name",
            "Hello World!!",
        ])
        .output()
        .expect("named");
    assert!(
        named.status.success(),
        "{}",
        String::from_utf8_lossy(&named.stderr)
    );
    assert!(root.join(".changeset/hello-world.md").is_file());

    let generated = oakum(&root)
        .args(["add", "--packages", "demo:patch", "--message", "y"])
        .output()
        .expect("generated");
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    let printed = String::from_utf8_lossy(&generated.stdout);
    let path = printed.trim();
    assert!(
        path.contains(".changeset/oakum-")
            && std::path::Path::new(path)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md")),
        "stdout: {printed}"
    );
}

#[test]
fn refuses_reserved_readme_stem() {
    let root = temp_repo("readme");
    cargo_package(&root, "demo", "0.1.0");

    let output = oakum(&root)
        .args([
            "add",
            "--packages",
            "demo:patch",
            "--message",
            "x",
            "--name",
            "README",
        ])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("reserved") || err.contains("instruction"),
        "stderr: {err}"
    );
    assert!(!root.join(".changeset/readme.md").exists());
}

#[test]
fn unknown_package_is_an_error() {
    let root = temp_repo("unknown");
    cargo_package(&root, "demo", "0.1.0");

    let output = oakum(&root)
        .args(["add", "--packages", "missing:patch", "--message", "x"])
        .output()
        .expect("run oakum add");
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("`missing`") && err.contains("not in the workspace"),
        "stderr: {err}"
    );
}

#[test]
fn refuses_to_overwrite_existing_bump_file() {
    let root = temp_repo("overwrite");
    cargo_package(&root, "demo", "0.1.0");
    fs::create_dir_all(root.join(".changeset")).expect("changeset dir");
    fs::write(root.join(".changeset/taken.md"), "already\n").expect("seed");

    let output = oakum(&root)
        .args([
            "add",
            "--packages",
            "demo:patch",
            "--message",
            "x",
            "--name",
            "taken",
        ])
        .output()
        .expect("run oakum add");
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("overwrite") || err.contains("existing"),
        "stderr: {err}"
    );
}

#[cfg(unix)]
#[test]
fn permission_denied_on_name_exists_is_not_overwrite() {
    use std::os::unix::fs::PermissionsExt;

    let root = temp_repo("exists-denied");
    cargo_package(&root, "demo", "0.1.0");
    let changeset = root.join(".changeset");
    fs::create_dir_all(&changeset).expect("changeset dir");
    fs::set_permissions(&changeset, fs::Permissions::from_mode(0o555)).expect("lock changeset");

    let output = oakum(&root)
        .args([
            "add",
            "--packages",
            "demo:patch",
            "--message",
            "x",
            "--name",
            "exists",
        ])
        .output()
        .expect("run oakum add");
    fs::set_permissions(&changeset, fs::Permissions::from_mode(0o755)).expect("unlock changeset");

    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        !err.contains("overwrite"),
        "permission failure must not look like an overwrite: {err}"
    );
    assert!(
        err.contains("Permission denied") || err.contains("failed to create"),
        "stderr: {err}"
    );
}

#[test]
fn malformed_packages_flag_is_an_error() {
    let root = temp_repo("malformed");
    cargo_package(&root, "demo", "0.1.0");

    let output = oakum(&root)
        .args(["add", "--packages", "core", "--message", "x"])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("name:level") || err.contains("missing"),
        "stderr: {err}"
    );
}

#[test]
fn tool_version_mismatch_refuses() {
    let root = temp_repo("toolver");
    cargo_package(&root, "demo", "0.1.0");
    fs::create_dir_all(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/_config.toml"),
        "tool-version = \"9.9.9\"\n",
    )
    .expect("config");

    let output = oakum(&root)
        .args([
            "add",
            "--packages",
            "demo:patch",
            "--message",
            "x",
            "--name",
            "tv",
        ])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("tool-version") && err.contains("upgrade"),
        "stderr: {err}"
    );
}

#[test]
fn yaml_coerced_package_name_is_refused() {
    for (label, packages, needle) in [
        ("yaml-yes", "yes:patch", "yes"),
        ("yaml-01", "01:patch", "01"),
        ("yaml-minus0", "-0:patch", "-0"),
    ] {
        let root = temp_repo(label);
        cargo_package(&root, "demo", "0.1.0");

        let output = oakum(&root)
            .args(["add", "--packages", packages, "--message", "x"])
            .output()
            .expect("run");
        assert!(!output.status.success(), "{packages}");
        let err = String::from_utf8_lossy(&output.stderr);
        assert!(
            err.contains(needle) && err.contains("intersection"),
            "{packages}: {err}"
        );
        let written: Vec<_> = root
            .join(".changeset")
            .read_dir()
            .expect("changeset")
            .map(|entry| entry.expect("entry").file_name())
            .filter(|name| name != "_config.toml")
            .collect();
        assert!(
            written.is_empty(),
            "{packages}: must not write a bump file: {written:?}"
        );
    }
}

#[test]
fn nothing_to_discover_is_an_error() {
    let root = temp_repo("empty");
    let output = oakum(&root)
        .args(["add", "--packages", "demo:patch", "--message", "x"])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("nothing to discover") || err.contains("discovery failed"),
        "stderr: {err}"
    );
}

#[test]
fn section_writes_the_heading_line_first() {
    let root = temp_repo("section");
    cargo_package(&root, "demo", "0.1.0");
    let output = oakum(&root)
        .args([
            "add",
            "--packages",
            "demo:patch",
            "--message",
            "Archived; no further releases.",
            "--section",
            "changed",
            "--name",
            "archive",
        ])
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = fs::read_to_string(root.join(".changeset/archive.md")).expect("read");
    assert_eq!(
        body,
        "---\ndemo: patch\n---\n\n### Changed\n\nArchived; no further releases.\n"
    );
}

#[test]
fn section_without_a_message_is_refused() {
    let root = temp_repo("section-no-message");
    cargo_package(&root, "demo", "0.1.0");
    let output = oakum(&root)
        .args(["add", "--packages", "demo:patch", "--section", "fixed"])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--message"), "{stderr}");
    assert!(
        !root.join(".changeset").exists()
            || root
                .join(".changeset")
                .read_dir()
                .unwrap()
                .all(|e| e.unwrap().file_name() == "_config.toml"),
        "nothing written"
    );
}

#[test]
fn section_conflicts_with_interactive() {
    let root = temp_repo("section-interactive");
    cargo_package(&root, "demo", "0.1.0");
    let output = oakum(&root)
        .args([
            "add",
            "--interactive",
            "--message",
            "x",
            "--section",
            "fixed",
        ])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--section") && stderr.contains("--interactive"),
        "the prompt has no section step, so the flag is refused, not dropped: {stderr}"
    );
}

#[test]
fn section_with_an_empty_message_is_refused() {
    let root = temp_repo("section-empty-message");
    cargo_package(&root, "demo", "0.1.0");
    let output = oakum(&root)
        .args([
            "add",
            "--packages",
            "demo:patch",
            "--message",
            "",
            "--section",
            "fixed",
        ])
        .output()
        .expect("run");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("non-empty `--message`"), "{stderr}");
    let written: Vec<_> = root
        .join(".changeset")
        .read_dir()
        .expect("changeset")
        .map(|entry| entry.expect("entry").file_name())
        .filter(|name| name != "_config.toml")
        .collect();
    assert!(written.is_empty(), "{written:?}");
}

/// A note that is a heading with nothing under it renders no changelog
/// section, so `version` would consume the file and the release would say
/// nothing about it. Caught where it is written: the author is still here, and
/// the alternative is finding out at release time or not at all.
#[test]
fn a_heading_with_no_body_is_refused_at_write_time() {
    let root = temp_repo("heading-only");
    cargo_package(&root, "demo", "0.1.0");

    let output = oakum(&root)
        .args(["add", "--packages", "demo:minor", "--message", "### Added"])
        .output()
        .expect("run oakum add");

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("heading with nothing under it"),
        "names what is wrong: {stderr}"
    );
    assert!(
        stderr.contains("--none"),
        "names the way to write a deliberately releaseless file: {stderr}"
    );
    // The highest level decides here too: `min` would read the `none` entry
    // and let the file through. Both packages must exist, or the refusal is
    // `validate_specs` rejecting an unknown name and proves nothing.
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"demo\", \"other\"]\n",
    )
    .expect("workspace");
    for member in ["demo", "other"] {
        let dir = root.join(member);
        fs::create_dir_all(dir.join("src")).expect("member src");
        fs::write(
            dir.join("Cargo.toml"),
            format!("[package]\nname = \"{member}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )
        .expect("member manifest");
        fs::write(dir.join("src/lib.rs"), "").expect("member lib");
    }
    let mixed = oakum(&root)
        .args([
            "add",
            "--packages",
            "other:none,demo:minor",
            "--message",
            "### Added",
        ])
        .output()
        .expect("run oakum add");
    assert_eq!(mixed.status.code(), Some(1), "{mixed:?}");
    assert!(
        String::from_utf8_lossy(&mixed.stderr).contains("heading with nothing under it"),
        "refused by the note guard, not by an unknown package: {mixed:?}"
    );

    let written: Vec<String> = fs::read_dir(root.join(".changeset"))
        .expect("changeset")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name != "_config.toml")
        .collect();
    assert!(written.is_empty(), "nothing was written: {written:?}");
}

/// Spaces render exactly what the empty message renders, so they take the
/// same path. Before this, `--section` refused `""` and accepted `"   "`, and
/// the second wrote a bump file whose note reached no changelog at all.
#[test]
fn a_whitespace_only_message_is_the_empty_message() {
    let root = temp_repo("whitespace-message");
    cargo_package(&root, "demo", "0.1.0");

    let sectioned = oakum(&root)
        .args([
            "add",
            "--packages",
            "demo:minor",
            "--section",
            "added",
            "--message",
            "   ",
        ])
        .output()
        .expect("run oakum add");
    assert_eq!(sectioned.status.code(), Some(1), "{sectioned:?}");
    assert!(
        String::from_utf8_lossy(&sectioned.stderr).contains("needs a non-empty `--message`"),
        "{sectioned:?}"
    );

    // Without `--section` it is a noteless bump file, which is a shape oakum
    // supports — written, and carrying no note rather than whitespace.
    let bare = oakum(&root)
        .args([
            "add",
            "--packages",
            "demo:minor",
            "--message",
            "  \t ",
            "--name",
            "bare",
        ])
        .output()
        .expect("run oakum add");
    assert_eq!(bare.status.code(), Some(0), "{bare:?}");
    let written = fs::read_to_string(root.join(".changeset/bare.md")).expect("bump file");
    assert_eq!(
        written.trim_end(),
        "---\ndemo: minor\n---",
        "no whitespace note trailing the frontmatter: {written:?}"
    );
}

/// The refusal must not fire on a file that renders, nor on a deliberately
/// releaseless one — a guard that rejects valid work gets deleted, not fixed.
#[test]
fn a_real_note_and_a_coverage_only_file_still_write() {
    let root = temp_repo("still-writes");
    cargo_package(&root, "demo", "0.1.0");

    for args in [
        [
            "add",
            "--packages",
            "demo:minor",
            "--message",
            "a real change",
        ]
        .as_slice(),
        [
            "add",
            "--none",
            "--packages",
            "demo:none",
            "--message",
            "### Added",
        ]
        .as_slice(),
    ] {
        let output = oakum(&root).args(args).output().expect("run oakum add");
        assert_eq!(
            output.status.code(),
            Some(0),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let written = fs::read_dir(root.join(".changeset"))
        .expect("changeset")
        .count();
    assert_eq!(written, 3, "both bump files landed beside the config");
}
