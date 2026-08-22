//! `_config.toml` at the CLI: schema refusals and missing-file defaults.

#![allow(clippy::disallowed_methods)]

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oakum"))
}

fn cargo_package(root: &Path, name: &str) {
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
        .join(format!("oakum-config-{label}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp repo");
    fs::create_dir(dir.join(".git")).expect("fixture .git");
    dir
}

fn write_config(root: &Path, body: &str) {
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    fs::write(root.join(".changeset/_config.toml"), body).expect("config");
}

fn add_demo_command(root: &Path) -> Command {
    let mut command = bin();
    command.current_dir(root).args([
        "add",
        "--packages",
        "demo:patch",
        "--message",
        "x",
        "--name",
        "cfg",
    ]);
    command
}

fn add_demo(root: &Path) -> std::process::Output {
    add_demo_command(root).output().expect("oakum add")
}

fn add_demo_with_deadline(root: &Path) -> (std::process::ExitStatus, String) {
    let mut child = add_demo_command(root)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("oakum add");
    let deadline = Instant::now() + Duration::from_secs(2);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll oakum add") {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill blocked oakum add");
            child.wait().expect("reap oakum add");
            panic!("oakum blocked while opening config");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut err = String::new();
    child
        .stderr
        .take()
        .expect("stderr")
        .read_to_string(&mut err)
        .expect("read stderr");
    (status, err)
}

#[test]
fn unknown_config_key_refuses() {
    let root = temp_repo("unknown");
    cargo_package(&root, "demo");
    write_config(&root, "tool-version = \"0.0.0\"\ngit-user = \"bot\"\n");

    let output = add_demo(&root);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("unknown configuration key")
            && err.contains("not a valid oakum config")
            && !err.contains("git-user"),
        "stderr: {err}"
    );
}

#[test]
fn snake_case_key_refuses() {
    let root = temp_repo("snake");
    cargo_package(&root, "demo");
    write_config(&root, "tool-version = \"0.0.0\"\nchange_files = false\n");

    let output = add_demo(&root);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("unknown configuration key"), "stderr: {err}");
    assert!(!err.contains("change_files"), "stderr: {err}");
}

#[test]
fn known_preference_keys_load() {
    let root = temp_repo("known");
    cargo_package(&root, "demo");
    write_config(
        &root,
        r#"
tool-version = "0.0.0"
versioning = "zero-major"
pr-status = "both"
tag-format = "v{{version}}"
commit-message = "chore: release {{version}}"
title = "Release"
template = "keep"

[packages.demo]
versioning = "semver"
resolves-dependencies-at = "build"
"#,
    );

    let output = add_demo(&root);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(root.join(".changeset/cfg.md").is_file());
}

#[test]
fn missing_tool_version_refuses() {
    let root = temp_repo("no-tool-version");
    cargo_package(&root, "demo");
    write_config(&root, "change-files = true\n");

    let output = add_demo(&root);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("tool-version") && err.contains("not a valid oakum config"),
        "stderr: {err}"
    );
}

#[test]
fn invalid_enum_value_refuses() {
    let root = temp_repo("bad-enum");
    cargo_package(&root, "demo");
    write_config(&root, "tool-version = \"0.0.0\"\npr-status = \"checks\"\n");

    let output = add_demo(&root);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("not a valid oakum config")
            && err.contains("invalid configuration value")
            && err.contains("line 2, column 13")
            && !err.contains("checks"),
        "stderr: {err}"
    );
}

#[test]
fn unknown_package_key_refuses() {
    let root = temp_repo("pkg-unknown");
    cargo_package(&root, "demo");
    write_config(
        &root,
        "tool-version = \"0.0.0\"\n\n[packages.demo]\npublish = true\n",
    );

    let output = add_demo(&root);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("unknown configuration key")
            && err.contains("not a valid oakum config")
            && !err.contains("publish"),
        "stderr: {err}"
    );
}

#[test]
fn tool_version_range_refuses() {
    let root = temp_repo("version-range");
    cargo_package(&root, "demo");
    write_config(&root, "tool-version = \"^0.0.0\"\n");

    let output = add_demo(&root);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("exact version") && err.contains("not a valid oakum config"),
        "stderr: {err}"
    );
}

#[test]
fn unknown_config_key_does_not_echo_source_lines() {
    let root = temp_repo("redacted-parse-error");
    cargo_package(&root, "demo");
    write_config(
        &root,
        "tool-version = \"0.0.0\"\nsecret = \"do-not-print-this-value\"\n",
    );

    let output = add_demo(&root);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("_config.toml"), "stderr: {err}");
    assert!(err.contains("line 2, column 1"), "stderr: {err}");
    assert!(err.contains("unknown configuration key"), "stderr: {err}");
    assert!(!err.contains("secret"), "stderr: {err}");
    assert!(!err.contains("do-not-print-this-value"), "stderr: {err}");
}

