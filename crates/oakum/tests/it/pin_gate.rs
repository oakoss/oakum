//! The write gate across the writers: a repository with no oakum install pin
//! is refused as `unverified`, naming the cheapest pin for it (ADR-0038).

use crate::support;

use std::fs;

use std::path::Path;

use support::fixture::{
    cargo_package, commit, git, git_repo, git_stdout, oakum, oakum_exit, write_config,
    write_install_pin, Fixture, BINARY_VERSION,
};

const WRITERS: [&[&str]; 4] = [
    &["add", "--packages", "demo:patch", "--message", "x"],
    &["generate", "--from", "HEAD~1"],
    &["version"],
    &["ci", "version-pr"],
];

/// Committed, so a refusal that wrote anything shows up in `git status`. A
/// pending bump file and a scoped `fix` commit give each writer something to
/// write.
fn unpinned(label: &str, npm: bool) -> Fixture {
    let root = git_repo("pin-gate", label);
    if npm {
        fs::write(
            root.join("package.json"),
            "{\"name\": \"demo\", \"version\": \"0.1.0\"}\n",
        )
        .expect("package.json");
    } else {
        cargo_package(&root, "demo", "0.1.0");
    }
    fs::create_dir_all(root.join(".changeset")).expect("changeset");
    // A `tool-version` naming this binary must not stand in for the pin.
    fs::write(
        root.join(".changeset/_config.toml"),
        format!("tool-version = \"{BINARY_VERSION}\"\n"),
    )
    .expect("config");
    fs::write(
        root.join(".changeset/one.md"),
        "---\ndemo: patch\n---\n\npatch demo\n",
    )
    .expect("bump file");
    commit(&root, "chore: init");
    fs::write(root.join("notes.txt"), "x\n").expect("notes");
    commit(&root, "fix(demo): x");
    root
}

/// Every writer runs before any verdict, so one failure does not hide another.
fn writers_that_did_not_refuse(root: &Fixture, pin: &str) -> Vec<String> {
    let mut failures = Vec::new();
    for args in WRITERS {
        let output = oakum(root)
            .args(args)
            .env_remove("GITHUB_TOKEN")
            .env_remove("GH_TOKEN")
            .output()
            .expect("oakum");
        let stderr = String::from_utf8_lossy(&output.stderr);
        let refused = output.status.code() == Some(2)
            && stderr.contains("no oakum install pin")
            && stderr.contains(pin);
        git(root, &["add", "-A"]);
        let wrote = git_stdout(root, &["status", "--porcelain"]);
        if !refused || !wrote.is_empty() {
            failures.push(format!(
                "{args:?}: exit {:?}, wrote {wrote:?}: {stderr}",
                output.status.code()
            ));
            git(root, &["reset", "--hard", "--quiet"]);
        }
    }
    failures
}

/// A pin elsewhere must not let a dangling `.github/workflows` read as empty:
/// the workflows it hides could pin another version. A composite action holds
/// the pin rather than `.mise.toml`, which a mise shim in the fixture would read.
#[cfg(unix)]
#[test]
fn a_dangling_workflows_link_beside_a_pin_refuses_add() {
    let root = unpinned("dangling-workflows", false);
    fs::create_dir_all(root.join(".github/actions/setup")).expect("action dir");
    fs::write(
        root.join(".github/actions/setup/action.yml"),
        format!(
            "runs:\n  using: composite\n  steps:\n    - run: cargo binstall --no-confirm oakum@{BINARY_VERSION}\n      shell: bash\n"
        ),
    )
    .expect("action");
    std::os::unix::fs::symlink("nowhere", root.join(".github/workflows")).expect("dangling");
    let output = oakum(&root).args(WRITERS[0]).output().expect("oakum add");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("`.github/workflows` is a symlink to a missing target"),
        "{stderr}"
    );
    assert!(
        fs::read_dir(root.join(".changeset"))
            .expect("changeset")
            .count()
            == 2,
        "no bump file written"
    );
}

