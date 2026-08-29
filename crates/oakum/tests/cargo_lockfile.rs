//! A member version bump that does not retarget `Cargo.lock` fails
//! `--locked`. The helper must make that build succeed without rewriting
//! unrelated rows.

#![allow(clippy::disallowed_methods)]

mod support;

use std::fs;
use std::path::Path;
use std::process::Command;

use support::fixture::{plain_repo, Fixture};

use oakum::manifest::{retarget_cargo_lock, CargoLockBump};
use semver::Version;

fn cargo_in(root: &Path, args: &[&str]) -> std::process::Output {
    cargo()
        .args(args)
        .current_dir(root)
        .env("CARGO_HOME", root.join(".cargo-home"))
        .output()
        .unwrap_or_else(|err| panic!("cargo {args:?}: {err}"))
}

fn cargo() -> Command {
    Command::new(env!("CARGO"))
}

fn assert_ok(root: &Path, args: &[&str]) {
    let out = cargo_in(root, args);
    assert!(
        out.status.success(),
        "cargo {args:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn workspace(label: &str) -> Fixture {
    let dir = plain_repo("cargo-lock", label);
    fs::create_dir_all(dir.join("lib/src")).expect("lib src");
    fs::create_dir_all(dir.join("app/src")).expect("app src");
    fs::write(
        dir.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"lib\", \"app\"]\n",
    )
    .expect("workspace");
    fs::write(
        dir.join("lib/Cargo.toml"),
        "[package]\nname = \"lockdemo-lib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("lib manifest");
    fs::write(dir.join("lib/src/lib.rs"), "pub fn n() -> u8 { 1 }\n").expect("lib src");
    fs::write(
        dir.join("app/Cargo.toml"),
        "[package]\nname = \"lockdemo-app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nlockdemo-lib = { path = \"../lib\" }\n",
    )
    .expect("app manifest");
    fs::write(
        dir.join("app/src/lib.rs"),
        "pub fn n() -> u8 { lockdemo_lib::n() }\n",
    )
    .expect("app src");
    dir
}

#[test]
fn retargeted_lockfile_satisfies_locked_and_leaves_other_rows() {
    let root = workspace("locked");
    assert_ok(&root, &["generate-lockfile", "--offline"]);
    let before = fs::read_to_string(root.join("Cargo.lock")).expect("lock");
    assert!(before.contains("name = \"lockdemo-lib\"\nversion = \"0.1.0\""));

    fs::write(
        root.join("lib/Cargo.toml"),
        "[package]\nname = \"lockdemo-lib\"\nversion = \"0.2.0\"\nedition = \"2021\"\n",
    )
    .expect("bump");
    let stale = cargo_in(
        &root,
        &["metadata", "--locked", "--offline", "--format-version=1"],
    );
    assert!(!stale.status.success(), "stale lockfile must fail --locked");
    assert!(
        String::from_utf8_lossy(&stale.stderr).contains("--locked"),
        "stderr: {}",
        String::from_utf8_lossy(&stale.stderr)
    );

    let from = Version::parse("0.1.0").expect("from");
    let to = Version::parse("0.2.0").expect("to");
    let after = retarget_cargo_lock(
        &before,
        &[CargoLockBump {
            name: "lockdemo-lib",
            from: &from,
            to: &to,
        }],
    )
    .expect("retarget");
    assert!(after.contains("name = \"lockdemo-lib\"\nversion = \"0.2.0\""));
    assert!(after.contains("name = \"lockdemo-app\"\nversion = \"0.1.0\""));
    fs::write(root.join("Cargo.lock"), after).expect("write lock");
    assert_ok(
        &root,
        &["metadata", "--locked", "--offline", "--format-version=1"],
    );
}
