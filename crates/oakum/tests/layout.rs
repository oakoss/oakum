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

/// dprint's toml plugin forces a space after `#` (`#:schema` → `# :schema`),
/// which breaks the taplo pragma `oakum init` writes. Under `.changeset/**`,
/// turn that off so `mise run check` / lefthook can still format the directory.
#[test]
fn dprint_keeps_changeset_schema_pragma_without_leading_space() {
    let path = support::workspace_root().join("dprint.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} should be readable: {e}", path.display()));
    let config: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("{} should parse: {e}", path.display()));

    let excludes = config
        .get("excludes")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("{} has no excludes array", path.display()));
    assert!(
        !excludes
            .iter()
            .filter_map(|v| v.as_str())
            .any(|entry| entry == ".changeset/**"),
        "{} must not exclude \".changeset/**\"; formatting stays on with a \
         comment.forceLeadingSpace override",
        path.display()
    );

    let overrides = config
        .pointer("/toml/overrides")
        .unwrap_or_else(|| panic!("{} missing toml.overrides", path.display()));
    let files = overrides
        .get("files")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("{} toml.overrides has no files array", path.display()));
    assert!(
        files
            .iter()
            .filter_map(|v| v.as_str())
            .any(|entry| entry == ".changeset/**"),
        "{} toml.overrides.files must include \".changeset/**\"",
        path.display()
    );
    assert_eq!(
        overrides
            .get("comment.forceLeadingSpace")
            .and_then(serde_json::Value::as_bool),
        Some(false),
        "{} must set comment.forceLeadingSpace = false under .changeset/** so \
         #:schema is not rewritten to # :schema",
        path.display()
    );

    // Config alone is not enough: a rewritten pragma stays green if the override
    // is restored later. Assert the `#:schema` spelling the override protects.
    let config_toml = support::workspace_root().join(".changeset/_config.toml");
    let config_text = std::fs::read_to_string(&config_toml)
        .unwrap_or_else(|e| panic!("{} should be readable: {e}", config_toml.display()));
    assert!(
        config_text.starts_with("#:schema"),
        "{} must begin with #:schema (no space after #); dprint's default \
         comment.forceLeadingSpace rewrites that to # :schema",
        config_toml.display()
    );
}

/// Self-host: `[tasks.oakum]` runs the workspace binary, not a `[tools]` pin (ADR-0007).
#[test]
fn mise_oakum_task_runs_the_workspace_binary() {
    let path = support::workspace_root().join(".mise.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} should be readable: {e}", path.display()));
    let mise: toml::Value =
        toml::from_str(&text).unwrap_or_else(|e| panic!("{} should parse: {e}", path.display()));

    let tools = mise.get("tools").and_then(toml::Value::as_table);
    if let Some(tools) = tools {
        assert!(
            !tools.contains_key("oakum") && !tools.contains_key("cargo:oakum"),
            "{} must not pin oakum under [tools]; self-host uses [tasks.oakum]",
            path.display()
        );
    }

    let runs = support::task_commands(&mise, "oakum");
    assert!(
        runs.iter().any(|command| {
            command.contains("cargo run")
                && selects_cargo_package(command, "oakum")
                && command.trim_end().ends_with("--")
        }),
        "[tasks.oakum] must run `cargo run … -p oakum --` so args pass through; got {runs:?}"
    );
}

/// `-p oakum` as a cargo package selector, not a prefix of `-p oakum-core`.
fn selects_cargo_package(command: &str, package: &str) -> bool {
    let needle = format!("-p {package}");
    let Some(idx) = command.find(&needle) else {
        return false;
    };
    !matches!(
        command[idx + needle.len()..].chars().next(),
        Some(c) if c.is_ascii_alphanumeric() || c == '_' || c == '-'
    )
}

