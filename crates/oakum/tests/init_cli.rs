//! `oakum init` (`okm-0f4`).

#![allow(clippy::disallowed_methods)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const BINARY_VERSION: &str = env!("CARGO_PKG_VERSION");

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oakum"))
}

fn temp_repo(label: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("oakum-init-{label}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp repo");
    fs::create_dir(dir.join(".git")).expect("fixture .git");
    dir
}

fn init(root: &Path) -> std::process::Output {
    bin()
        .current_dir(root)
        .args(["init"])
        .output()
        .expect("oakum init")
}

fn init_args(root: &Path, args: &[&str]) -> std::process::Output {
    bin()
        .current_dir(root)
        .args(["init"])
        .args(args)
        .output()
        .expect("oakum init")
}

fn config_path(root: &Path) -> PathBuf {
    root.join(".changeset/_config.toml")
}

fn schema_path(root: &Path) -> PathBuf {
    root.join(".changeset/_schema.json")
}

fn readme_path(root: &Path) -> PathBuf {
    root.join(".changeset/README.md")
}

fn assert_no_oakum_files(root: &Path) {
    assert!(!config_path(root).exists());
    assert!(!schema_path(root).exists());
    assert!(!readme_path(root).exists());
    assert!(!root.join(".github").exists());
}

#[test]
fn empty_repo_writes_three_files_and_prints_workflow() {
    let root = temp_repo("empty");
    let output = init(&root);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("created .changeset/_config.toml"),
        "{stdout}"
    );
    assert!(
        stdout.contains("created .changeset/_schema.json"),
        "{stdout}"
    );
    assert!(stdout.contains("created .changeset/README.md"), "{stdout}");
    assert!(
        stdout.contains(&format!("oakum@{BINARY_VERSION}")),
        "{stdout}"
    );
    assert!(stdout.contains("oakum check"), "{stdout}");
    assert!(
        stdout.contains(
            "run: oakum ci pr-status\n        if: github.event_name == 'pull_request' && (success() || failure())\n        continue-on-error: true",
        ),
        "{stdout}"
    );
    assert!(
        stdout.contains("contents: read\n      pull-requests: write"),
        "{stdout}"
    );
    assert_eq!(stdout.matches("fetch-depth: 0").count(), 2, "{stdout}");
    assert!(stdout.contains("oakum ci version-pr"), "{stdout}");
    assert!(
        stdout.contains("github.event.repository.default_branch"),
        "{stdout}"
    );
    assert!(stdout.contains("${{ secrets.GITHUB_TOKEN }}"), "{stdout}");
    assert!(stdout.contains("uninstall"), "{stdout}");
    assert!(stdout.contains("--interactive"), "{stdout}");
    assert!(stdout.contains("no packages found"), "{stdout}");

    let config = fs::read_to_string(config_path(&root)).expect("config");
    assert!(
        config.contains(&format!("tool-version = \"{BINARY_VERSION}\"")),
        "{config}"
    );
    assert!(config.contains("versioning = \"zero-major\""), "{config}");
    assert!(config.contains("change-files = true"), "{config}");
    assert!(config.contains("conventional-commits = true"), "{config}");
    oakum::config::parse(&config).expect("written config parses");

    let schema = fs::read_to_string(schema_path(&root)).expect("schema");
    assert_eq!(schema, oakum::config::schema_json());
    assert!(readme_path(&root).is_file());
    assert!(!root.join(".github").exists());
}

#[test]
fn second_run_is_idempotent() {
    let root = temp_repo("idempotent");
    assert!(init(&root).status.success());
    let config_before = fs::read_to_string(config_path(&root)).expect("config");
    let schema_before = fs::read_to_string(schema_path(&root)).expect("schema");
    let readme_before = fs::read_to_string(readme_path(&root)).expect("readme");
    let output = init(&root);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("already initialized"), "{stdout}");
    assert!(
        !stdout.contains("created .changeset/_config.toml"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("created .changeset/_schema.json"),
        "{stdout}"
    );
    assert!(!stdout.contains("created .changeset/README.md"), "{stdout}");
    assert_eq!(
        fs::read_to_string(config_path(&root)).expect("config"),
        config_before
    );
    assert_eq!(
        fs::read_to_string(schema_path(&root)).expect("schema"),
        schema_before
    );
    assert_eq!(
        fs::read_to_string(readme_path(&root)).expect("readme"),
        readme_before
    );
}

#[test]
fn knope_toml_names_migrate_and_writes_nothing() {
    let root = temp_repo("knope");
    fs::write(root.join("knope.toml"), "").expect("knope");
    let output = init(&root);
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("knope\tknope.toml"), "{stdout}");
    assert!(stderr.contains("oakum migrate"), "{stderr}");
    assert_no_oakum_files(&root);
}

#[test]
fn bump_files_without_config_name_migrate() {
    let root = temp_repo("orphan-bump");
    fs::create_dir(root.join(".changeset")).expect("changeset");
    fs::write(root.join(".changeset/feat.md"), "---\n---\nnote\n").expect("bump");
    let output = init(&root);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("oakum migrate"));
    assert!(!config_path(&root).exists());
    assert!(!schema_path(&root).exists());
    assert!(!readme_path(&root).exists());
}

#[test]
fn instruction_file_is_reported_and_init_continues() {
    let root = temp_repo("agents");
    fs::create_dir(root.join(".changeset")).expect("changeset");
    fs::write(root.join(".changeset/AGENTS.md"), "notes\n").expect("agents");
    let output = init(&root);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("AGENTS.md"), "{stdout}");
    assert!(!stderr.contains("oakum migrate"), "{stderr}");
    assert!(config_path(&root).is_file());
}