#[test]
fn no_pin_refuses_writers_naming_the_cheapest_pin() {
    let cargo = unpinned("cargo", false);
    let mut failures = writers_that_did_not_refuse(
        &cargo,
        &format!(
            "add `\"github:oakoss/oakum\" = \"{BINARY_VERSION}\"` under `[tools]` in `.mise.toml`"
        ),
    );
    let npm = unpinned("npm", true);
    failures.extend(writers_that_did_not_refuse(
        &npm,
        &format!("`pnpm add -D --allow-build=@oakoss/oakum @oakoss/oakum@{BINARY_VERSION}`"),
    ));
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

fn cargo_workspace(root: &Path, body: &str) {
    fs::write(
        root.join("Cargo.toml"),
        format!("[workspace]\nresolver = \"2\"\n{body}"),
    )
    .expect("workspace Cargo.toml");
}

/// `extra` follows `[package]`, for a member that declares dependencies.
fn member(root: &Path, dir: &str, name: &str, version: &str, extra: &str) {
    let path = root.join(dir);
    fs::create_dir_all(path.join("src")).expect("member src");
    fs::write(
        path.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2021\"\n{extra}"
        ),
    )
    .expect("member Cargo.toml");
    fs::write(path.join("src/lib.rs"), "").expect("member lib.rs");
}

/// Membership is Cargo's answer, so a member glob of any shape finds the
/// self-host pin: `check` passes it and `add` writes.
#[test]
fn a_member_matched_by_any_glob_is_the_self_host_pin() {
    let root = git_repo("pin-gate", "glob-member");
    cargo_workspace(&root, "members = [\"crates/oak*\"]\n");
    member(&root, "crates/oakum", "oakum", BINARY_VERSION, "");
    write_config(&root, "");
    commit(&root, "init");
    git(&root, &["tag", &format!("v{BINARY_VERSION}")]);

    let (code, document, stderr) = oakum_exit(&root, &["check", "--json"]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(!stderr.contains("unsupported"), "{stderr}");
    let parsed: serde_json::Value =
        serde_json::from_str(&document).unwrap_or_else(|err| panic!("{err}: {document}"));
    let pin = parsed["looks"]
        .as_array()
        .expect("looks")
        .iter()
        .find(|row| row["look"] == "install pin")
        .expect("an install pin row");
    assert_eq!(pin["outcome"], "ok", "{document}");

    let (code, _, stderr) = oakum_exit(
        &root,
        &["add", "--packages", "oakum:patch", "--message", "x"],
    );
    assert_eq!(code, Some(0), "{stderr}");
}

/// Cargo decides membership: a nested glob or an implicit path-dependency
/// member named `oakum` at another version is a mismatch.
#[test]
fn a_member_cargo_finds_at_another_version_is_a_mismatch() {
    for (label, members, dir, package, extra) in [
        ("oak-glob", "crates/oak*", "crates/oakum", "oakum", ""),
        (
            "two-level-glob",
            "crates/*/*",
            "crates/tools/oakum",
            "oakum",
            "",
        ),
        (
            "implicit-member",
            "app",
            "vendor/oakum",
            "app",
            "\n[dependencies]\noakum = { path = \"../vendor/oakum\" }\n",
        ),
    ] {
        let root = git_repo("pin-gate", label);
        cargo_workspace(&root, &format!("members = [\"{members}\"]\n"));
        if package == "app" {
            member(&root, "app", "app", "0.1.0", extra);
        }
        member(&root, dir, "oakum", "0.0.1", "");
        write_config(&root, "");
        commit(&root, "init");

        let (code, _, stderr) = oakum_exit(
            &root,
            &[
                "add",
                "--packages",
                &format!("{package}:patch"),
                "--message",
                "x",
            ],
        );
        assert_eq!(code, Some(1), "{label}: {stderr}");
        assert!(
            stderr.contains(&format!(
                "this binary is `{BINARY_VERSION}` but the install pin is `0.0.1` ({dir}/Cargo.toml)"
            )),
            "{label}: {stderr}"
        );
    }
}

/// Cargo reports `0.0.0` for a package with no `version`, which is not a pin
/// the manifest wrote.
#[test]
fn a_member_without_a_version_is_unverified() {
    let root = git_repo("pin-gate", "no-version");
    cargo_workspace(&root, "members = [\"crates/*\"]\n");
    fs::create_dir_all(root.join("crates/oakum/src")).expect("member src");
    fs::write(
        root.join("crates/oakum/Cargo.toml"),
        "[package]\nname = \"oakum\"\nedition = \"2021\"\n",
    )
    .expect("member Cargo.toml");
    fs::write(root.join("crates/oakum/src/lib.rs"), "").expect("lib.rs");
    write_config(&root, "");
    commit(&root, "init");
    for args in [
        &["upgrade"][..],
        &["add", "--packages", "oakum:patch", "--message", "x"],
    ] {
        let (code, _, stderr) = oakum_exit(&root, args);
        assert_eq!(code, Some(2), "{args:?}: {stderr}");
        assert!(
            stderr.contains("`crates/oakum/Cargo.toml` package `oakum` has no exact `version`"),
            "{args:?}: {stderr}"
        );
    }
}

/// Cargo still reads the legacy `[project]` table, so its version is a pin.
#[test]
fn a_legacy_project_table_declares_the_version() {
    let root = git_repo("pin-gate", "project-table");
    cargo_workspace(&root, "members = [\"crates/*\"]\n");
    fs::create_dir_all(root.join("crates/oakum/src")).expect("member src");
    fs::write(
        root.join("crates/oakum/Cargo.toml"),
        "[project]\nname = \"oakum\"\nversion = \"0.0.1\"\nedition = \"2021\"\n",
    )
    .expect("member Cargo.toml");
    fs::write(root.join("crates/oakum/src/lib.rs"), "").expect("lib.rs");
    write_config(&root, "");
    commit(&root, "init");
    let (code, _, stderr) = oakum_exit(&root, &["upgrade"]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains("the install pin is `0.0.1` (crates/oakum/Cargo.toml)"),
        "{stderr}"
    );
}

/// `version.workspace = true` is a declared version, resolved by Cargo.
#[test]
fn an_inherited_member_version_is_the_pin() {
    let root = git_repo("pin-gate", "inherited-version");
    cargo_workspace(
        &root,
        "members = [\"crates/*\"]\n\n[workspace.package]\nversion = \"0.0.1\"\n",
    );
    fs::create_dir_all(root.join("crates/oakum/src")).expect("member src");
    fs::write(
        root.join("crates/oakum/Cargo.toml"),
        "[package]\nname = \"oakum\"\nversion.workspace = true\nedition = \"2021\"\n",
    )
    .expect("member Cargo.toml");
    fs::write(root.join("crates/oakum/src/lib.rs"), "").expect("lib.rs");
    write_config(&root, "");
    commit(&root, "init");
    let (code, _, stderr) = oakum_exit(
        &root,
        &["add", "--packages", "oakum:patch", "--message", "x"],
    );
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains("the install pin is `0.0.1` (crates/oakum/Cargo.toml)"),
        "{stderr}"
    );
}

