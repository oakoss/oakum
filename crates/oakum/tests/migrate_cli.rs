//! `oakum migrate` (`okm-de5`).

#![allow(clippy::disallowed_methods)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const BINARY_VERSION: &str = env!("CARGO_PKG_VERSION");

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oakum"))
}

fn temp_repo(label: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("oakum-migrate-{label}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp repo");
    fs::create_dir(dir.join(".git")).expect("fixture .git");
    dir
}

fn migrate(root: &Path) -> std::process::Output {
    bin()
        .current_dir(root)
        .args(["migrate"])
        .output()
        .expect("oakum migrate")
}

fn migrate_args(root: &Path, args: &[&str]) -> std::process::Output {
    bin()
        .current_dir(root)
        .args(["migrate"])
        .args(args)
        .output()
        .expect("oakum migrate")
}

fn config_path(root: &Path) -> PathBuf {
    root.join(".changeset/_config.toml")
}

#[test]
fn nothing_to_migrate_names_init() {
    let root = temp_repo("empty");
    let output = migrate(&root);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("oakum init"), "{err}");
    assert!(!config_path(&root).exists());
}

#[test]
fn quoted_unscoped_keys_are_rewritten() {
    let root = temp_repo("quoted");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\n\"core\": minor\n---\nnote\n",
    )
    .expect("bump");
    fs::write(
        root.join(".changeset/config.json"),
        r#"{"changelog": "@changesets/cli/changelog", "access": "public"}"#,
    )
    .expect("config");
    let output = migrate(&root);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = fs::read_to_string(root.join(".changeset/feat.md")).expect("bump");
    assert_eq!(body, "---\ncore: minor\n---\nnote\n");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("rewrote .changeset/feat.md"), "{stdout}");
    assert!(stdout.contains("dropped `access`"), "{stdout}");
    assert!(stdout.contains("dropped `changelog`"), "{stdout}");
    assert!(stdout.contains("remaining"), "{stdout}");
    let config = fs::read_to_string(config_path(&root)).expect("oakum config");
    assert!(config.contains("versioning = \"semver\""), "{config}");
    assert!(
        config.contains(&format!("tool-version = \"{BINARY_VERSION}\"")),
        "{config}"
    );
    assert!(root.join(".changeset/config.json").is_file());
}

#[test]
fn knope_sets_zero_major_and_warns_about_readme() {
    let root = temp_repo("knope");
    fs::write(root.join("knope.toml"), "").expect("knope");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\n\"core\": patch\n---\n",
    )
    .expect("bump");
    let output = migrate(&root);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config = fs::read_to_string(config_path(&root)).expect("config");
    let bump = fs::read_to_string(root.join(".changeset/feat.md")).expect("bump");
    assert_eq!(bump, "---\ncore: patch\n---\n");
    assert!(config.contains("versioning = \"zero-major\""), "{config}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("knope.toml"), "{stdout}");
    assert!(stdout.contains("aborts knope"), "{stdout}");
    assert!(!stdout.contains("remove .changeset/"), "{stdout}");
    assert!(root.join("knope.toml").is_file());
}

#[test]
fn knope_plus_scoped_package_refuses_and_writes_nothing() {
    let root = temp_repo("scoped-knope");
    fs::write(root.join("knope.toml"), "").expect("knope");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\n\"@oakum/cli\": minor\n---\n",
    )
    .expect("bump");
    let output = migrate(&root);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("@oakum/cli"), "{err}");
    assert!(err.contains("knope.toml"), "{err}");
    assert!(!config_path(&root).exists());
    assert!(!root.join(".changeset/_schema.json").exists());
    assert!(!root.join(".changeset/README.md").exists());
    let body = fs::read_to_string(root.join(".changeset/feat.md")).expect("bump");
    assert!(body.contains("\"@oakum/cli\""), "{body}");
}

#[test]
fn already_migrated_is_idempotent() {
    let root = temp_repo("again");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(
        root.join(".changeset/feat.md"),
        "---\n\"core\": minor\n---\n",
    )
    .expect("bump");
    fs::write(root.join(".changeset/config.json"), "{}").expect("json");
    assert!(migrate(&root).status.success());
    let bump_before = fs::read_to_string(root.join(".changeset/feat.md")).expect("bump");
    assert_eq!(bump_before, "---\ncore: minor\n---\n");
    let config_before = fs::read_to_string(config_path(&root)).expect("config");
    let output = migrate(&root);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("already migrated"), "{stdout}");
    assert_eq!(
        fs::read_to_string(root.join(".changeset/feat.md")).expect("bump"),
        bump_before
    );
    assert_eq!(
        fs::read_to_string(config_path(&root)).expect("config"),
        config_before
    );
}

#[test]
fn versioning_flag_overrides_inference() {
    let root = temp_repo("override");
    fs::write(root.join("knope.toml"), "").expect("knope");
    let output = migrate_args(&root, &["--versioning", "semver"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config = fs::read_to_string(config_path(&root)).expect("config");
    assert!(config.contains("versioning = \"semver\""), "{config}");
}

#[test]
fn instruction_file_is_warned() {
    let root = temp_repo("agents");
    fs::write(root.join("knope.toml"), "").expect("knope");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(root.join(".changeset/AGENTS.md"), "notes\n").expect("agents");
    let output = migrate(&root);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("AGENTS.md"), "{stdout}");
    assert!(
        stdout.contains("aborts knope") || stdout.contains("`AGENTS.md`"),
        "{stdout}"
    );
}

#[test]
fn malformed_later_file_does_not_rewrite_earlier_files() {
    let root = temp_repo("partial");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(root.join(".changeset/a.md"), "---\n\"core\": minor\n---\n").expect("a");
    fs::write(root.join(".changeset/b.md"), "not a bump file\n").expect("b");
    let output = migrate(&root);
    assert!(!output.status.success());
    let body = fs::read_to_string(root.join(".changeset/a.md")).expect("a");
    assert!(body.contains("\"core\""), "{body}");
    assert!(!config_path(&root).exists());
}

#[test]
fn knope_with_none_level_refuses() {
    let root = temp_repo("knope-none");
    fs::write(root.join("knope.toml"), "").expect("knope");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(root.join(".changeset/feat.md"), "---\ncore: none\n---\n").expect("bump");
    let output = migrate(&root);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("none"), "{err}");
    assert!(!config_path(&root).exists());
}

#[test]
fn knope_with_empty_frontmatter_refuses() {
    let root = temp_repo("knope-empty");
    fs::write(root.join("knope.toml"), "").expect("knope");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(root.join(".changeset/empty.md"), "---\n---\nnote\n").expect("empty");
    let output = migrate(&root);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("empty"), "{err}");
    assert!(!config_path(&root).exists());
}

#[test]
fn bumpy_pending_files_are_copied_into_changeset() {
    let root = temp_repo("bumpy");
    fs::create_dir(root.join(".bumpy")).expect("dir");
    fs::write(root.join(".bumpy/_config.json"), "{}").expect("config");
    fs::write(
        root.join(".bumpy/feat.md"),
        "---\n\"core\": minor\n---\nnote\n",
    )
    .expect("bump");
    let output = migrate(&root);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = fs::read_to_string(root.join(".changeset/feat.md")).expect("copied");
    assert_eq!(body, "---\ncore: minor\n---\nnote\n");
    assert!(root.join(".bumpy/feat.md").is_file());
    assert!(config_path(&root).is_file());
}
