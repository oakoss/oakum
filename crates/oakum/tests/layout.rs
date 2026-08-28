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

/// `lefthook.yml` reads the edition because rustfmt does not read a manifest.
/// The pattern is line-anchored against this file, so moving the key into a
/// member — where `edition.workspace = true` is the natural spelling — hands it
/// an empty capture, and rustfmt then formats staged files under the wrong
/// edition.
///
/// The grepped value is compared against the one cargo resolves rather than
/// merely required to be non-empty. `grep -m1` takes the first matching line in
/// the whole file, so an unrelated table above `[workspace.package]` carrying
/// the same key silently wins, and the hook guards only against empty.
#[test]
fn the_grep_that_reads_the_root_manifest_finds_the_value_cargo_resolves() {
    let (path, manifest) = root_manifest();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} should be readable: {e}", path.display()));

    // The hook's own pattern, not a tightened version of it: a stricter prefix
    // here would skip a first match the hook would take.
    let prefix = "edition = \"";
    let grepped = text
        .lines()
        .find(|line| line.starts_with(prefix))
        .and_then(|line| line.split('"').nth(1))
        .unwrap_or_else(|| {
            panic!(
                "the first line in {} starting `{prefix}` yields no quoted value, so \
                 lefthook's rustfmt hook acts on an empty string",
                path.display()
            )
        });

    assert_eq!(
        Some(grepped),
        manifest["workspace"]["package"]["edition"].as_str(),
        "the first `{prefix}` line in {} is not the edition cargo resolves, so \
         lefthook's rustfmt hook formats staged files under another one",
        path.display()
    );
}

/// ADR-0025's whole content: `rust-version` and `.mise.toml`'s pin are one
/// number. Nothing else enforces it, and the drift Renovate can produce is the
/// silent direction — a pin bump edits `.mise.toml` alone (`rust-version` is not
/// a dependency its cargo manager sees), leaving a floor *below* the toolchain,
/// which every check passes.
#[test]
fn the_declared_floor_equals_the_pinned_toolchain() {
    let (manifest_path, manifest) = root_manifest();
    let root = support::workspace_root();
    let mise_path = root.join(".mise.toml");
    let text = std::fs::read_to_string(&mise_path)
        .unwrap_or_else(|e| panic!("{} should be readable: {e}", mise_path.display()));
    let mise: toml::Value = toml::from_str(&text)
        .unwrap_or_else(|e| panic!("{} should parse: {e}", mise_path.display()));

    // `rust = "1.97.1"` and `rust = { version = "1.97.1", ... }` are both valid,
    // and Renovate may rewrite one into the other.
    let tool = mise
        .get("tools")
        .and_then(|tools| tools.get("rust"))
        .unwrap_or_else(|| panic!("{} declares no [tools] rust", mise_path.display()));
    let pinned = tool
        .as_str()
        .or_else(|| tool.get("version")?.as_str())
        .unwrap_or_else(|| panic!("{}'s rust pin carries no version", mise_path.display()));

    assert_eq!(
        manifest["workspace"]["package"]["rust-version"].as_str(),
        Some(pinned),
        "{} declares a floor that is not {}'s pin of {pinned}. A floor below the \
         pin passes every check while oakum tests a compiler it does not claim \
         to support — the split ADR-0025 removed",
        manifest_path.display(),
        mise_path.display()
    );

    // The third copy: `rust-toolchain.toml` is what CI release runners honor.
    // A Renovate bump that edits the other two alone leaves releases building
    // a different compiler than every local check, silently.
    let toolchain_path = root.join("rust-toolchain.toml");
    let text = std::fs::read_to_string(&toolchain_path)
        .unwrap_or_else(|e| panic!("{} should be readable: {e}", toolchain_path.display()));
    let toolchain: toml::Value = toml::from_str(&text)
        .unwrap_or_else(|e| panic!("{} should parse: {e}", toolchain_path.display()));
    let channel = toolchain
        .get("toolchain")
        .and_then(|t| t.get("channel"))
        .and_then(|c| c.as_str())
        .unwrap_or_else(|| {
            panic!(
                "{} declares no [toolchain] channel",
                toolchain_path.display()
            )
        });
    assert_eq!(
        channel,
        pinned,
        "{} pins {channel}, not {}'s {pinned}; CI release builds would use a \
         different compiler than every local check",
        toolchain_path.display(),
        mise_path.display()
    );
}

/// `ci-summary` gates on the jobs in its `needs`, so a job missing from that list
/// runs and is ignored. The loud direction is covered — naming a deleted job in
/// `needs` is a GitHub config error actionlint catches — but adding a job and
/// forgetting to gate it fails nowhere.
#[test]
fn the_ci_summary_gates_on_every_other_job() {
    let root = support::workspace_root();
    let path = root.join(".github/workflows/ci.yml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} should be readable: {e}", path.display()));

    // Parsed by indentation rather than with a YAML crate: a job is the only
    // two-space key under `jobs:`, and adding a dependency to read one file is a
    // worse trade than this shape.
    // Split rather than stripped: a trailing comment on the key would make
    // `strip_suffix(':')` drop that job from the list entirely, and a job this
    // test never sees is a job it reports as gated.
    let jobs: Vec<&str> = text
        .lines()
        .skip_while(|line| *line != "jobs:")
        .filter_map(|line| {
            let (name, rest) = line.strip_prefix("  ")?.split_once(':')?;
            let bare = !name.is_empty()
                && !name.starts_with('#')
                && !name.contains(char::is_whitespace)
                && (rest.trim().is_empty() || rest.trim_start().starts_with('#'));
            bare.then_some(name)
        })
        .collect();
    assert!(
        jobs.len() > 2,
        "found {} jobs in {}, so this parse is reading the wrong thing",
        jobs.len(),
        path.display()
    );

    // Anchored to the summary's own block: the first `needs:` in the file belongs
    // to whichever job declares one, and reading another job's list would report
    // gated jobs as ungated.
    let needs = text
        .lines()
        .skip_while(|line| *line != "  ci-summary:")
        .find_map(|line| line.trim().strip_prefix("needs: ["))
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or_else(|| {
            panic!(
                "{} gives ci-summary no single-line needs list",
                path.display()
            )
        });
    let gated: Vec<&str> = needs.split(',').map(str::trim).collect();

    for job in &jobs {
        if *job == "ci-summary" {
            continue;
        }
        assert!(
            gated.contains(job),
            "{} runs `{job}` but CI Summary does not gate on it, so it can fail \
             while the required check stays green",
            path.display()
        );
    }
}

/// `edition` is read from the root by lefthook, and `rust-version` is the number
/// ADR-0025 keeps equal to `.mise.toml`'s pin. A member that sets either itself
/// leaves both claims true of the root while that package builds with something
/// else.
#[test]
fn every_member_inherits_the_shared_toolchain_keys() {
    let (_, members) = support::workspace();

    for member in members {
        let path = member.join("Cargo.toml");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} should be readable: {e}", path.display()));
        let manifest: toml::Value = toml::from_str(&text)
            .unwrap_or_else(|e| panic!("{} should parse: {e}", path.display()));

        for key in ["edition", "rust-version"] {
            assert_eq!(
                manifest
                    .get("package")
                    .and_then(|package| package.get(key))
                    .and_then(|value| value.get("workspace"))
                    .and_then(toml::Value::as_bool),
                Some(true),
                "{} does not inherit {key} from the workspace, so the root declares \
                 one value while this package builds with another",
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