#[test]
fn tool_version_mismatch_names_upgrade() {
    let root = temp_repo("mismatch");
    fs::create_dir(root.join(".changeset")).expect("changeset");
    fs::write(config_path(&root), "tool-version = \"9.9.9\"\n").expect("config");
    let output = init(&root);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("tool-version"), "{stderr}");
    assert!(stderr.contains("upgrade"), "{stderr}");
}

#[test]
fn explicit_versioning_that_disagrees_is_refused() {
    let root = temp_repo("disagree");
    fs::create_dir(root.join(".changeset")).expect("changeset");
    fs::write(
        config_path(&root),
        format!("tool-version = \"{BINARY_VERSION}\"\nversioning = \"semver\"\n"),
    )
    .expect("config");
    let output = init_args(&root, &["--versioning", "zero-major"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("versioning"), "{stderr}");
    assert!(stderr.contains("semver"), "{stderr}");
    let config = fs::read_to_string(config_path(&root)).expect("config");
    assert!(config.contains("versioning = \"semver\""), "{config}");
}

#[test]
fn versioning_semver_is_written_explicitly() {
    let root = temp_repo("semver");
    let output = init_args(&root, &["--versioning", "semver"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let config = fs::read_to_string(config_path(&root)).expect("config");
    assert!(config.contains("versioning = \"semver\""), "{config}");
    assert!(
        config.contains(&format!("tool-version = \"{BINARY_VERSION}\"")),
        "{config}"
    );
    oakum::config::parse(&config).expect("written config parses");
}

#[test]
fn existing_readme_is_not_overwritten() {
    let root = temp_repo("keep-readme");
    fs::create_dir(root.join(".changeset")).expect("changeset");
    fs::write(root.join(".changeset/README.md"), "keep me\n").expect("readme");
    let output = init(&root);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(readme_path(&root)).expect("readme"),
        "keep me\n"
    );
}

#[test]
fn interactive_without_a_tty_names_flags() {
    let root = temp_repo("no-tty");
    let output = bin()
        .current_dir(&root)
        .args(["init", "--interactive"])
        .stdin(Stdio::null())
        .output()
        .expect("oakum init");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--interactive"), "{stderr}");
    assert!(stderr.contains("--versioning"), "{stderr}");
    assert_no_oakum_files(&root);
}

#[test]
fn malformed_package_json_is_unverified_and_writes_nothing() {
    let root = temp_repo("bad-json");
    fs::write(root.join("package.json"), "{").expect("json");
    let output = init(&root);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unverified"), "{stderr}");
    assert_no_oakum_files(&root);
}

#[test]
fn interactive_on_mismatched_version_names_upgrade_first() {
    let root = temp_repo("interactive-mismatch");
    fs::create_dir(root.join(".changeset")).expect("changeset");
    fs::write(config_path(&root), "tool-version = \"9.9.9\"\n").expect("config");
    let output = bin()
        .current_dir(&root)
        .args(["init", "--interactive"])
        .stdin(Stdio::null())
        .output()
        .expect("oakum init");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("upgrade"), "{stderr}");
    assert!(!stderr.contains("--interactive"), "{stderr}");
}

#[test]
fn first_init_replaces_a_stale_schema() {
    let root = temp_repo("stale-schema");
    fs::create_dir(root.join(".changeset")).expect("changeset");
    fs::write(schema_path(&root), "{}\n").expect("stale schema");
    let output = init(&root);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let schema = fs::read_to_string(schema_path(&root)).expect("schema");
    assert_eq!(schema, oakum::config::schema_json());
}

#[cfg(unix)]
#[test]
fn changeset_directory_symlink_is_followed() {
    use std::os::unix::fs::symlink;

    let root = temp_repo("changeset-link");
    fs::create_dir(root.join("actual-changeset")).expect("target");
    symlink("actual-changeset", root.join(".changeset")).expect("symlink");
    let output = init(&root);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(root.join("actual-changeset/_config.toml").is_file());
    assert!(config_path(&root).is_file());
}

#[test]
fn already_initialized_does_not_create_a_missing_readme() {
    let root = temp_repo("no-readme");
    fs::create_dir(root.join(".changeset")).expect("changeset");
    fs::write(
        config_path(&root),
        format!("tool-version = \"{BINARY_VERSION}\"\n"),
    )
    .expect("config");
    let output = init(&root);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("already initialized"), "{stdout}");
    assert!(!readme_path(&root).exists());
    assert!(!schema_path(&root).exists());
}

#[test]
fn already_initialized_refuses_a_missing_template_file() {
    let root = temp_repo("missing-tpl");
    fs::create_dir(root.join(".changeset")).expect("changeset");
    fs::write(
        config_path(&root),
        format!("tool-version = \"{BINARY_VERSION}\"\ntag-format = {{ file = \"notes.md\" }}\n"),
    )
    .expect("config");
    let output = init(&root);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "missing template file must fail: stdout={stdout} stderr={err}"
    );
    assert!(err.contains("failed to resolve template"), "{err}");
    assert!(err.contains("tag-format"), "{err}");
    assert!(!stdout.contains("already initialized"), "{stdout}");
    assert_eq!(
        fs::read_to_string(config_path(&root)).expect("config"),
        format!("tool-version = \"{BINARY_VERSION}\"\ntag-format = {{ file = \"notes.md\" }}\n")
    );
    assert!(!schema_path(&root).exists());
    assert!(!readme_path(&root).exists());
}
