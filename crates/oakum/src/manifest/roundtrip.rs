//! Hand-formatted fixtures for every in-memory write path (okm-pm1).
//!
//! A diff on an untouched region is as severe as a wrong version.

use semver::Version;

use crate::plan::bounds::Bounds;
use crate::plan::workspace::{DeclaredRange, Dependency, DependencyKind, Ecosystem, PackageId};

use super::cargo_lock::{retarget_cargo_lock, CargoLockBump};
use super::json::set_json_string;
use super::rewrite::rewrite_dependency;
use super::toml::set_toml_string;

const CARGO_PACKAGE: &str = include_str!("fixtures/cargo-package.toml");
const CARGO_PACKAGE_CRLF: &str = include_str!("fixtures/cargo-package.crlf.toml");
const CARGO_PACKAGE_INSERT: &str = include_str!("fixtures/cargo-package-insert.toml");
const CARGO_PACKAGE_INSERT_CRLF: &str = include_str!("fixtures/cargo-package-insert.crlf.toml");
const PACKAGE_JSON: &str = include_str!("fixtures/package.json");
const PACKAGE_JSON_CRLF: &str = include_str!("fixtures/package.crlf.json");
const CARGO_DEP: &str = include_str!("fixtures/cargo-dep.toml");
const CARGO_DEP_CRLF: &str = include_str!("fixtures/cargo-dep.crlf.toml");
const PACKAGE_DEP: &str = include_str!("fixtures/package-dep.json");
const PACKAGE_DEP_CRLF: &str = include_str!("fixtures/package-dep.crlf.json");
const CARGO_LOCK: &str = include_str!("fixtures/cargo-lock.toml");
const CARGO_LOCK_CRLF: &str = include_str!("fixtures/cargo-lock.crlf.toml");
const PACKAGE_INSERT: &str = include_str!("fixtures/package-insert.json");
const PACKAGE_INSERT_CRLF: &str = include_str!("fixtures/package-insert.crlf.json");
const CARGO_DEP_TABLE: &str = include_str!("fixtures/cargo-dep-table.toml");
const CARGO_DEP_TABLE_CRLF: &str = include_str!("fixtures/cargo-dep-table.crlf.toml");

fn cargo_dep(name: &str, range: &str) -> Dependency {
    Dependency {
        on: PackageId::new(Ecosystem::Cargo, name),
        kind: DependencyKind::Normal,
        declared_as: name.to_owned(),
        target: None,
        range: DeclaredRange::Plain(Bounds::from_cargo_text(range).expect("range")),
    }
}

fn npm_dep(name: &str, range: &str) -> Dependency {
    Dependency {
        on: PackageId::new(Ecosystem::Npm, name),
        kind: DependencyKind::Normal,
        declared_as: name.to_owned(),
        target: None,
        range: DeclaredRange::Plain(Bounds::from_npm_text(range).expect("range")),
    }
}

fn once(src: &str, needle: &str, next: &str) -> String {
    assert_eq!(
        src.matches(needle).count(),
        1,
        "needle {needle:?} must occur once"
    );
    src.replacen(needle, next, 1)
}

fn bump_cargo_package(src: &str) -> String {
    set_toml_string(src, &["package", "version"], "0.2.0").expect("toml")
}

fn bump_json_package(src: &str) -> String {
    set_json_string(src, &["version"], "0.2.0").expect("json")
}

fn bump_cargo_dep(src: &str) -> String {
    rewrite_dependency(
        Ecosystem::Cargo,
        src,
        &cargo_dep("core", "^0.1.0"),
        &Version::parse("0.2.0").expect("v"),
    )
    .expect("rewrite")
    .expect("changed")
}

fn bump_npm_dep(src: &str) -> String {
    rewrite_dependency(
        Ecosystem::Npm,
        src,
        &npm_dep("core", "^0.1.0"),
        &Version::parse("0.2.0").expect("v"),
    )
    .expect("rewrite")
    .expect("changed")
}

