//! Hidden `detect-release-tools` plumbing (`okm-0s5`).

#![allow(clippy::disallowed_methods)]

mod support;

use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use support::fixture::{oakum, plain_repo, Fixture};

fn temp_repo(label: &str) -> Fixture {
    let root = plain_repo("detect", label);
    fs::create_dir(root.join(".git")).expect("fixture .git");
    root
}

fn detect_command(root: &Path) -> Command {
    let mut command = oakum(root);
    command.args(["detect-release-tools"]);
    command
}

fn detect(root: &Path) -> std::process::Output {
    detect_command(root).output().expect("run")
}

#[cfg(unix)]
fn detect_with_deadline(root: &Path) -> (std::process::ExitStatus, String) {
    let mut child = detect_command(root)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("detect-release-tools");
    let deadline = Instant::now() + Duration::from_secs(2);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll detect") {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill blocked detect");
            child.wait().expect("reap detect");
            panic!("detect blocked while opening a marker");
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

fn assert_hit(root: &std::path::Path, needle: &str) {
    let output = detect(root);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(needle), "{stdout}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("oakum migrate"));
}

#[test]
fn empty_repo_prints_nothing() {
    let root = temp_repo("empty");
    let output = detect(&root);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).is_empty());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("migrate"));
}

#[test]
fn mismatched_tool_version_still_detects() {
    let root = temp_repo("toolver");
    fs::create_dir(root.join(".changeset")).expect("changeset");
    fs::write(
        root.join(".changeset/_config.toml"),
        "tool-version = \"9.9.9\"\n",
    )
    .expect("config");
    fs::write(root.join(".changeset/feat.md"), "---\n---\n").expect("feat");
    let output = detect(&root);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("changesets\t.changeset/"), "{stdout}");
    assert!(stderr.contains("oakum migrate"), "{stderr}");
    assert!(!stderr.contains("upgrade"), "{stderr}");
}

#[test]
fn knope_toml_names_migrate() {
    let root = temp_repo("knope");
    fs::write(root.join("knope.toml"), "").expect("knope.toml");
    assert_hit(&root, "knope\tknope.toml");
}

#[test]
fn bumpy_config_is_detected() {
    let root = temp_repo("bumpy");
    fs::create_dir(root.join(".bumpy")).expect("dir");
    fs::write(root.join(".bumpy/_config.json"), "{}").expect("config");
    assert_hit(&root, "bumpy\t.bumpy/_config.json");
}

#[test]
fn release_please_config_is_detected() {
    let root = temp_repo("rp");
    fs::write(root.join("release-please-config.json"), "{}").expect("rp");
    assert_hit(&root, "release-please\trelease-please-config.json");
}

#[test]
fn release_plz_toml_is_detected() {
    let root = temp_repo("rplz");
    fs::write(root.join("release-plz.toml"), "").expect("toml");
    assert_hit(&root, "release-plz\trelease-plz.toml");
}

#[test]
fn dotted_release_plz_toml_is_detected() {
    let root = temp_repo("rplz-dot");
    fs::write(root.join(".release-plz.toml"), "").expect("toml");
    assert_hit(&root, "release-plz\t.release-plz.toml");
}

#[test]
fn cargo_workspace_metadata_is_release_plz() {
    let root = temp_repo("cargo-plz");
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\n[workspace.metadata.release_plz]\n",
    )
    .expect("cargo");
    assert_hit(&root, "release-plz\tCargo.toml");
}

#[test]
fn releaserc_json_is_semantic_release() {
    let root = temp_repo("rc");
    fs::write(root.join(".releaserc.json"), "{}").expect("rc");
    fs::create_dir(root.join("pkg")).expect("pkg");
    fs::write(root.join("pkg/.releaserc"), "{}").expect("nested");
    let output = detect(&root);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.trim(),
        "semantic-release\t.releaserc.json",
        "{stdout}"
    );
    assert!(!stdout.contains("pkg/.releaserc"));
}

#[test]
fn release_config_js_is_detected() {
    let root = temp_repo("reljs");
    fs::write(root.join("release.config.js"), "").expect("js");
    assert_hit(&root, "semantic-release\trelease.config.js");
}

#[test]
fn release_config_cjs_is_detected() {
    let root = temp_repo("relcjs");
    fs::write(root.join("release.config.cjs"), "").expect("cjs");
    assert_hit(&root, "semantic-release\trelease.config.cjs");
}