/// `.github/workflows/oakum.yml`: main-push version-pr and release only; PR check is in ci.yml.
#[test]
fn oakum_workflow_dogfoods_the_workspace_binary() {
    let path = support::workspace_root().join(".github/workflows/oakum.yml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} should be readable: {e}", path.display()));

    assert!(
        !text.contains("pull_request:"),
        "{} must not run on pull requests; oakum check lives in ci.yml",
        path.display()
    );
    assert!(
        !text.contains("cargo binstall") && !text.contains("cargo install oakum"),
        "{} must not install oakum from a registry; self-host uses mise run oakum",
        path.display()
    );
    assert!(
        text.contains("mise run oakum -- ci version-pr")
            && text.contains("mise run oakum -- release"),
        "{} must invoke version-pr and release via [tasks.oakum]",
        path.display()
    );
    assert!(
        !text.contains("mise run oakum -- check") && !text.contains("ci pr-status"),
        "{} must not run check or pr-status; those live in ci.yml",
        path.display()
    );
    let before_release = text.split("  release:").next().unwrap_or("");
    assert!(
        before_release.contains("./.github/actions/app-token")
            && before_release.contains("GITHUB_TOKEN: ${{ steps.app-token.outputs.token }}")
            && !before_release.contains("secrets.GITHUB_TOKEN"),
        "{} version job must use the App token so the version PR runs CI (not github-actions[bot])",
        path.display()
    );
    assert!(
        text.contains("./.github/actions/app-token")
            && text.contains("resolve-identity: true")
            && text.contains("steps.app-token.outputs.token"),
        "{} release job must use the App token so tag pushes start cargo-dist",
        path.display()
    );
    let release_section = text.split("  release:").nth(1).unwrap_or("");
    assert!(
        release_section.contains("token: ${{ steps.app-token.outputs.token }}")
            && release_section.contains("GITHUB_TOKEN: ${{ steps.app-token.outputs.token }}")
            && release_section.contains("persist-credentials: true")
            && !release_section.contains("secrets.GITHUB_TOKEN"),
        "{} release checkout and env must wire the App token through, not GITHUB_TOKEN",
        path.display()
    );
    assert!(
        text.matches("fetch-depth: 0").count() >= 2,
        "{} must fetch full history in every job that runs oakum",
        path.display()
    );
}

/// `.github/actions/app-token`: one SHA pin for create-github-app-token org-wide.
#[test]
fn app_token_action_pins_create_github_app_token() {
    let path = support::workspace_root().join(".github/actions/app-token/action.yml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} should be readable: {e}", path.display()));
    assert_eq!(
        text.matches("actions/create-github-app-token@").count(),
        1,
        "{} must pin create-github-app-token exactly once",
        path.display()
    );
    assert!(
        text.contains("owner: oakoss")
            && text.contains("secrets.BOT_CLIENT_ID")
            && text.contains("secrets.BOT_PRIVATE_KEY"),
        "{} must wire oakoss App secrets",
        path.display()
    );
}

/// PR dogfood: workspace binary check and pr-status in ci.yml, gated by CI Summary.
#[test]
fn ci_workflow_dogfoods_oakum_check_on_pull_requests() {
    let path = support::workspace_root().join(".github/workflows/ci.yml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} should be readable: {e}", path.display()));

    let static_analysis = text
        .split("  static-analysis:")
        .nth(1)
        .and_then(|tail| tail.split("  tests:").next())
        .unwrap_or("");
    assert!(
        static_analysis.contains("mise run oakum -- check"),
        "{} static-analysis must run oakum check",
        path.display()
    );
    assert!(
        static_analysis.contains("mise run oakum -- ci pr-status"),
        "{} static-analysis must run ci pr-status",
        path.display()
    );
    assert!(
        static_analysis.contains(
            "if: github.event_name == 'pull_request'\n        run: mise run oakum -- check"
        ),
        "{} oakum check must run only on pull requests",
        path.display()
    );
    assert!(
        static_analysis.contains(
            "if: github.event_name == 'pull_request' && (success() || failure())\n        continue-on-error: true\n        run: mise run oakum -- ci pr-status"
        ),
        "{} pr-status must run after check on pull requests (ADR-0015)",
        path.display()
    );
    assert!(
        static_analysis.contains("pull-requests: write"),
        "{} static-analysis needs pull-requests: write for pr-status (ADR-0015)",
        path.display()
    );
    assert!(
        static_analysis.contains("./.github/actions/app-token"),
        "{} static-analysis must mint an App token for pr-status (ADR-0015)",
        path.display()
    );
    let pr_status_block = static_analysis
        .split("mise run oakum -- ci pr-status")
        .nth(1)
        .unwrap_or("");
    assert!(
        pr_status_block.contains("steps.app-token.outputs.token")
            && !pr_status_block.contains("secrets.GITHUB_TOKEN"),
        "{} pr-status must use the App token, not github.token",
        path.display()
    );
}