fn bump_cargo_lock(src: &str) -> String {
    let from = Version::parse("0.1.0").expect("from");
    let to = Version::parse("0.2.0").expect("to");
    retarget_cargo_lock(
        src,
        &[CargoLockBump {
            name: "demo",
            from: &from,
            to: &to,
        }],
    )
    .expect("lock")
}

fn expect_cargo_package(src: &str) -> String {
    once(src, "\tversion = \"0.1.0\"", "\tversion = \"0.2.0\"")
}

fn expect_json_package(src: &str) -> String {
    once(src, "\t\"version\": \"0.1.0\"", "\t\"version\": \"0.2.0\"")
}

fn expect_cargo_dep(src: &str) -> String {
    once(src, "\tcore = \"^0.1.0\"", "\tcore = \"^0.2.0\"")
}

fn expect_npm_dep(src: &str) -> String {
    once(src, "\t\"core\": \"^0.1.0\"", "\t\"core\": \"^0.2.0\"")
}

fn expect_cargo_lock(src: &str) -> String {
    once(
        &once(src, "\tversion = \"0.1.0\"", "\tversion = \"0.2.0\""),
        "demo 0.1.0",
        "demo 0.2.0",
    )
}

fn expect_cargo_dep_table(src: &str) -> String {
    once(src, "version = \"^0.1.0\"", "version = \"^0.2.0\"")
}

fn newline(src: &str) -> &'static str {
    if src.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn expect_cargo_insert(src: &str) -> String {
    let nl = newline(src);
    once(
        src,
        "edition = \"2021\"",
        &format!("edition = \"2021\"{nl}version = \"0.2.0\""),
    )
}

fn expect_json_insert(src: &str) -> String {
    let nl = newline(src);
    once(
        src,
        "\"private\": true",
        &format!("\"private\": true,{nl}\t\"version\": \"0.2.0\""),
    )
}

fn without_trailing_newline(src: &str) -> &str {
    src.strip_suffix("\r\n")
        .or_else(|| src.strip_suffix('\n'))
        .unwrap_or(src)
}

fn assert_exclusive_lf(src: &str) {
    assert!(
        !src.contains('\r'),
        "LF fixture must not contain CR: {src:?}"
    );
}

fn assert_exclusive_crlf(src: &str) {
    assert!(src.contains("\r\n"), "CRLF fixture was normalized: {src:?}");
    let stripped = src.replace("\r\n", "");
    assert!(
        !stripped.contains('\n') && !stripped.contains('\r'),
        "CRLF fixture is mixed: {src:?}"
    );
}

#[test]
fn cargo_package_keeps_comments_tabs_and_neighbors() {
    assert_exclusive_lf(CARGO_PACKAGE);
    assert_eq!(
        bump_cargo_package(CARGO_PACKAGE),
        expect_cargo_package(CARGO_PACKAGE)
    );
}

#[test]
fn cargo_package_keeps_crlf() {
    assert_exclusive_crlf(CARGO_PACKAGE_CRLF);
    assert_eq!(
        bump_cargo_package(CARGO_PACKAGE_CRLF),
        expect_cargo_package(CARGO_PACKAGE_CRLF)
    );
}

#[test]
fn cargo_package_insert_keeps_neighbors() {
    assert_exclusive_lf(CARGO_PACKAGE_INSERT);
    assert_eq!(
        set_toml_string(CARGO_PACKAGE_INSERT, &["package", "version"], "0.2.0").expect("insert"),
        expect_cargo_insert(CARGO_PACKAGE_INSERT)
    );
}

#[test]
fn cargo_package_insert_keeps_crlf() {
    assert_exclusive_crlf(CARGO_PACKAGE_INSERT_CRLF);
    assert_eq!(
        set_toml_string(CARGO_PACKAGE_INSERT_CRLF, &["package", "version"], "0.2.0")
            .expect("insert"),
        expect_cargo_insert(CARGO_PACKAGE_INSERT_CRLF)
    );
}