#[test]
fn malformed_toml_does_not_echo_source_lines() {
    let root = temp_repo("redacted-syntax-error");
    cargo_package(&root, "demo");
    write_config(
        &root,
        "tool-version = \"0.0.0\"\ntitle = \"do-not-print-this-value\n",
    );

    let output = add_demo(&root);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("line 2, column 33"), "stderr: {err}");
    assert!(err.contains("invalid TOML syntax"), "stderr: {err}");
    assert!(!err.contains("do-not-print-this-value"), "stderr: {err}");
}

#[test]
fn invalid_config_value_is_redacted() {
    let root = temp_repo("redacted-value");
    cargo_package(&root, "demo");
    write_config(
        &root,
        "tool-version = \"0.0.0\"\npr-status = \"do-not-print-this-value\"\n",
    );

    let output = add_demo(&root);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("invalid configuration value"), "stderr: {err}");
    assert!(err.contains("line 2, column 13"), "stderr: {err}");
    assert!(!err.contains("do-not-print-this-value"), "stderr: {err}");
}

#[test]
fn invalid_config_type_is_redacted() {
    let root = temp_repo("redacted-type");
    cargo_package(&root, "demo");
    write_config(
        &root,
        "tool-version = \"0.0.0\"\nchange-files = \"type-value-must-not-print\"\n",
    );

    let output = add_demo(&root);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("invalid configuration value"), "stderr: {err}");
    assert!(err.contains("line 2, column 16"), "stderr: {err}");
    assert!(!err.contains("type-value-must-not-print"), "stderr: {err}");
}

#[cfg(unix)]
#[test]
fn config_symlink_outside_repository_refuses_without_reading_source() {
    use std::os::unix::fs::symlink;

    let root = temp_repo("external-symlink");
    cargo_package(&root, "demo");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    let external =
        root.with_file_name(format!("oakum-external-config-{}.toml", std::process::id()));
    fs::write(
        &external,
        "secret = \"external-source-must-not-be-printed\"\n",
    )
    .expect("external config");
    symlink(&external, root.join(".changeset/_config.toml")).expect("config symlink");

    let output = add_demo(&root);
    fs::remove_file(&external).expect("remove external config");
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("outside the repository"), "stderr: {err}");
    assert!(
        !err.contains("external-source-must-not-be-printed"),
        "stderr: {err}"
    );
}

#[cfg(unix)]
#[test]
fn relative_config_symlink_outside_repository_refuses_without_reading_source() {
    use std::os::unix::fs::symlink;

    let root = temp_repo("relative-external-symlink");
    cargo_package(&root, "demo");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    let external = root.with_file_name(format!(
        "oakum-relative-external-config-{}.toml",
        std::process::id()
    ));
    fs::write(
        &external,
        "secret = \"relative-external-source-must-not-be-printed\"\n",
    )
    .expect("external config");
    let target = PathBuf::from("../..").join(external.file_name().expect("external file name"));
    symlink(target, root.join(".changeset/_config.toml")).expect("config symlink");

    let output = add_demo(&root);
    fs::remove_file(&external).expect("remove external config");
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("repository"), "stderr: {err}");
    assert!(
        !err.contains("relative-external-source-must-not-be-printed"),
        "stderr: {err}"
    );
}

#[cfg(unix)]
#[test]
fn changeset_symlink_outside_repository_refuses() {
    use std::os::unix::fs::symlink;

    let root = temp_repo("external-changeset");
    cargo_package(&root, "demo");
    let external = root.with_file_name(format!("oakum-external-changeset-{}", std::process::id()));
    let _ = fs::remove_dir_all(&external);
    fs::create_dir(&external).expect("external changeset");
    fs::write(
        external.join("_config.toml"),
        "secret = \"ancestor-source-must-not-be-printed\"\n",
    )
    .expect("external config");
    symlink(&external, root.join(".changeset")).expect("changeset symlink");

    let output = add_demo(&root);
    fs::remove_dir_all(&external).expect("remove external changeset");
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("repository"), "stderr: {err}");
    assert!(
        !err.contains("ancestor-source-must-not-be-printed"),
        "stderr: {err}"
    );
}