/// A root package is the member at the repository root, named by its manifest.
#[test]
fn a_root_package_named_oakum_is_the_self_host_pin() {
    let root = git_repo("pin-gate", "root-package");
    cargo_package(&root, "oakum", "0.0.1");
    write_config(&root, "");
    commit(&root, "init");

    let (code, _, stderr) = oakum_exit(
        &root,
        &["add", "--packages", "oakum:patch", "--message", "x"],
    );
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains(&format!(
            "this binary is `{BINARY_VERSION}` but the install pin is `0.0.1` (Cargo.toml)"
        )),
        "{stderr}"
    );
}

/// The pin is the root workspace's `oakum`, whichever directory oakum runs
/// in: inside an excluded workspace with its own `[workspace]`, it is still
/// found.
#[test]
fn the_root_workspace_holds_the_pin_from_a_nested_workspace() {
    let root = git_repo("pin-gate", "nested-workspace");
    cargo_workspace(&root, "members = [\"crates/*\"]\nexclude = [\"tools\"]\n");
    member(&root, "crates/oakum", "oakum", "0.0.1", "");
    fs::create_dir_all(root.join("tools")).expect("tools");
    cargo_workspace(&root.join("tools"), "members = [\"demo\"]\n");
    member(&root, "tools/demo", "demo", "0.1.0", "");
    write_config(&root, "");
    commit(&root, "init");
    let tools = root.join("tools");

    for args in [
        &["upgrade"][..],
        &["init"],
        &["add", "--packages", "demo:patch", "--message", "x"],
    ] {
        let (code, _, stderr) = oakum_exit(&tools, args);
        assert_eq!(code, Some(1), "{args:?}: {stderr}");
        assert!(
            stderr.contains("the install pin is `0.0.1` (crates/oakum/Cargo.toml)"),
            "{args:?}: {stderr}"
        );
    }
    let (_, document, stderr) = oakum_exit(&tools, &["check", "--json"]);
    let parsed: serde_json::Value =
        serde_json::from_str(&document).unwrap_or_else(|err| panic!("{err}: {document}"));
    let pin = parsed["looks"]
        .as_array()
        .expect("looks")
        .iter()
        .find(|row| row["look"] == "install pin")
        .expect("an install pin row");
    assert_eq!(pin["outcome"], "ok", "the root pin was read: {document}");
    assert!(!stderr.contains("no oakum install pin"), "{stderr}");
}