#[test]
fn cargo_package_insert_without_trailing_newline_keeps_absence() {
    let src = without_trailing_newline(CARGO_PACKAGE_INSERT);
    assert!(!src.ends_with('\n'));
    let out = set_toml_string(src, &["package", "version"], "0.2.0").expect("insert");
    assert!(!out.ends_with('\n'), "{out:?}");
    assert_eq!(out, expect_cargo_insert(src));
}

#[test]
fn cargo_package_insert_crlf_without_trailing_newline_keeps_absence() {
    let src = without_trailing_newline(CARGO_PACKAGE_INSERT_CRLF);
    assert!(!src.ends_with('\n'));
    let out = set_toml_string(src, &["package", "version"], "0.2.0").expect("insert");
    assert!(!out.ends_with('\n'), "{out:?}");
    assert_eq!(out, expect_cargo_insert(src));
}

#[test]
fn cargo_package_without_trailing_newline_keeps_absence() {
    let src = without_trailing_newline(CARGO_PACKAGE);
    assert!(!src.ends_with('\n'));
    let out = bump_cargo_package(src);
    assert!(!out.ends_with('\n'), "{out:?}");
    assert_eq!(out, expect_cargo_package(src));
}

#[test]
fn json_package_keeps_comments_tabs_and_neighbors() {
    assert_exclusive_lf(PACKAGE_JSON);
    assert_eq!(
        bump_json_package(PACKAGE_JSON),
        expect_json_package(PACKAGE_JSON)
    );
}

#[test]
fn json_package_keeps_crlf() {
    assert_exclusive_crlf(PACKAGE_JSON_CRLF);
    assert_eq!(
        bump_json_package(PACKAGE_JSON_CRLF),
        expect_json_package(PACKAGE_JSON_CRLF)
    );
}

#[test]
fn json_package_without_trailing_newline_keeps_absence() {
    let src = without_trailing_newline(PACKAGE_JSON);
    assert!(!src.ends_with('\n'));
    let out = bump_json_package(src);
    assert!(!out.ends_with('\n'), "{out:?}");
    assert_eq!(out, expect_json_package(src));
}

#[test]
fn json_package_insert_keeps_neighbors() {
    assert_exclusive_lf(PACKAGE_INSERT);
    assert_eq!(
        set_json_string(PACKAGE_INSERT, &["version"], "0.2.0").expect("insert"),
        expect_json_insert(PACKAGE_INSERT)
    );
}

#[test]
fn json_package_insert_keeps_crlf() {
    assert_exclusive_crlf(PACKAGE_INSERT_CRLF);
    assert_eq!(
        set_json_string(PACKAGE_INSERT_CRLF, &["version"], "0.2.0").expect("insert"),
        expect_json_insert(PACKAGE_INSERT_CRLF)
    );
}

#[test]
fn json_package_insert_without_trailing_newline_keeps_absence() {
    let src = without_trailing_newline(PACKAGE_INSERT);
    assert!(!src.ends_with('\n'));
    let out = set_json_string(src, &["version"], "0.2.0").expect("insert");
    assert!(!out.ends_with('\n'), "{out:?}");
    assert_eq!(out, expect_json_insert(src));
}

#[test]
fn json_package_insert_crlf_without_trailing_newline_keeps_absence() {
    let src = without_trailing_newline(PACKAGE_INSERT_CRLF);
    assert!(!src.ends_with('\n'));
    let out = set_json_string(src, &["version"], "0.2.0").expect("insert");
    assert!(!out.ends_with('\n'), "{out:?}");
    assert_eq!(out, expect_json_insert(src));
}

#[test]
fn cargo_dep_keeps_comments_tabs_and_neighbors() {
    assert_exclusive_lf(CARGO_DEP);
    assert_eq!(bump_cargo_dep(CARGO_DEP), expect_cargo_dep(CARGO_DEP));
}

