// The tripwire in clippy.toml enforces ADR-0002's I/O boundary, and its failure
// modes are silent: delete the file and clippy reports nothing, or mistype one
// path and that entry alone stops firing. Neither fails under `-D warnings`,
// because an unresolvable path is reported by clippy's config loader rather
// than by the lint system. Both leave CI green.
//
// These tests drive clippy-driver directly against probe files. No cargo, no
// fixture crate, no network.
#![allow(clippy::disallowed_methods)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

/// Only `std`, `core`, and `alloc` are in scope for the bare probe crate these
/// tests compile. A path rooted anywhere else resolves against nothing, and
/// clippy says nothing about it — so `every_denylist_path_resolves` cannot see
/// it, and would certify it.
const PROBEABLE_ROOTS: [&str; 3] = ["std", "core", "alloc"];

fn driver() -> PathBuf {
    let sysroot = Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()
        .expect("rustc should be on PATH");
    let root = String::from_utf8(sysroot.stdout).expect("sysroot should be utf-8");
    let exe = if cfg!(windows) {
        "clippy-driver.exe"
    } else {
        "clippy-driver"
    };
    let in_sysroot = Path::new(root.trim()).join("bin").join(exe);
    if in_sysroot.exists() {
        return in_sysroot;
    }

    // Some packagings ship the driver outside the sysroot. Missing entirely
    // means unverified, and unverified must not read as passing.
    let on_path = Command::new(exe).arg("--version").output();
    assert!(
        on_path.is_ok_and(|o| o.status.success()),
        "clippy-driver not in this toolchain's sysroot ({}) and not on PATH; \
         the clippy component is required",
        in_sysroot.display()
    );
    PathBuf::from(exe)
}

/// Both spellings clippy accepts. Checking only one would reject a rename that
/// clippy itself is fine with.
const CONFIG_NAMES: [&str; 2] = ["clippy.toml", ".clippy.toml"];

fn config_root() -> PathBuf {
    config().0
}

/// The workspace root and the config clippy loads from it. The lint levels are
/// package-scoped but the config file is not, so it sits at the root and clippy
/// ascends to find it.
///
/// Three preconditions, each closing a way the tests below go green while
/// proving nothing: the file is at the anchored root rather than an ancestor,
/// nothing shadows it in between, and it parses into a non-empty denylist.
/// Searching upward for a config is the failure mode being guarded, not the fix.
fn config() -> (PathBuf, PathBuf) {
    let (root, members) = support::workspace();

    // Both spellings at once is the case that reads as fine and is not: clippy
    // loads `.clippy.toml`, ignores the other, and says so in a warning that
    // `-D warnings` does not escalate.
    let present: Vec<PathBuf> = CONFIG_NAMES
        .iter()
        .map(|name| root.join(name))
        .filter(|path| path.exists())
        .collect();
    assert!(
        present.len() < 2,
        "both config spellings sit at {}; clippy reads .clippy.toml and only warns \
         that the other is ignored, so these tests would audit an unread file",
        root.display()
    );
    let file = present.into_iter().next().unwrap_or_else(|| {
        panic!(
            "no {CONFIG_NAMES:?} at {} — these tests would silently read an ancestor's",
            root.display()
        )
    });

    // Every member rather than this package alone: `--workspace` lints them all,
    // and the probe compiles `plan`'s own sources, so a config beside
    // `plan-no-std` would decide what `plan` is checked against.
    for member in members {
        for dir in member.ancestors().take_while(|dir| *dir != root) {
            for name in CONFIG_NAMES {
                assert!(
                    !dir.join(name).exists(),
                    "{} sits below the workspace root, so clippy reads it first and \
                     these tests audit a file the build does not use",
                    dir.join(name).display()
                );
            }
        }
    }

    let text = std::fs::read_to_string(&file)
        .unwrap_or_else(|e| panic!("{} should be readable: {e}", file.display()));
    let parsed: toml::Value = toml::from_str(&text).unwrap_or_else(|e| {
        panic!(
            "{} is not valid TOML, so clippy loads no denylist from it at all: {e}",
            file.display()
        )
    });
    assert!(
        parsed
            .get("disallowed-methods")
            .and_then(toml::Value::as_array)
            .is_some_and(|entries| !entries.is_empty()),
        "{} carries no disallowed-methods entries",
        file.display()
    );

    (root, file)
}