/// Dogfood lock: bump-files only, tool-version lockstep with crates/oakum,
/// zero-major. `oakum init` writes conventional-commits true; this repo sets
/// false, and nothing else pins that.
#[test]
fn dogfood_changeset_config_matches_bump_files_only_lock() {
    let root = support::workspace_root();
    let path = root.join(".changeset/_config.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} should be readable: {e}", path.display()));
    let config: toml::Value =
        toml::from_str(&text).unwrap_or_else(|e| panic!("{} should parse: {e}", path.display()));

    assert_eq!(
        config
            .get("conventional-commits")
            .and_then(toml::Value::as_bool),
        Some(false),
        "{} must keep conventional-commits = false (bump-files only)",
        path.display()
    );
    assert_eq!(
        config.get("change-files").and_then(toml::Value::as_bool),
        Some(true),
        "{} must keep change-files = true",
        path.display()
    );
    assert_eq!(
        config.get("versioning").and_then(toml::Value::as_str),
        Some("zero-major"),
        "{} must keep versioning = \"zero-major\"",
        path.display()
    );
    assert!(
        config.get("tag-format").is_none(),
        "{} must omit tag-format (dogfood leaves the default)",
        path.display()
    );
    assert!(
        config.get("private-packages").is_none(),
        "{} must omit private-packages (dogfood leaves the default)",
        path.display()
    );

    let member = root.join("crates/oakum/Cargo.toml");
    let member_text = std::fs::read_to_string(&member)
        .unwrap_or_else(|e| panic!("{} should be readable: {e}", member.display()));
    let member_manifest: toml::Value = toml::from_str(&member_text)
        .unwrap_or_else(|e| panic!("{} should parse: {e}", member.display()));
    let package_version = member_manifest
        .get("package")
        .and_then(|p| p.get("version"))
        .and_then(toml::Value::as_str)
        .unwrap_or_else(|| panic!("{} missing package.version", member.display()));
    assert_eq!(
        config.get("tool-version").and_then(toml::Value::as_str),
        Some(package_version),
        "{} tool-version must match crates/oakum package.version ({package_version})",
        path.display()
    );
}

/// Drop blank lines and `#` comment lines so layout needles cannot hide in comments.
fn executable_shell_lines(script: &str) -> String {
    script
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !trimmed.is_empty() && !trimmed.starts_with('#')
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn first_executable_line_containing<'a>(script: &'a str, needle: &str) -> Option<&'a str> {
    script.lines().find(|line| {
        let trimmed = line.trim_start();
        !trimmed.starts_with('#') && line.contains(needle)
    })
}