#[test]
fn cargo_dep_keeps_crlf() {
    assert_exclusive_crlf(CARGO_DEP_CRLF);
    assert_eq!(
        bump_cargo_dep(CARGO_DEP_CRLF),
        expect_cargo_dep(CARGO_DEP_CRLF)
    );
}

#[test]
fn cargo_dep_without_trailing_newline_keeps_absence() {
    let src = without_trailing_newline(CARGO_DEP);
    assert!(!src.ends_with('\n'));
    let out = bump_cargo_dep(src);
    assert!(!out.ends_with('\n'), "{out:?}");
    assert_eq!(out, expect_cargo_dep(src));
}

#[test]
fn cargo_dep_table_keeps_path_comment_and_neighbors() {
    assert_exclusive_lf(CARGO_DEP_TABLE);
    assert_eq!(
        bump_cargo_dep(CARGO_DEP_TABLE),
        expect_cargo_dep_table(CARGO_DEP_TABLE)
    );
}

#[test]
fn cargo_dep_table_keeps_crlf() {
    assert_exclusive_crlf(CARGO_DEP_TABLE_CRLF);
    assert_eq!(
        bump_cargo_dep(CARGO_DEP_TABLE_CRLF),
        expect_cargo_dep_table(CARGO_DEP_TABLE_CRLF)
    );
}

#[test]
fn cargo_dep_table_without_trailing_newline_keeps_absence() {
    let src = without_trailing_newline(CARGO_DEP_TABLE);
    assert!(!src.ends_with('\n'));
    let out = bump_cargo_dep(src);
    assert!(!out.ends_with('\n'), "{out:?}");
    assert_eq!(out, expect_cargo_dep_table(src));
}

#[test]
fn cargo_dep_table_crlf_without_trailing_newline_keeps_absence() {
    let src = without_trailing_newline(CARGO_DEP_TABLE_CRLF);
    assert!(!src.ends_with('\n'));
    let out = bump_cargo_dep(src);
    assert!(!out.ends_with('\n'), "{out:?}");
    assert_eq!(out, expect_cargo_dep_table(src));
}

#[test]
fn npm_dep_keeps_comments_tabs_and_neighbors() {
    assert_exclusive_lf(PACKAGE_DEP);
    assert_eq!(bump_npm_dep(PACKAGE_DEP), expect_npm_dep(PACKAGE_DEP));
}

#[test]
fn npm_dep_keeps_crlf() {
    assert_exclusive_crlf(PACKAGE_DEP_CRLF);
    assert_eq!(
        bump_npm_dep(PACKAGE_DEP_CRLF),
        expect_npm_dep(PACKAGE_DEP_CRLF)
    );
}

#[test]
fn npm_dep_without_trailing_newline_keeps_absence() {
    let src = without_trailing_newline(PACKAGE_DEP);
    assert!(!src.ends_with('\n'));
    let out = bump_npm_dep(src);
    assert!(!out.ends_with('\n'), "{out:?}");
    assert_eq!(out, expect_npm_dep(src));
}

#[test]
fn cargo_lock_keeps_header_and_registry_specs() {
    assert_exclusive_lf(CARGO_LOCK);
    assert_eq!(bump_cargo_lock(CARGO_LOCK), expect_cargo_lock(CARGO_LOCK));
}

#[test]
fn cargo_lock_keeps_crlf() {
    assert_exclusive_crlf(CARGO_LOCK_CRLF);
    assert_eq!(
        bump_cargo_lock(CARGO_LOCK_CRLF),
        expect_cargo_lock(CARGO_LOCK_CRLF)
    );
}

#[test]
fn cargo_lock_without_trailing_newline_keeps_absence() {
    let src = without_trailing_newline(CARGO_LOCK);
    assert!(!src.ends_with('\n'));
    let out = bump_cargo_lock(src);
    assert!(!out.ends_with('\n'), "{out:?}");
    assert_eq!(out, expect_cargo_lock(src));
}
