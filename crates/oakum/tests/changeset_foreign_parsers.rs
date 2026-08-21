//! ADR-0005 Confirmation: every body oakum writes must be accepted by both
//! foreign parsers with the intended package names — not merely `Ok` / exit 0.
//!
//! knope's `changesets` crate retains quotes on keys and then matches nothing
//! (silent skip). `@changesets/parse` is the format gate behind `@changesets/cli`
//! (workspace membership is out of scope for this suite).
#![allow(clippy::disallowed_methods)]

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;
use std::sync::OnceLock;

use changesets::{Change, ChangeType};
use oakum::changeset::{write, KnopePresence};
use oakum::plan::BumpLevel;
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExpectKnope {
    /// Intended names; knope must not retain quotes.
    NamesMatch,
    /// Keys keep surrounding quotes (documented silent skip).
    SilentSkip,
}

struct WrittenBody {
    label: &'static str,
    body: String,
    expected: Vec<(&'static str, BumpLevel)>,
    note: &'static str,
    knope: ExpectKnope,
}

fn oakum_bodies() -> Vec<WrittenBody> {
    [
        (
            "unscoped_multi",
            &[("core", BumpLevel::Minor), ("utils", BumpLevel::Patch)][..],
            "\nNotes here.\n",
            KnopePresence::Absent,
            ExpectKnope::NamesMatch,
        ),
        (
            "empty_note",
            &[("core", BumpLevel::Patch)],
            "",
            KnopePresence::Absent,
            ExpectKnope::NamesMatch,
        ),
        (
            "patch",
            &[("core", BumpLevel::Patch)],
            "p\n",
            KnopePresence::Absent,
            ExpectKnope::NamesMatch,
        ),
        (
            "minor",
            &[("core", BumpLevel::Minor)],
            "m\n",
            KnopePresence::Absent,
            ExpectKnope::NamesMatch,
        ),
        (
            "major",
            &[("core", BumpLevel::Major)],
            "M\n",
            KnopePresence::Absent,
            ExpectKnope::NamesMatch,
        ),
        (
            "unscoped_with_knope_flag",
            &[("core", BumpLevel::Patch)],
            "n\n",
            KnopePresence::Present,
            ExpectKnope::NamesMatch,
        ),
        (
            "hyphenated_unscoped",
            &[("oakum-cli", BumpLevel::Patch)],
            "hyphen\n",
            KnopePresence::Absent,
            ExpectKnope::NamesMatch,
        ),
        (
            "scoped_quoted",
            &[("@oakum/core", BumpLevel::Minor)],
            "note\n",
            KnopePresence::Absent,
            ExpectKnope::SilentSkip,
        ),
        (
            "multi_scoped",
            &[
                ("@oakum/core", BumpLevel::Minor),
                ("@oakum/pkg-name", BumpLevel::Patch),
            ],
            "two-scoped\n",
            KnopePresence::Absent,
            ExpectKnope::SilentSkip,
        ),
        (
            "mixed_scoped_unscoped",
            &[
                ("@oakum/core", BumpLevel::Minor),
                ("utils", BumpLevel::Patch),
            ],
            "mix\n",
            KnopePresence::Absent,
            ExpectKnope::SilentSkip,
        ),
    ]
    .into_iter()
    .map(|(label, entries, note, knope_flag, knope)| {
        let mut seen = BTreeMap::new();
        for (name, level) in entries {
            assert!(
                seen.insert(*name, *level).is_none(),
                "{label}: duplicate package `{name}` in fixture"
            );
        }
        let body =
            write(entries, note, knope_flag).unwrap_or_else(|e| panic!("{label}: write: {e}"));
        WrittenBody {
            label,
            body,
            expected: entries.to_vec(),
            note,
            knope,
        }
    })
    .collect()
}

#[test]
fn oakum_writes_accepted_by_knope_changesets_crate() {
    for case in oakum_bodies() {
        let change = Change::from_file_name_and_content(&format!("{}.md", case.label), &case.body)
            .unwrap_or_else(|e| panic!("{}: knope parse: {e}", case.label));

        let got: BTreeMap<String, ChangeType> = change
            .versioning
            .iter()
            .map(|(name, ty)| (name.clone(), ty.clone()))
            .collect();

        match case.knope {
            ExpectKnope::NamesMatch => {
                assert_eq!(
                    got.len(),
                    case.expected.len(),
                    "{}: package count (got {got:?})",
                    case.label
                );
                for (name, level) in &case.expected {
                    let ty = got.get(*name).unwrap_or_else(|| {
                        panic!(
                            "{}: missing package `{name}` (got names {:?})",
                            case.label,
                            got.keys()
                        )
                    });
                    assert_eq!(
                        ty,
                        &bump_to_change_type(*level),
                        "{}: level for `{name}`",
                        case.label
                    );
                }
                assert_eq!(
                    change.summary,
                    knope_summary(case.note),
                    "{}: summary",
                    case.label
                );
            }
            ExpectKnope::SilentSkip => {
                let expected_keys: Vec<String> = case
                    .expected
                    .iter()
                    .map(|(name, _)| knope_retained_key(name))
                    .collect();
                let mut got_keys: Vec<String> = got.keys().cloned().collect();
                got_keys.sort();
                let mut want_keys = expected_keys;
                want_keys.sort();
                assert_eq!(
                    got_keys, want_keys,
                    "{}: knope silent-skip keys",
                    case.label
                );
                for (name, level) in &case.expected {
                    let key = knope_retained_key(name);
                    assert_eq!(
                        got.get(&key),
                        Some(&bump_to_change_type(*level)),
                        "{}: level under retained key `{key}`",
                        case.label
                    );
                    if key != *name {
                        assert!(
                            !got.contains_key(*name),
                            "{}: intended name `{name}` must not appear when knope retains quotes",
                            case.label
                        );
                    }
                }
                assert_eq!(
                    change.summary,
                    knope_summary(case.note),
                    "{}: summary",
                    case.label
                );
            }
        }
    }
}

#[test]
fn quoted_unscoped_key_is_silent_skip_under_knope() {
    // Oakum never writes quoted unscoped keys; retained quotes are the Confirmation detector.
    let body = "---\n\"core\": patch\n---\n";
    let change = Change::from_file_name_and_content("quoted.md", body).expect("knope Ok");
    assert_eq!(
        change.versioning,
        changesets::Versioning::from(("\"core\"", ChangeType::Patch))
    );
}

#[test]
fn quoted_unscoped_key_is_accepted_by_changesets_parse() {
    let runtime = js_runtime_dir();
    let body = "---\n\"core\": patch\n---\n";
    let parsed = parse_with_changesets_parse(&runtime, body).expect("@changesets/parse");
    assert_eq!(parsed.releases.len(), 1);
    assert_eq!(parsed.releases[0].name, "core");
    assert_eq!(parsed.releases[0].bump, BumpLevel::Patch);
}

#[test]
fn unquoted_scoped_key_is_rejected_by_changesets_parse() {
    let runtime = js_runtime_dir();
    let body = "---\n@oakum/core: minor\n---\n";
    let err = parse_with_changesets_parse(&runtime, body).expect_err("unquoted scoped YAML");
    assert!(
        err.to_lowercase().contains("reserved character @"),
        "expected YAML reserved-@ failure, got: {err}"
    );
}

#[test]
fn unquoted_scoped_key_is_accepted_by_knope() {
    let body = "---\n@oakum/core: minor\n---\n";
    let change = Change::from_file_name_and_content("unquoted.md", body).expect("knope Ok");
    assert_eq!(
        change.versioning,
        changesets::Versioning::from(("@oakum/core", ChangeType::Minor))
    );
}

#[test]
fn oakum_writes_accepted_by_changesets_parse() {
    let runtime = js_runtime_dir();

    for case in oakum_bodies() {
        let parsed = parse_with_changesets_parse(&runtime, &case.body)
            .unwrap_or_else(|e| panic!("{}: @changesets/parse: {e}", case.label));

        assert_eq!(
            parsed.releases.len(),
            case.expected.len(),
            "{}: release count",
            case.label
        );
        let mut by_name: BTreeMap<&str, BumpLevel> = BTreeMap::new();
        for release in &parsed.releases {
            assert!(
                by_name
                    .insert(release.name.as_str(), release.bump)
                    .is_none(),
                "{}: duplicate release name `{}`",
                case.label,
                release.name
            );
        }
        for (name, level) in &case.expected {
            let got = by_name.get(name).unwrap_or_else(|| {
                panic!(
                    "{}: missing `{name}` (got {:?})",
                    case.label,
                    by_name.keys()
                )
            });
            assert_eq!(got, level, "{}: type for `{name}`", case.label);
        }
        assert_eq!(
            parsed.summary,
            knope_summary(case.note),
            "{}: summary",
            case.label
        );
    }
}

fn bump_to_change_type(level: BumpLevel) -> ChangeType {
    match level {
        BumpLevel::Patch => ChangeType::Patch,
        BumpLevel::Minor => ChangeType::Minor,
        BumpLevel::Major => ChangeType::Major,
    }
}

/// Package key as knope's splitter sees it (scoped names keep surrounding quotes).
fn knope_retained_key(name: &str) -> String {
    if name.starts_with('@') {
        format!("\"{name}\"")
    } else {
        name.to_string()
    }
}

/// Mirror knope's summary trim; `@changesets/parse` matches it on these fixtures.
fn knope_summary(note: &str) -> String {
    note.lines()
        .skip_while(|line| line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn fixture_src() -> PathBuf {
    support::workspace_root().join("crates/oakum/tests/fixtures/changeset-foreign")
}

/// Install `@changesets/parse` under `CARGO_TARGET_TMPDIR`, not the checkout.
///
/// Serialized via [`OnceLock`] so parallel tests in *this* process do not race
/// `pnpm install`. Shared target dirs across processes are unsupported.
fn js_runtime_dir() -> PathBuf {
    static RUNTIME: OnceLock<PathBuf> = OnceLock::new();
    RUNTIME.get_or_init(prepare_js_runtime).clone()
}

fn prepare_js_runtime() -> PathBuf {
    let src = fixture_src();
    let dest = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("oakum-changeset-foreign");
    fs::create_dir_all(&dest).unwrap_or_else(|e| panic!("mkdir {}: {e}", dest.display()));
    for name in ["package.json", "pnpm-lock.yaml", "parse.mjs"] {
        fs::copy(src.join(name), dest.join(name))
            .unwrap_or_else(|e| panic!("copy {name} → {}: {e}", dest.display()));
    }
    let expected = expected_parse_version(&dest);
    let stamp = fixture_input_stamp(&dest);
    ensure_js_deps(&dest, &expected, &stamp);
    dest
}

fn fixture_input_stamp(runtime: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    fs::read(runtime.join("package.json"))
        .unwrap_or_else(|e| panic!("read package.json: {e}"))
        .hash(&mut hasher);
    fs::read(runtime.join("pnpm-lock.yaml"))
        .unwrap_or_else(|e| panic!("read pnpm-lock.yaml: {e}"))
        .hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn expected_parse_version(runtime: &Path) -> String {
    #[derive(Deserialize)]
    struct FixturePkg {
        dependencies: BTreeMap<String, String>,
    }
    let text = fs::read_to_string(runtime.join("package.json"))
        .unwrap_or_else(|e| panic!("read fixture package.json: {e}"));
    let pkg: FixturePkg =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("fixture package.json: {e}"));
    pkg.dependencies
        .get("@changesets/parse")
        .cloned()
        .unwrap_or_else(|| panic!("fixture package.json missing @changesets/parse"))
}

fn ensure_js_deps(runtime: &Path, expected_version: &str, stamp: &str) {
    require_node();
    let marker = runtime.join("node_modules/@changesets/parse/package.json");
    let entry = runtime.join("node_modules/@changesets/parse/dist/index.mjs");
    let stamp_path = runtime.join(".oakum-fixture-stamp");
    let stamp_ok = fs::read_to_string(&stamp_path).ok().as_deref() == Some(stamp);
    if stamp_ok && marker.is_file() && entry.is_file() {
        match read_parse_marker(&marker) {
            Ok(pkg) if pkg.name == "@changesets/parse" && pkg.version == expected_version => {
                return;
            }
            Ok(_) => {}
            Err(err) => panic!("read installed marker {}: {err}", marker.display()),
        }
    }
    let output = Command::new("pnpm")
        .args(["install", "--frozen-lockfile"])
        .current_dir(runtime)
        .output()
        .unwrap_or_else(|e| panic!("pnpm install in {}: {e}", runtime.display()));
    assert!(
        output.status.success(),
        "pnpm install failed in {}\nstderr: {}\nstdout: {}",
        runtime.display(),
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        marker.is_file(),
        "install did not produce {}",
        marker.display()
    );
    assert!(
        entry.is_file(),
        "install did not produce {}",
        entry.display()
    );
    let pkg = read_parse_marker(&marker)
        .unwrap_or_else(|e| panic!("read installed marker {}: {e}", marker.display()));
    assert_eq!(
        (pkg.name.as_str(), pkg.version.as_str()),
        ("@changesets/parse", expected_version),
        "installed package at {}",
        marker.display()
    );
    fs::write(&stamp_path, stamp)
        .unwrap_or_else(|e| panic!("write stamp {}: {e}", stamp_path.display()));
}

fn require_node() {
    let output = Command::new("node").arg("-v").output().unwrap_or_else(|e| {
        panic!("node not on PATH ({e}); pin `node` via mise (.mise.toml) for ADR-0005 Confirmation")
    });
    assert!(
        output.status.success(),
        "node -v failed\nstderr: {}\nstdout: {}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let version = String::from_utf8_lossy(&output.stdout);
    let version = version.trim().trim_start_matches('v');
    let mut parts = version.split('.');
    let major: u32 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("unparseable node version: {version}"));
    let minor: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let ok = (major == 22 && minor >= 11) || major == 24 || major >= 26;
    assert!(
        ok,
        "node {version} outside @changesets/parse engines (^22.11 || ^24 || >=26); mise pins 24.19.0"
    );
}

fn read_parse_marker(marker: &Path) -> Result<NpmPackage, String> {
    let text = fs::read_to_string(marker).map_err(|e| format!("read: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("json: {e}"))
}

#[derive(Debug, Deserialize)]
struct NpmPackage {
    name: String,
    version: String,
}

#[derive(Debug, Deserialize)]
struct JsParse {
    releases: Vec<JsRelease>,
    summary: String,
}

#[derive(Debug, Deserialize)]
struct JsRelease {
    name: String,
    #[serde(rename = "type", deserialize_with = "bump_level_from_str")]
    bump: BumpLevel,
}

fn bump_level_from_str<'de, D>(deserializer: D) -> Result<BumpLevel, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    BumpLevel::from_str(&raw).map_err(serde::de::Error::custom)
}

fn parse_with_changesets_parse(runtime: &Path, body: &str) -> Result<JsParse, String> {
    let script = runtime.join("parse.mjs");
    let mut child = Command::new("node")
        .arg(&script)
        .current_dir(runtime)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            format!("spawn node ({e}); pin `node` via mise (.mise.toml) for ADR-0005 Confirmation")
        })?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| String::from("piped stdin missing"))?;
        stdin
            .write_all(body.as_bytes())
            .map_err(|e| format!("write stdin: {e}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("wait node: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "exit {:?}\nstderr: {}\nstdout: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        ));
    }
    let parsed: JsParse = serde_json::from_slice(&output.stdout).map_err(|e| {
        format!(
            "json: {e}; stdout: {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })?;
    if parsed.releases.is_empty() {
        return Err(String::from(
            "@changesets/parse returned no releases (empty frontmatter is not a Confirmation accept)",
        ));
    }
    let mut names = std::collections::BTreeSet::new();
    for release in &parsed.releases {
        if release.name.is_empty() {
            return Err(String::from(
                "@changesets/parse returned an empty package name",
            ));
        }
        if !names.insert(release.name.as_str()) {
            return Err(format!(
                "@changesets/parse returned duplicate package `{}`",
                release.name
            ));
        }
    }
    Ok(parsed)
}