/// A Cargo workspace cargo cannot load is one unreadable source: the
/// workflow's pin still names the mismatch.
#[test]
fn a_missing_member_does_not_mask_a_workflow_pin() {
    let root = git_repo("pin-gate", "missing-member");
    cargo_workspace(&root, "members = [\"crates/gone\"]\n");
    write_install_pin(&root, "9.9.9");
    write_config(&root, "");
    commit(&root, "init");
    for args in [
        &["upgrade"][..],
        &["add", "--packages", "demo:patch", "--message", "x"],
    ] {
        let (code, _, stderr) = oakum_exit(&root, args);
        assert_eq!(code, Some(1), "{args:?}: {stderr}");
        assert!(
            stderr.contains("install oakum `9.9.9`"),
            "{args:?}: {stderr}"
        );
    }
}

/// With nothing else to decide, the unreadable Cargo source is the answer,
/// `unverified` rather than a failed discovery.
#[test]
fn an_invalid_root_manifest_alone_is_unverified() {
    let root = git_repo("pin-gate", "invalid-root-toml");
    fs::write(root.join("Cargo.toml"), "[[[not toml").expect("Cargo.toml");
    write_config(&root, "");
    commit(&root, "init");
    let (code, _, stderr) = oakum_exit(
        &root,
        &["add", "--packages", "demo:patch", "--message", "x"],
    );
    assert_eq!(code, Some(2), "{stderr}");
    assert!(
        stderr.starts_with(
            "unverified: could not read the Cargo workspace for a member named `oakum` (cargo: "
        ),
        "{stderr}"
    );
}

/// An empty member set is a Cargo workspace with no `oakum`, not a failure.
#[test]
fn a_workspace_with_no_members_has_no_cargo_pin() {
    let root = git_repo("pin-gate", "no-members");
    cargo_workspace(&root, "members = []\n");
    write_install_pin(&root, BINARY_VERSION);
    write_config(&root, "");
    commit(&root, "init");
    for args in [&["upgrade"][..], &["init"]] {
        let (code, _, stderr) = oakum_exit(&root, args);
        assert_eq!(code, Some(0), "{args:?}: {stderr}");
    }
}

/// Refused before cargo runs, which would block on a FIFO.
fn refuses_a_root_manifest_that_is_not_a_file(label: &str, place: fn(&Path)) {
    let root = git_repo("pin-gate", label);
    write_install_pin(&root, BINARY_VERSION);
    write_config(&root, "");
    commit(&root, "init");
    place(&root.join("Cargo.toml"));
    // `upgrade` first: `init` writes its files before it exits unverified.
    for args in [
        &["upgrade"][..],
        &["init"],
        &["add", "--packages", "demo:patch", "--message", "x"],
    ] {
        let (code, _, stderr) = oakum_exit(&root, args);
        assert_eq!(code, Some(2), "{label} {args:?}: {stderr}");
        assert!(
            stderr.contains("`Cargo.toml` is not a regular file"),
            "{label} {args:?}: {stderr}"
        );
        if args == ["upgrade"] {
            assert!(
                !root.join(".changeset/_schema.json").exists(),
                "{label}: upgrade wrote the schema"
            );
        }
    }
}

#[test]
fn a_directory_root_manifest_is_unverified() {
    refuses_a_root_manifest_that_is_not_a_file("dir-root-toml", |path| {
        fs::create_dir(path).expect("directory");
    });
}

#[cfg(unix)]
#[test]
fn a_fifo_root_manifest_is_unverified() {
    refuses_a_root_manifest_that_is_not_a_file("fifo-root-toml", |path| {
        let status = std::process::Command::new("mkfifo")
            .arg(path)
            .status()
            .expect("mkfifo");
        assert!(status.success(), "mkfifo: {status}");
    });
}