/// cargo-dist uploads into the GitHub Release oakum already created (linesmith shape).
#[test]
fn release_workflow_uploads_into_existing_github_release() {
    let path = support::workspace_root().join(".github/workflows/release.yml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} should be readable: {e}", path.display()));

    let host = text
        .split("\n  host:")
        .nth(1)
        .and_then(|tail| tail.split("\n  publish-").next())
        .unwrap_or("");
    let upload_step = host
        .split("- name: Upload artifacts to release")
        .nth(1)
        .unwrap_or("");
    let code = executable_shell_lines(upload_step);

    assert!(
        upload_step.contains("\n          TAG:"),
        "{} upload step env must set TAG",
        path.display()
    );
    let empty_tag_at = code
        .find("[[ -z \"${TAG}\" ]]")
        .unwrap_or_else(|| panic!("{} missing empty-TAG guard", path.display()));
    let view_at = code
        .find("if view_err=$(gh release view")
        .unwrap_or_else(|| panic!("{} missing gh release view probe", path.display()));
    assert!(
        empty_tag_at < view_at,
        "{} empty-TAG guard must precede the release view probe",
        path.display()
    );
    assert!(
        first_executable_line_containing(
            upload_step,
            "gh release upload \"$TAG\" artifacts/* --clobber"
        )
        .is_some(),
        "{} host job must upload with --clobber on an executable line",
        path.display()
    );

    let edit_lines: Vec<&str> = upload_step
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !trimmed.starts_with('#') && line.contains("gh release edit")
        })
        .collect();
    assert!(
        !edit_lines.is_empty(),
        "{} missing executable gh release edit",
        path.display()
    );
    for edit_line in &edit_lines {
        assert!(
            !edit_line.contains("$LATEST_FLAG")
                && !edit_line.contains("${LATEST_FLAG}")
                && !edit_line.contains("--latest"),
            "{} edit line must not pass --latest / $LATEST_FLAG: {edit_line}",
            path.display()
        );
    }

    let create_line = first_executable_line_containing(upload_step, "gh release create")
        .unwrap_or_else(|| panic!("{} missing executable gh release create", path.display()));
    assert!(
        create_line.contains("$LATEST_FLAG") || create_line.contains("${LATEST_FLAG}"),
        "{} create line must pass $LATEST_FLAG: {create_line}",
        path.display()
    );

    assert!(
        first_executable_line_containing(
            upload_step,
            "elif [[ \"$view_err\" == *\"release not found\"* ]]"
        )
        .is_some(),
        "{} must gate create on an executable release-not-found elif",
        path.display()
    );
    assert!(
        first_executable_line_containing(upload_step, "::error::gh release view failed").is_some(),
        "{} must fail the probe on an executable error line",
        path.display()
    );
    assert!(
        code.contains("set -euo pipefail") && code.contains("inherit_errexit"),
        "{} host upload step must fail-fast",
        path.display()
    );

    let verify_def = upload_step
        .split("verify_assets_present()")
        .nth(1)
        .and_then(|tail| tail.split("\n          }").next())
        .unwrap_or("");
    let verify_code = executable_shell_lines(verify_def);
    assert!(
        verify_code.contains("grep -qxF")
            && verify_code.contains("basename")
            && verify_code.contains(".assets[].name"),
        "{} verify_assets_present must keep name-based asset checks",
        path.display()
    );

    let upload_arm = code
        .split("elif [[ \"$view_err\" == *\"release not found\"* ]]")
        .next()
        .and_then(|head| head.split("if view_err=").nth(1))
        .unwrap_or("");
    let create_arm = code
        .split("elif [[ \"$view_err\" == *\"release not found\"* ]]")
        .nth(1)
        .and_then(|tail| tail.split("else").next())
        .unwrap_or("");
    assert_eq!(
        upload_arm.matches("verify_assets_present \"$TAG\"").count(),
        1,
        "{} upload arm must call verify_assets_present once",
        path.display()
    );
    assert_eq!(
        create_arm.matches("verify_assets_present \"$TAG\"").count(),
        1,
        "{} create arm must call verify_assets_present once",
        path.display()
    );
}

// One disarm this file cannot guard: `autotests = false` in the member manifest,
// or an explicit `[[test]]` list, drops every file in `tests/` from the build.
// `mise run test` then runs the unit tests alone and exits 0 — measured
// 2026-08-20, 2 targets built instead of 6. An assertion here goes with them, so
// it reports nothing. Catching it needs something outside `tests/`, and the one
// place that survives is a `#[cfg(test)]` module under `src/`, which would have
// to read a file and so carry the I/O opt-out marker ADR-0002 counts as its
// split trigger.
