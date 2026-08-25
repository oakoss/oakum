//! `oakum add` binary: flag gating, workspace validation, and file write.

#![allow(clippy::disallowed_methods)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

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

fn temp_repo(label: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("oakum-add-{label}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp repo");
    // Stop `repo_root`'s upward `.git` walk inside the oakum checkout's target/.
    fs::create_dir(dir.join(".git")).expect("fixture .git");
    dir
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

#[test]
fn writes_empty_frontmatter() {
    let root = temp_repo("empty-fm");
    cargo_package(&root, "demo");

    let output = bin()
        .current_dir(&root)
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
    assert_eq!(body, "---\n---\ndocs only");
}

#[test]
fn writes_none_level_packages() {
    let root = temp_repo("none");
    cargo_package(&root, "demo");

    let output = bin()
        .current_dir(&root)
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
    assert_eq!(body, "---\ndemo: none\n---\ncovered");
}

#[test]
fn none_rejects_non_none_levels() {
    let root = temp_repo("none-bad");
    cargo_package(&root, "demo");

    let output = bin()
        .current_dir(&root)
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
    cargo_package(&root, "demo");

    let output = bin()
        .current_dir(&root)
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
    assert_eq!(body, "---\ndemo: none\n---\ncovered");
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
    cargo_package(&root, "demo");

    let output = bin()
        .current_dir(&root)
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
    assert_eq!(body, "---\ndemo: minor\n---\nAdds the add command.");
}

#[test]
fn slugifies_name_and_generates_default_stem() {
    let root = temp_repo("slug");
    cargo_package(&root, "demo");

    let named = bin()
        .current_dir(&root)
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

    let generated = bin()
        .current_dir(&root)
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
    cargo_package(&root, "demo");

    let output = bin()
        .current_dir(&root)
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
    cargo_package(&root, "demo");

    let output = bin()
        .current_dir(&root)
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
    cargo_package(&root, "demo");
    fs::create_dir_all(root.join(".changeset")).expect("changeset dir");
    fs::write(root.join(".changeset/taken.md"), "already\n").expect("seed");

    let output = bin()
        .current_dir(&root)
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

#[test]
fn malformed_packages_flag_is_an_error() {
    let root = temp_repo("malformed");
    cargo_package(&root, "demo");

    let output = bin()
        .current_dir(&root)
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
    cargo_package(&root, "demo");
    fs::create_dir_all(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/_config.toml"),
        "tool-version = \"9.9.9\"\n",
    )
    .expect("config");

    let output = bin()
        .current_dir(&root)
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
        cargo_package(&root, "demo");

        let output = bin()
            .current_dir(&root)
            .args(["add", "--packages", packages, "--message", "x"])
            .output()
            .expect("run");
        assert!(!output.status.success(), "{packages}");
        let err = String::from_utf8_lossy(&output.stderr);
        assert!(
            err.contains(needle) && err.contains("intersection"),
            "{packages}: {err}"
        );
        assert!(
            !root.join(".changeset").exists(),
            "{packages}: must not write a bump file"
        );
    }
}

#[test]
fn nothing_to_discover_is_an_error() {
    let root = temp_repo("empty");
    let output = bin()
        .current_dir(&root)
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