/// Per-target-dir, so concurrent runs from separate worktrees do not race on
/// one absolute path. Gitignored, and swept by `cargo clean`.
fn scratch(case: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(case);
    std::fs::create_dir_all(&dir).expect("scratch dir should be creatable");
    dir
}

/// The no-config control cannot use `scratch`: `CARGO_TARGET_TMPDIR` is inside
/// the repository, and clippy ascends from `CLIPPY_CONF_DIR` until it finds a
/// config — so it would read the repo's own and the control would prove nothing.
/// The process id keeps concurrent runs apart; `assert_no_config_above` is what
/// makes "no config" a checked precondition rather than a property of the host.
fn scratch_outside_repo(case: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("oakum-io-boundary-{}-{case}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir should be creatable");
    dir
}

fn assert_no_config_above(dir: &Path) {
    for ancestor in dir.ancestors() {
        for name in ["clippy.toml", ".clippy.toml"] {
            assert!(
                !ancestor.join(name).exists(),
                "{} sits above the probe dir, so the no-config control proves nothing",
                ancestor.join(name).display()
            );
        }
    }
}

/// Returns (success, stderr).
fn run(dir: &Path, conf_dir: &Path, source: &str, flags: &[&str]) -> (bool, String) {
    let out = dir.join("out");
    std::fs::create_dir_all(&out).expect("out dir should be creatable");
    let file = dir.join("probe.rs");
    std::fs::write(&file, source).expect("probe should be writable");

    // A real --out-dir is required; `-o /dev/null` makes the driver fail on a
    // temp-dir error and look like a lint failure.
    let output = Command::new(driver())
        .arg(&file)
        .args([
            "--edition",
            "2021",
            "--crate-type",
            "lib",
            "--emit",
            "metadata",
        ])
        .arg("--out-dir")
        .arg(&out)
        .args(flags)
        .env("CLIPPY_CONF_DIR", conf_dir)
        .output()
        .expect("clippy-driver should be runnable");

    (
        output.status.success(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// The lint is warn-by-default through `clippy::all`, so without this the driver
/// exits 0 on a violation and the assertions are vacuous. Scoped to the one lint
/// rather than `-D warnings`, which would escalate unrelated warnings and make
/// failures ambiguous.
const DENY_LINT: [&str; 2] = ["-D", "clippy::disallowed_methods"];

const CALLS_DISALLOWED: &str = "pub fn f() { let _ = std::process::Command::new(\"echo\"); }";

/// Every config-loader diagnostic points at the config file, whatever its
/// wording — "does not refer to a reachable function" for a bad path, "expected
/// a function, found a struct" for one that resolves to the wrong item.
/// Matching the location catches both; matching either phrase catches one.
fn config_diagnostics(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|l| l.contains("clippy.toml:"))
        .collect()
}

/// A nonzero exit is not enough. Clippy's config loader echoes the offending
/// line back when it cannot read the file, and that line contains the very
/// method name a substring check greps for — so a broken denylist produces a
/// failure that looks exactly like the lint firing.
fn assert_the_denylist_fired(stderr: &str) {
    assert!(
        stderr.contains("use of a disallowed method `std::process::Command::new`"),
        "the failure did not come from the denylist:\n{stderr}"
    );
    assert!(
        config_diagnostics(stderr).is_empty(),
        "clippy could not use the config, so the failure proves nothing about \
         the denylist:\n{stderr}"
    );
}

#[test]
fn denylist_is_loaded_and_fires() {
    let (ok, stderr) = run(
        &scratch("fires"),
        &config_root(),
        CALLS_DISALLOWED,
        &DENY_LINT,
    );
    assert!(!ok, "expected a lint failure, got success:\n{stderr}");
    assert_the_denylist_fired(&stderr);
}

/// The test above hands clippy the config's own directory. `cargo clippy` never
/// does: it starts each unit at that unit's package and reaches the file only by
/// ascending out of `crates/oakum`. Nothing else here would notice if it stopped
/// arriving.
#[test]
fn the_build_reaches_the_config_by_ascending_from_this_package() {
    let file = config().1;
    let package = Path::new(env!("CARGO_MANIFEST_DIR"));

    let (ok, stderr) = run(&scratch("ascend"), package, CALLS_DISALLOWED, &DENY_LINT);
    assert!(
        !ok,
        "clippy loaded no denylist starting from {}, so the build lints this \
         package against nothing while {} sits unread:\n{stderr}",
        package.display(),
        file.display()
    );
    assert_the_denylist_fired(&stderr);
}

/// Without this, `denylist_is_loaded_and_fires` could pass for some reason other
/// than the config — proving nothing about clippy.toml.
#[test]
fn the_same_call_passes_with_no_config() {
    let empty = scratch_outside_repo("noconf");
    assert_no_config_above(&empty);

    let (ok, stderr) = run(&empty, &empty, CALLS_DISALLOWED, &DENY_LINT);
    assert!(
        ok,
        "call was rejected without clippy.toml, so the other test proves nothing:\n{stderr}"
    );
}

/// The marker shape the project actually uses. Paired with
/// `a_stale_expect_marker_fails`: with a violation it suppresses, without one
/// it fails.
#[test]
fn an_expect_marker_suppresses_a_real_violation() {
    let source = format!(
        "#[expect(clippy::disallowed_methods, reason = \"io module\")]\n{CALLS_DISALLOWED}"
    );
    let (ok, stderr) = run(&scratch("optout"), &config_root(), &source, &DENY_LINT);
    assert!(
        ok,
        "the marker attribute did not suppress the lint:\n{stderr}"
    );
}

/// clippy.toml prefers `expect` over `allow` for boundary markers because a
/// marker left on a module that no longer performs I/O becomes an unfulfilled
/// expectation. That property is the whole reason for the preference, and it
/// depends on `-D warnings` staying in `mise run check`.
#[test]
fn a_stale_expect_marker_fails() {
    let source = "#[expect(clippy::disallowed_methods)]\npub fn f() {}";
    let (ok, stderr) = run(
        &scratch("stale-expect"),
        &config_root(),
        source,
        &["-D", "warnings"],
    );
    assert!(
        !ok && stderr.contains("unfulfilled"),
        "a marker on a module with no I/O did not fail, so `expect` buys nothing \
         over `allow`:\n{stderr}"
    );
}

/// `a_stale_expect_marker_fails` is only worth anything if the project's own
/// check escalates warnings — an unfulfilled expectation is a warning, so
/// without this the stale marker it proves fatal is merely noisy and the
/// preference for `expect` over `allow` buys nothing.
#[test]
fn the_check_task_escalates_warnings_to_errors() {
    let path = support::workspace_root().join(".mise.toml");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} should be readable: {e}", path.display()));
    // `expect` would render the error with `Debug`, which embeds the whole file.
    let mise: toml::Value =
        toml::from_str(&text).unwrap_or_else(|e| panic!("{} should parse: {e}", path.display()));

    // Split on `#` first: `true # cargo clippy …` matches the substring while the
    // shell runs nothing.
    let clippy: Vec<&str> = support::task_commands(&mise, "check")
        .into_iter()
        .filter_map(|line| line.split('#').next())
        .filter(|command| command.contains("cargo clippy"))
        .collect();
    assert!(
        !clippy.is_empty(),
        "[tasks.check] no longer runs cargo clippy"
    );

    for invocation in clippy {
        // Tokenised: `-Dwarnings` and `-D warnings` are both valid and a
        // substring match on either spelling misses the other.
        let args: Vec<&str> = invocation.split_whitespace().collect();
        assert!(
            args.windows(2).any(|w| w == ["-D", "warnings"]) || args.contains(&"-Dwarnings"),
            "[tasks.check] runs clippy without -D warnings, so a stale \
             `#[expect(clippy::disallowed_methods)]` warns instead of failing:\n{invocation}"
        );
        // Refused wholesale rather than by which lint they name: a later `-A` on
        // `warnings` or an enclosing group outranks the `-D` above it and
        // `--cap-lints` caps the lot, and deciding which spellings overlap means
        // reimplementing clippy's precedence. One obvious shape instead.
        for arg in &args {
            let flag = arg.split('=').next().unwrap_or(arg);
            assert!(
                !flag.starts_with("-A") && !matches!(flag, "--allow" | "--cap-lints"),
                "[tasks.check] relaxes lints with `{arg}`; this invocation is kept to \
                 -D warnings alone so nothing can outrank it:\n{invocation}"
            );
        }
    }
}

/// The silent one. A typo or a std rename disarms an entry with CI green,
/// because this diagnostic comes from the config loader and `-D warnings` does
/// not escalate it.
#[test]
fn every_denylist_path_resolves() {
    let (ok, stderr) = run(
        &scratch("paths"),
        &config_root(),
        "pub fn f() {}",
        &DENY_LINT,
    );
    assert!(
        ok,
        "the clean probe did not compile, so the scan below proves nothing:\n{stderr}"
    );

    let bad = config_diagnostics(&stderr);
    assert!(
        bad.is_empty(),
        "clippy.toml has {} entry/entries clippy cannot use, each silently disarmed:\n{}",
        bad.len(),
        bad.join("\n")
    );
}

/// Three disarm shapes — a mistyped crate root, an unknown crate, and a path
/// into a real non-std dependency — produce NO diagnostic at all, so
/// `every_denylist_path_resolves` cannot see them and would certify them.
/// Refuse them by root instead. An entry naming another crate is not wrong; it
/// is unverifiable by that probe, and adding one means extending the probe with
/// `--extern` first.
///
/// `allow-invalid` gets the same treatment: clippy suggests it by name when a
/// path fails to resolve, and setting it silences exactly the diagnostic
/// `every_denylist_path_resolves` reads.
#[test]
fn every_denylist_path_is_probeable() {
    #[derive(serde::Deserialize)]
    struct Config {
        #[serde(rename = "disallowed-methods")]
        entries: Vec<Entry>,
        // clippy.toml omits `disallowed-macros` deliberately and records why.
        // Adding that key, or `disallowed-types`, would otherwise pass unread
        // here and this test would certify half a file.
        #[serde(flatten)]
        rest: std::collections::BTreeMap<String, toml::Value>,
    }

    #[derive(serde::Deserialize)]
    struct Entry {
        path: String,
        // Absent in every entry today; clippy's default is false.
        #[serde(rename = "allow-invalid", default)]
        allow_invalid: bool,
        // Clippy rejects an unknown entry field outright, so anything landing
        // here is a field clippy grew and this test has not been taught — which
        // is exactly the blindness this test exists to close. `allow-invalid`
        // was itself once such a field.
        #[serde(flatten)]
        rest: std::collections::BTreeMap<String, toml::Value>,
    }

    let file = config().1;
    let text = std::fs::read_to_string(&file)
        .unwrap_or_else(|e| panic!("{} should be readable: {e}", file.display()));
    // `expect` would render the error with `Debug`, which embeds the whole file.
    let config: Config = toml::from_str(&text).unwrap_or_else(|e| {
        panic!(
            "{} does not match the shape this test audits: {e}",
            file.display()
        )
    });
    assert!(
        config.rest.is_empty(),
        "{} has keys this test does not read, so it audits only part of the \
         config: {:?}",
        file.display(),
        config.rest.keys().collect::<Vec<_>>()
    );

    let offenders: Vec<&str> = config
        .entries
        .iter()
        .filter(|entry| {
            entry.allow_invalid
                || !PROBEABLE_ROOTS.contains(&entry.path.split("::").next().unwrap_or_default())
                || entry
                    .rest
                    .keys()
                    .any(|k| !matches!(k.as_str(), "reason" | "replacement"))
        })
        .map(|entry| entry.path.as_str())
        .collect();

    assert!(
        offenders.is_empty(),
        "these entries are rooted outside {PROBEABLE_ROOTS:?}, set allow-invalid, or \
         carry a field this test does not know — the resolution check is blind to \
         them:\n{}",
        offenders.join("\n")
    );
}

/// `every_denylist_path_resolves` asserts an absence. If clippy stops reporting
/// bad paths, or reports them somewhere other than against the config file,
/// that absence becomes trivially true and the check goes green forever. This
/// plants a known-bad entry and asserts the report still appears.
#[test]
fn the_config_loader_still_reports_bad_paths() {
    let dir = scratch("canary-conf");
    std::fs::write(
        dir.join("clippy.toml"),
        "disallowed-methods = [\n  { path = \"std::fs::__oakum_canary\", reason = \"canary\" },\n]\n",
    )
    .expect("canary config should be writable");

    let (_, stderr) = run(&dir, &dir, "pub fn f() {}", &DENY_LINT);
    assert!(
        !config_diagnostics(&stderr).is_empty(),
        "clippy no longer reports an unusable denylist entry against the config \
         file; every_denylist_path_resolves is now blind:\n{stderr}"
    );
}
