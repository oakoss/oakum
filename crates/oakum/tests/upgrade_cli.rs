//! `oakum upgrade`: the one command exempt from the version gate (ADR-0007).
//! Owns `tool-version` in `_config.toml` and `_schema.json`; writes nothing
//! on validation failure.

#![allow(clippy::disallowed_methods)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

const BINARY_VERSION: &str = env!("CARGO_PKG_VERSION");

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oakum"))
}

fn git(root: &std::path::Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .expect("git");
    assert!(status.success(), "git {args:?} failed");
}

fn temp_repo(label: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("oakum-upgrade-{label}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp repo");
    git(&dir, &["init", "-b", "main"]);
    dir
}

fn write_config(root: &std::path::Path, body: &str) {
    fs::create_dir_all(root.join(".changeset")).expect("changeset dir");
    fs::write(root.join(".changeset/_config.toml"), body).expect("config");
}

fn run_upgrade(root: &std::path::Path) -> (bool, String, String) {
    let out = bin()
        .arg("upgrade")
        .current_dir(root)
        .output()
        .expect("oakum upgrade");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn upgrade_rewrites_the_version_and_creates_the_schema() {
    let root = temp_repo("rewrite");
    // 999.0.0 differs from every real binary version, so write commands refuse
    // and upgrade must not.
    write_config(
        &root,
        "# pinned by upgrade\ntool-version = \"999.0.0\" # note\nversioning = \"semver\"\n",
    );

    let add = bin()
        .args(["add", "--packages", "demo:patch", "--message", "x"])
        .current_dir(&root)
        .output()
        .expect("add");
    assert!(
        !add.status.success(),
        "the version gate must refuse writes first"
    );
    assert!(
        String::from_utf8_lossy(&add.stderr).contains("oakum upgrade"),
        "the refusal names the fix"
    );

    let (ok, stdout, stderr) = run_upgrade(&root);
    assert!(ok, "{stderr}");
    assert!(
        stdout.contains(&format!("999.0.0 -> {BINARY_VERSION}")),
        "{stdout}"
    );
    assert!(
        stdout.contains("(downgrade"),
        "999.0.0 is newer than any real binary, so the direction must be named: {stdout}"
    );
    let config = fs::read_to_string(root.join(".changeset/_config.toml")).expect("config");
    assert_eq!(
        config,
        format!(
            "# pinned by upgrade\ntool-version = \"{BINARY_VERSION}\" # note\nversioning = \"semver\"\n"
        ),
        "every byte outside the version value survives"
    );
    let schema = fs::read_to_string(root.join(".changeset/_schema.json")).expect("schema");
    assert_eq!(schema, oakum::config::schema_json());
}

#[test]
fn upgrade_is_idempotent() {
    let root = temp_repo("idempotent");
    write_config(&root, "tool-version = \"999.0.0\"\n");
    let (ok, _, stderr) = run_upgrade(&root);
    assert!(ok, "{stderr}");
    let config_before = fs::read_to_string(root.join(".changeset/_config.toml")).expect("config");
    let schema_before = fs::read_to_string(root.join(".changeset/_schema.json")).expect("schema");

    let (ok, stdout, stderr) = run_upgrade(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("already at"), "{stdout}");
    assert_eq!(
        fs::read_to_string(root.join(".changeset/_config.toml")).expect("config"),
        config_before
    );
    assert_eq!(
        fs::read_to_string(root.join(".changeset/_schema.json")).expect("schema"),
        schema_before
    );
}

#[test]
fn invalid_config_writes_nothing() {
    let root = temp_repo("invalid");
    let body = "tool-version = \"999.0.0\"\ngit-user = \"nope\"\n";
    write_config(&root, body);

    let (ok, stdout, stderr) = run_upgrade(&root);
    assert!(!ok, "unknown key must fail validation: {stdout}");
    assert!(stderr.contains("nothing was written"), "{stderr}");
    assert_eq!(
        fs::read_to_string(root.join(".changeset/_config.toml")).expect("config"),
        body,
        "a failed upgrade must not touch the config"
    );
    assert!(
        !root.join(".changeset/_schema.json").exists(),
        "a failed upgrade must not write the schema"
    );
}

#[test]
fn missing_template_file_writes_nothing() {
    let root = temp_repo("missing-tpl");
    let body = "tool-version = \"999.0.0\"\ntag-format = { file = \"notes.md\" }\n";
    write_config(&root, body);

    let (ok, stdout, stderr) = run_upgrade(&root);
    assert!(!ok, "missing template file must fail: {stdout}");
    assert!(stderr.contains("nothing was written"), "{stderr}");
    assert!(stderr.contains("failed to resolve template"), "{stderr}");
    assert!(stderr.contains("tag-format"), "{stderr}");
    assert_eq!(
        fs::read_to_string(root.join(".changeset/_config.toml")).expect("config"),
        body,
        "a failed upgrade must not touch the config"
    );
    assert!(
        !root.join(".changeset/_schema.json").exists(),
        "a failed upgrade must not write the schema"
    );
}

#[test]
fn missing_config_names_init() {
    let root = temp_repo("missing");
    let (ok, stdout, stderr) = run_upgrade(&root);
    assert!(!ok, "{stdout}");
    assert!(stderr.contains("oakum init"), "{stderr}");
}

#[test]
fn ordinary_upgrade_carries_no_downgrade_marker() {
    let root = temp_repo("forward");
    // A prerelease of the binary's own version orders below it, so this is
    // the forward direction regardless of what version the binary reports.
    let old = format!("{BINARY_VERSION}-alpha.1");
    write_config(&root, &format!("tool-version = \"{old}\"\n"));

    let (ok, stdout, stderr) = run_upgrade(&root);
    assert!(ok, "{stderr}");
    assert!(
        stdout.contains(&format!("{old} -> {BINARY_VERSION}")),
        "{stdout}"
    );
    assert!(!stdout.contains("(downgrade"), "{stdout}");
}

#[cfg(unix)]
#[test]
fn symlinked_config_is_rewritten_through_the_link() {
    let root = temp_repo("symlink");
    fs::create_dir_all(root.join(".changeset")).expect("changeset dir");
    fs::write(
        root.join(".changeset/real-config.toml"),
        "tool-version = \"999.0.0\"\n",
    )
    .expect("real config");
    std::os::unix::fs::symlink("real-config.toml", root.join(".changeset/_config.toml"))
        .expect("symlink");

    let (ok, _, stderr) = run_upgrade(&root);
    assert!(ok, "{stderr}");
    assert!(
        root.join(".changeset/_config.toml")
            .symlink_metadata()
            .expect("metadata")
            .file_type()
            .is_symlink(),
        "upgrade must rewrite the target, not replace the link"
    );
    assert_eq!(
        fs::read_to_string(root.join(".changeset/real-config.toml")).expect("target"),
        format!("tool-version = \"{BINARY_VERSION}\"\n")
    );
    assert_eq!(
        fs::read_to_string(root.join(".changeset/_schema.json")).expect("schema"),
        oakum::config::schema_json()
    );
}

#[cfg(unix)]
#[test]
fn stale_staging_symlink_cannot_redirect_the_write() {
    let root = temp_repo("staging-hijack");
    let body = format!("tool-version = \"{BINARY_VERSION}\"\n");
    write_config(&root, &body);
    // A committed symlink at the predictable staging path must not let the
    // schema bytes land in the config.
    std::os::unix::fs::symlink(
        "_config.toml",
        root.join(".changeset/._schema.json.oakum-upgrade"),
    )
    .expect("hostile staging symlink");

    let (ok, _, stderr) = run_upgrade(&root);
    assert!(ok, "{stderr}");
    assert_eq!(
        fs::read_to_string(root.join(".changeset/_config.toml")).expect("config"),
        body,
        "the config must not receive the staged schema bytes"
    );
    assert_eq!(
        fs::read_to_string(root.join(".changeset/_schema.json")).expect("schema"),
        oakum::config::schema_json()
    );
    // The pid-suffixed staging name never matches a committed path; the
    // hostile link is not ours to remove and must survive untouched.
    assert!(root
        .join(".changeset/._schema.json.oakum-upgrade")
        .symlink_metadata()
        .expect("hostile symlink still present")
        .file_type()
        .is_symlink(),);
}

fn staging_leftovers(root: &std::path::Path) -> Vec<String> {
    fs::read_dir(root.join(".changeset"))
        .expect("read changeset")
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".oakum-upgrade"))
        .collect()
}

