// The workspace layout ADR-0002 records, asserted rather than assumed. Splitting
// one manifest into a virtual root plus members put the lint levels in a
// different file from the members that opt into them, and both halves fail
// silently: a root that stops declaring `unsafe_code = "forbid"` and a member
// that stops forwarding to it both compile green. The two line-anchored greps
// that read the root manifest fail the same way, handing their callers a value
// the build does not use.
#![allow(clippy::disallowed_methods)]

mod support;

use std::path::PathBuf;

fn root_manifest() -> (PathBuf, toml::Value) {
    let path = support::workspace_root().join("Cargo.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} should be readable: {e}", path.display()));
    // `expect` would render the error with `Debug`, which embeds the whole file.
    let manifest =
        toml::from_str(&text).unwrap_or_else(|e| panic!("{} should parse: {e}", path.display()));
    (path, manifest)
}

/// A `[package]` here would make the root a member of its own workspace, which
/// changes what `cargo` selects by default and so what every task in
/// `.mise.toml` builds.
#[test]
fn the_root_manifest_is_virtual() {
    let (path, manifest) = root_manifest();

    assert!(
        manifest.get("package").is_none(),
        "{} declares a [package], so the root is no longer a virtual manifest",
        path.display()
    );
}

/// Cargo accepts a bare string or a table carrying `priority`; the root manifest
/// already uses both spellings, so reading only the string form would report a
/// strengthening as a disarm.
fn lint_level(value: &toml::Value) -> Option<&str> {
    value.as_str().or_else(|| value.get("level")?.as_str())
}

/// Half of the wiring. `deny` reads as the obvious value to weaken and `allow`
/// is the spelling that disarms rather than relaxes: deleting the line leaves
/// `clippy::all`'s warn-by-default plus `-D warnings` as a backstop, while
/// `allow` silences the lint outright.
#[test]
fn the_root_declares_the_lint_levels_members_forward_to() {
    let (path, manifest) = root_manifest();

    for (group, lint, level) in [
        ("rust", "unsafe_code", "forbid"),
        ("clippy", "disallowed_methods", "deny"),
    ] {
        // Indexed with `get` rather than `[]`, whose panic on a missing
        // intermediate key reads `index not found` and names neither the file nor
        // the lint — and a deleted `[workspace.lints.clippy]` is a manifest cargo
        // accepts.
        let declared = manifest
            .get("workspace")
            .and_then(|w| w.get("lints"))
            .and_then(|lints| lints.get(group))
            .and_then(|group| group.get(lint))
            .and_then(lint_level);

        assert_eq!(
            declared,
            Some(level),
            "{} no longer sets {group}.{lint} to {level}, so every member forwards \
             to a level that does not enforce it",
            path.display()
        );
    }
}

/// Priority decides the order cargo emits the flags in, and the last one wins.
/// `all = { level = "allow", priority = 1 }` leaves `disallowed_methods = "deny"`
/// sitting untouched in the manifest while a trailing `-A clippy::all` overrides
/// it, so asserting the entry's own level is not enough.
#[test]
fn no_clippy_lint_group_outranks_the_deny_it_would_override() {
    let (path, manifest) = root_manifest();

    let groups = manifest["workspace"]["lints"]["clippy"]
        .as_table()
        .unwrap_or_else(|| {
            panic!(
                "{}'s [workspace.lints.clippy] is not a table",
                path.display()
            )
        });

    for (name, entry) in groups {
        if name == "disallowed_methods" {
            continue;
        }
        // A bare string carries priority 0, which ties with the deny and leaves
        // the order unspecified.
        let priority = entry.get("priority").and_then(toml::Value::as_integer);
        assert!(
            priority.is_some_and(|p| p < 0),
            "{}'s [workspace.lints.clippy] entry `{name}` does not carry a negative \
             priority, so cargo may emit it after disallowed_methods and override it",
            path.display()
        );
    }
}