#[cfg(unix)]
#[test]
fn relative_changeset_symlink_outside_repository_refuses() {
    use std::os::unix::fs::symlink;

    let root = temp_repo("relative-external-changeset");
    cargo_package(&root, "demo");
    let external = root.with_file_name(format!(
        "oakum-relative-external-changeset-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&external);
    fs::create_dir(&external).expect("external changeset");
    fs::write(
        external.join("_config.toml"),
        "secret = \"relative-ancestor-source-must-not-be-printed\"\n",
    )
    .expect("external config");
    let target = PathBuf::from("..").join(external.file_name().expect("external directory name"));
    symlink(target, root.join(".changeset")).expect("changeset symlink");

    let output = add_demo(&root);
    fs::remove_dir_all(&external).expect("remove external changeset");
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("repository"), "stderr: {err}");
    assert!(
        !err.contains("relative-ancestor-source-must-not-be-printed"),
        "stderr: {err}"
    );
}

#[cfg(unix)]
#[test]
fn external_directory_is_rejected_before_file_validation() {
    use std::os::unix::fs::symlink;

    let root = temp_repo("external-directory");
    cargo_package(&root, "demo");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    let external = root.with_file_name(format!(
        "oakum-external-config-directory-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&external);
    fs::create_dir(&external).expect("external directory");
    symlink(&external, root.join(".changeset/_config.toml")).expect("config symlink");

    let output = add_demo(&root);
    fs::remove_dir(&external).expect("remove external directory");
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("outside the repository"), "stderr: {err}");
    assert!(!err.contains("regular file"), "stderr: {err}");
}

#[cfg(unix)]
#[test]
fn config_symlink_to_regular_file_inside_repository_loads() {
    use std::os::unix::fs::symlink;

    let root = temp_repo("internal-symlink");
    cargo_package(&root, "demo");
    let config_dir = root.join(".changeset/config");
    fs::create_dir_all(&config_dir).expect("config directory");
    fs::write(
        config_dir.join("oakum.toml"),
        format!("tool-version = \"{}\"\n", env!("CARGO_PKG_VERSION")),
    )
    .expect("config");
    symlink("config/oakum.toml", root.join(".changeset/_config.toml")).expect("config symlink");

    let output = add_demo(&root);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(root.join(".changeset/cfg.md").is_file());
}

#[cfg(unix)]
#[test]
fn changeset_symlink_to_directory_inside_repository_loads() {
    use std::os::unix::fs::symlink;

    let root = temp_repo("internal-changeset-symlink");
    cargo_package(&root, "demo");
    let changeset = root.join("config/changeset");
    fs::create_dir_all(&changeset).expect("changeset target");
    fs::write(
        changeset.join("_config.toml"),
        format!("tool-version = \"{}\"\n", env!("CARGO_PKG_VERSION")),
    )
    .expect("config");
    symlink("config/changeset", root.join(".changeset")).expect("changeset symlink");

    let output = add_demo(&root);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(changeset.join("cfg.md").is_file());
}

#[cfg(unix)]
#[test]
fn dangling_config_symlink_is_not_missing_config() {
    use std::os::unix::fs::symlink;

    let root = temp_repo("dangling-symlink");
    cargo_package(&root, "demo");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    symlink("missing-config.toml", root.join(".changeset/_config.toml")).expect("config symlink");

    let output = add_demo(&root);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("failed to"), "stderr: {err}");
}

#[cfg(unix)]
#[test]
fn config_symlink_to_directory_inside_repository_refuses() {
    use std::os::unix::fs::symlink;

    let root = temp_repo("internal-directory-symlink");
    cargo_package(&root, "demo");
    fs::create_dir_all(root.join(".changeset/config")).expect("config directory");
    symlink("config", root.join(".changeset/_config.toml")).expect("config symlink");

    let output = add_demo(&root);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("regular file"), "stderr: {err}");
}

#[cfg(unix)]
#[test]
fn config_fifo_is_rejected_without_blocking() {
    let root = temp_repo("config-fifo");
    cargo_package(&root, "demo");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    let config = root.join(".changeset/_config.toml");
    let mkfifo = Command::new("mkfifo")
        .arg(&config)
        .status()
        .expect("mkfifo");
    assert!(mkfifo.success(), "mkfifo: {mkfifo}");

    let (status, err) = add_demo_with_deadline(&root);

    assert!(!status.success());
    assert!(err.contains("regular file"), "stderr: {err}");
}

#[cfg(unix)]
#[test]
fn external_config_fifo_is_rejected_without_reading() {
    use std::os::unix::fs::symlink;

    let root = temp_repo("external-config-fifo");
    cargo_package(&root, "demo");
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    let external =
        root.with_file_name(format!("oakum-external-config-fifo-{}", std::process::id()));
    let _ = fs::remove_file(&external);
    let mkfifo = Command::new("mkfifo")
        .arg(&external)
        .status()
        .expect("mkfifo");
    assert!(mkfifo.success(), "mkfifo: {mkfifo}");
    symlink(&external, root.join(".changeset/_config.toml")).expect("config symlink");

    let (status, err) = add_demo_with_deadline(&root);
    fs::remove_file(&external).expect("remove external FIFO");

    assert!(!status.success());
    assert!(err.contains("outside the repository"), "stderr: {err}");
}

#[test]
fn config_path_must_resolve_to_regular_file() {
    let root = temp_repo("config-directory");
    cargo_package(&root, "demo");
    fs::create_dir_all(root.join(".changeset/_config.toml")).expect("config directory");

    let output = add_demo(&root);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("regular file"), "stderr: {err}");
}

#[test]
fn missing_config_file_still_adds() {
    let root = temp_repo("no-config");
    cargo_package(&root, "demo");
    let output = add_demo(&root);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(root.join(".changeset/cfg.md").is_file());
}