#[test]
fn failed_rename_cleans_up_the_staging_file() {
    let root = temp_repo("failed-rename");
    let body = format!("tool-version = \"{BINARY_VERSION}\"\n");
    write_config(&root, &body);
    // A directory at the schema path makes the rename fail after staging.
    fs::create_dir_all(root.join(".changeset/_schema.json")).expect("blocking dir");

    let (ok, stdout, stderr) = run_upgrade(&root);
    assert!(!ok, "{stdout}");
    assert!(stderr.contains("_schema.json"), "{stderr}");
    assert_eq!(
        fs::read_to_string(root.join(".changeset/_config.toml")).expect("config"),
        body,
        "the config write must not happen after the schema write fails"
    );
    assert_eq!(
        staging_leftovers(&root),
        Vec::<String>::new(),
        "the staging file must be cleaned up after a failed rename"
    );
}

#[test]
fn stale_schema_is_regenerated_without_touching_the_config() {
    let root = temp_repo("stale-schema");
    let body = format!("tool-version = \"{BINARY_VERSION}\"\n");
    write_config(&root, &body);
    fs::write(root.join(".changeset/_schema.json"), "{}\n").expect("stale schema");

    let (ok, stdout, stderr) = run_upgrade(&root);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("(unchanged)"), "{stdout}");
    assert!(stdout.contains("regenerated"), "{stdout}");
    assert_eq!(
        fs::read_to_string(root.join(".changeset/_config.toml")).expect("config"),
        body
    );
    assert_eq!(
        fs::read_to_string(root.join(".changeset/_schema.json")).expect("schema"),
        oakum::config::schema_json()
    );
}