/// The other half. Deleting a member's opt-in leaves the build green:
/// `unsafe_code = "forbid"` stops applying outright, and `disallowed_methods`
/// drops from `deny` to warn, surviving only because `mise run check` passes
/// `-D warnings`.
///
/// Iterated over the manifest's own member list rather than a list restated
/// here, so a member added later is covered the day it lands.
#[test]
fn every_member_forwards_to_the_workspace_lint_levels() {
    let (_, members) = support::workspace();
    assert!(!members.is_empty(), "the workspace has no members to check");

    for member in members {
        let path = member.join("Cargo.toml");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} should be readable: {e}", path.display()));
        let manifest: toml::Value = toml::from_str(&text)
            .unwrap_or_else(|e| panic!("{} should parse: {e}", path.display()));

        assert_eq!(
            manifest
                .get("lints")
                .and_then(|lints| lints.get("workspace"))
                .and_then(toml::Value::as_bool),
            Some(true),
            "{} does not forward to [workspace.lints], so unsafe_code = forbid \
             and disallowed_methods = deny do not apply to it",
            path.display()
        );
    }
}

/// `lefthook.yml` reads the edition because rustfmt does not read a manifest,
/// and `.github/workflows/ci.yml` reads the floor to install the MSRV toolchain.
/// Both patterns are line-anchored against this file, so moving either key into
/// a member — where `<key>.workspace = true` is the natural spelling — hands
/// them an empty capture.
///
/// The grepped value is compared against the one cargo resolves rather than
/// merely required to be non-empty. `grep -m1` takes the first matching line in
/// the whole file, so an unrelated table above `[workspace.package]` carrying
/// the same key silently wins, and both consumers guard only against empty.
#[test]
fn the_greps_that_read_the_root_manifest_find_the_value_cargo_resolves() {
    let (path, manifest) = root_manifest();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} should be readable: {e}", path.display()));

    // Each consumer's own pattern, not a tightened version of it. CI anchors on
    // `^rust-version` alone, so `rust-version.workspace = true` sitting above the
    // real line is a first match this test would skip and CI would not.
    for (key, prefix, consumer) in [
        ("edition", "edition = \"", "lefthook.yml's rustfmt hook"),
        ("rust-version", "rust-version", "ci.yml's MSRV job"),
    ] {
        let grepped = text
            .lines()
            .find(|line| line.starts_with(prefix))
            .and_then(|line| line.split('"').nth(1))
            .unwrap_or_else(|| {
                panic!(
                    "the first line in {} starting `{prefix}` yields no quoted value, \
                     so {consumer} acts on an empty string",
                    path.display()
                )
            });

        assert_eq!(
            Some(grepped),
            manifest["workspace"]["package"][key].as_str(),
            "the first `{prefix}` line in {} is not the value cargo resolves, so \
             {consumer} acts on one the build never uses",
            path.display()
        );
    }
}

/// The greps above read the root. A member that overrides either key directly
/// keeps them accurate about the root while the build uses something else, so
/// inheritance is the property that makes reading the root correct at all.
#[test]
fn every_member_inherits_the_keys_those_greps_read() {
    let (_, members) = support::workspace();

    for member in members {
        let path = member.join("Cargo.toml");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} should be readable: {e}", path.display()));
        let manifest: toml::Value = toml::from_str(&text)
            .unwrap_or_else(|e| panic!("{} should parse: {e}", path.display()));

        for key in ["edition", "rust-version"] {
            assert_eq!(
                manifest["package"][key]
                    .get("workspace")
                    .and_then(toml::Value::as_bool),
                Some(true),
                "{} sets {key} itself instead of inheriting, so lefthook and CI read \
                 a value from the root that this package does not build with",
                path.display()
            );
        }
    }
}

// One disarm this file cannot guard: `autotests = false` in the member manifest,
// or an explicit `[[test]]` list, drops every file in `tests/` from the build.
// `mise run test` then runs the unit tests alone and exits 0 — measured
// 2026-08-20, 2 targets built instead of 6. An assertion here goes with them, so
// it reports nothing. Catching it needs something outside `tests/`, and the one
// place that survives is a `#[cfg(test)]` module under `src/`, which would have
// to read a file and so carry the I/O opt-out marker ADR-0002 counts as its
// split trigger.