#[test]
fn release_config_mjs_is_detected() {
    let root = temp_repo("relmjs");
    fs::write(root.join("release.config.mjs"), "").expect("mjs");
    assert_hit(&root, "semantic-release\trelease.config.mjs");
}

#[test]
fn release_config_ts_is_detected() {
    let root = temp_repo("relts");
    fs::write(root.join("release.config.ts"), "").expect("ts");
    assert_hit(&root, "semantic-release\trelease.config.ts");
}

#[test]
fn releaserc_bare_is_detected() {
    let root = temp_repo("rcbare");
    fs::write(root.join(".releaserc"), "{}").expect("rc");
    assert_hit(&root, "semantic-release\t.releaserc");
}

#[test]
fn releaserc_yaml_is_detected() {
    let root = temp_repo("rcyaml");
    fs::write(root.join(".releaserc.yaml"), "").expect("yaml");
    assert_hit(&root, "semantic-release\t.releaserc.yaml");
}

#[test]
fn releaserc_yml_is_detected() {
    let root = temp_repo("rcyml");
    fs::write(root.join(".releaserc.yml"), "").expect("yml");
    assert_hit(&root, "semantic-release\t.releaserc.yml");
}

#[test]
fn package_json_release_key_is_detected() {
    let root = temp_repo("pkg");
    fs::write(root.join("package.json"), "{\"release\":{}}\n").expect("json");
    assert_hit(&root, "semantic-release\tpackage.json");
}

#[test]
fn nx_json_release_key_is_detected() {
    let root = temp_repo("nx");
    fs::write(root.join("nx.json"), "{\"release\":{}}\n").expect("nx");
    assert_hit(&root, "nx release\tnx.json");
}

#[test]
fn orphan_bump_file_is_changesets() {
    let root = temp_repo("orphan");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::write(root.join(".changeset/feat.md"), "---\n---\n").expect("feat");
    assert_hit(&root, "changesets\t.changeset/");
}

#[test]
fn changeset_directory_named_like_a_bump_file_is_ignored() {
    let root = temp_repo("dir-md");
    fs::create_dir(root.join(".changeset")).expect("dir");
    fs::create_dir(root.join(".changeset/feat.md")).expect("nested dir");
    let output = detect(&root);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).is_empty());
}

#[test]
fn instruction_files_alone_are_not_a_migration() {
    let root = temp_repo("notes");
    fs::create_dir(root.join(".changeset")).expect("dir");
    for name in ["AGENTS.md", "CLAUDE.md", "GEMINI.md", "README.md"] {
        fs::write(root.join(".changeset").join(name), "notes").expect(name);
    }
    let output = detect(&root);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).is_empty());
}

#[test]
fn malformed_package_json_is_unverified() {
    let root = temp_repo("bad-json");
    fs::write(root.join("package.json"), "{").expect("json");
    let output = detect(&root);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("unverified"), "{err}");
}

#[test]
fn malformed_cargo_toml_is_unverified() {
    let root = temp_repo("bad-toml");
    fs::write(root.join("Cargo.toml"), "[").expect("toml");
    let output = detect(&root);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("unverified"), "{err}");
}

#[test]
fn changeset_as_a_file_is_unverified() {
    let root = temp_repo("cs-file");
    fs::write(root.join(".changeset"), "not a dir").expect("file");
    let output = detect(&root);
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("unverified"), "{err}");
}

#[test]
fn knope_survives_malformed_package_json() {
    let root = temp_repo("knope-json");
    fs::write(root.join("knope.toml"), "").expect("knope");
    fs::write(root.join("package.json"), "{").expect("json");
    let output = detect(&root);
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("knope\tknope.toml"), "{stdout}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("oakum migrate"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unverified"));
}

#[cfg(unix)]
#[test]
fn fifo_package_json_is_unverified_and_does_not_block() {
    let root = temp_repo("fifo-json");
    let status = Command::new("mkfifo")
        .arg(root.join("package.json"))
        .status()
        .expect("mkfifo");
    assert!(status.success(), "mkfifo failed: {status}");
    let (status, err) = detect_with_deadline(&root);
    assert!(!status.success());
    assert!(err.contains("unverified"), "{err}");
}
