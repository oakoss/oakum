//! Shared `@changesets/parse` harness for ADR-0005 Confirmation tests.
//!
//! Installs the fixture under `CARGO_TARGET_TMPDIR` and shells to `parse.mjs`.
//! Node engines come from the fixture lockfile for `@changesets/parse`; the
//! installed package.json must match that range (not a duplicated mise pin).

use std::collections::BTreeMap;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;
use std::sync::OnceLock;

use oakum::plan::BumpLevel;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct JsParse {
    pub releases: Vec<JsRelease>,
    pub summary: String,
}

#[derive(Debug, Deserialize)]
pub struct JsRelease {
    pub name: String,
    #[serde(rename = "type", deserialize_with = "bump_level_from_str")]
    pub bump: BumpLevel,
}

fn bump_level_from_str<'de, D>(deserializer: D) -> Result<BumpLevel, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    BumpLevel::from_str(&raw).map_err(serde::de::Error::custom)
}

/// Install `@changesets/parse` under `CARGO_TARGET_TMPDIR`, not the checkout.
///
/// Serialized via [`OnceLock`] so parallel tests in *this* process do not race
/// `pnpm install`. Shared target dirs across processes are unsupported.
pub fn js_runtime_dir() -> PathBuf {
    static RUNTIME: OnceLock<PathBuf> = OnceLock::new();
    RUNTIME.get_or_init(prepare_js_runtime).clone()
}

fn fixture_src() -> PathBuf {
    super::workspace_root().join("crates/oakum/tests/fixtures/changeset-foreign")
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
    // Lockfile is the source of truth for require_node and drift checks.
    // Installed package.json alone cannot detect lockfile↔package drift and
    // would skip a broken lock on the warm path.
    let lockfile_engines = engines_from_lockfile(runtime, expected_version);
    require_node(&lockfile_engines);
    let marker = runtime.join("node_modules/@changesets/parse/package.json");
    let entry = runtime.join("node_modules/@changesets/parse/dist/index.mjs");
    let stamp_path = runtime.join(".oakum-fixture-stamp");
    let stamp_ok = fs::read_to_string(&stamp_path).ok().as_deref() == Some(stamp);
    if stamp_ok && marker.is_file() && entry.is_file() {
        match read_parse_package(&marker) {
            Ok(pkg) if pkg.name == "@changesets/parse" && pkg.version == expected_version => {
                assert_eq!(
                    pkg.engines.node, lockfile_engines,
                    "installed engines.node drifted from the lockfile range"
                );
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
    let pkg = read_parse_package(&marker)
        .unwrap_or_else(|e| panic!("read installed marker {}: {e}", marker.display()));
    assert_eq!(
        (pkg.name.as_str(), pkg.version.as_str()),
        ("@changesets/parse", expected_version),
        "installed package at {}",
        marker.display()
    );
    assert_eq!(
        pkg.engines.node, lockfile_engines,
        "installed @changesets/parse engines.node must match the lockfile range"
    );
    fs::write(&stamp_path, stamp)
        .unwrap_or_else(|e| panic!("write stamp {}: {e}", stamp_path.display()));
}

fn engines_from_lockfile(runtime: &Path, expected_version: &str) -> String {
    let lock = fs::read_to_string(runtime.join("pnpm-lock.yaml"))
        .unwrap_or_else(|e| panic!("read pnpm-lock.yaml: {e}"));
    let header = format!("  '@changesets/parse@{expected_version}':");
    let mut lines = lock.lines();
    while let Some(line) = lines.next() {
        if line != header {
            continue;
        }
        for following in lines.by_ref() {
            let trimmed = following.trim_start();
            if let Some(rest) = trimmed.strip_prefix("engines:") {
                return parse_lockfile_node_engines(rest.trim(), expected_version);
            }
            // Next package entry; engines should already have appeared.
            if following.starts_with("  '") || following.starts_with("  \"") {
                break;
            }
        }
        break;
    }
    panic!("pnpm-lock.yaml missing engines.node for @changesets/parse@{expected_version}");
}

fn parse_lockfile_node_engines(inline: &str, expected_version: &str) -> String {
    // pnpm writes `engines: {node: ^22.11 || ^24 || >=26}` on one line.
    // Split on commas inside the map so a key like `my_node:` cannot match.
    let inner = inline
        .trim()
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .unwrap_or_else(|| {
            panic!("lockfile engines for @changesets/parse@{expected_version} not a map: {inline}")
        });
    for part in inner.split(',') {
        let part = part.trim();
        let Some(rest) = part.strip_prefix("node:") else {
            continue;
        };
        let spec = rest.trim().trim_matches('\'').trim_matches('"');
        assert!(
            !spec.is_empty(),
            "lockfile engines.node empty for @changesets/parse@{expected_version}"
        );
        return spec.to_string();
    }
    panic!("lockfile engines for @changesets/parse@{expected_version} has no node key: {inline}");
}

fn require_node(engines: &str) {
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
    assert!(
        node_satisfies_engines(version, engines),
        "node {version} outside @changesets/parse engines ({engines}); see `.mise.toml` `[tools].node`"
    );
}

/// Supports the npm `engines.node` forms `@changesets/parse` publishes:
/// `^X`, `^X.Y`, `^X.Y.Z`, and `>=X` alternatives joined by `||`.
fn node_satisfies_engines(version: &str, engines: &str) -> bool {
    let Some((major, minor, patch)) = parse_node_version(version) else {
        return false;
    };
    engines.split("||").map(str::trim).any(|alt| {
        if let Some(rest) = alt.strip_prefix(">=") {
            let Some((req_major, req_minor, req_patch)) = parse_node_version(rest.trim()) else {
                return false;
            };
            (major, minor, patch) >= (req_major, req_minor, req_patch)
        } else if let Some(rest) = alt.strip_prefix('^') {
            let Some((req_major, req_minor, req_patch)) = parse_node_version(rest.trim()) else {
                return false;
            };
            major == req_major && (minor, patch) >= (req_minor, req_patch)
        } else {
            false
        }
    })
}

fn parse_node_version(version: &str) -> Option<(u32, u32, u32)> {
    let mut parts = version.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map_or(Some(0), |s| s.parse().ok())?;
    let patch = parts.next().map_or(Some(0), |s| s.parse().ok())?;
    Some((major, minor, patch))
}

fn read_parse_package(marker: &Path) -> Result<ParsePackage, String> {
    let text = fs::read_to_string(marker).map_err(|e| format!("read: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("json: {e}"))
}

#[derive(Debug, Deserialize)]
struct ParsePackage {
    name: String,
    version: String,
    engines: ParseEngines,
}

#[derive(Debug, Deserialize)]
struct ParseEngines {
    node: String,
}

/// Require at least one named release (empty frontmatter is not an intersection accept).
pub fn parse_with_changesets_parse(runtime: &Path, body: &str) -> Result<JsParse, String> {
    let parsed = parse_js_raw(runtime, body)?;
    if parsed.releases.is_empty() {
        return Err(String::from(
            "@changesets/parse returned no releases (empty frontmatter is not an intersection Confirmation accept)",
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

/// No release-count gate; empty frontmatter is allowed.
pub fn parse_js_raw(runtime: &Path, body: &str) -> Result<JsParse, String> {
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
    serde_json::from_slice(&output.stdout).map_err(|e| {
        format!(
            "json: {e}; stdout: {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

#[test]
fn node_satisfies_engines_boundaries() {
    let engines = "^22.11 || ^24 || >=26";
    assert!(node_satisfies_engines("22.11.0", engines));
    assert!(node_satisfies_engines("22.20.0", engines));
    assert!(!node_satisfies_engines("22.10.9", engines));
    assert!(!node_satisfies_engines("23.0.0", engines));
    assert!(node_satisfies_engines("24.0.0", engines));
    assert!(node_satisfies_engines("24.20.0", engines));
    assert!(!node_satisfies_engines("25.0.0", engines));
    assert!(node_satisfies_engines("26.0.0", engines));
    assert!(node_satisfies_engines("27.1.0", engines));
    assert!(!node_satisfies_engines("not-a-version", engines));
    assert!(!node_satisfies_engines("24.0.0", "bogus"));
}

#[test]
fn parse_lockfile_node_engines_reads_real_node_key() {
    assert_eq!(
        parse_lockfile_node_engines("{node: ^22.11 || ^24 || >=26}", "1.0.0"),
        "^22.11 || ^24 || >=26"
    );
    assert_eq!(
        parse_lockfile_node_engines("{npm: >=8, node: ^24}", "1.0.0"),
        "^24"
    );
}

#[test]
#[should_panic(expected = "has no node key")]
fn parse_lockfile_node_engines_rejects_my_node_suffix() {
    let _ = parse_lockfile_node_engines("{my_node: ^22.11 || ^24 || >=26}", "1.0.0");
}

#[test]
#[should_panic(expected = "empty")]
fn parse_lockfile_node_engines_rejects_empty_node() {
    let _ = parse_lockfile_node_engines("{node: }", "1.0.0");
}

#[test]
fn engines_from_lockfile_reads_fixture_shape() {
    let runtime = tempfile_runtime_with_lock(
        "\
lockfileVersion: '9.0'
packages:
  '@changesets/parse@1.0.0':
    resolution: {integrity: sha512-deadbeef}
    engines: {node: ^22.11 || ^24 || >=26}
",
    );
    assert_eq!(
        engines_from_lockfile(runtime.path(), "1.0.0"),
        "^22.11 || ^24 || >=26"
    );
}

#[test]
#[should_panic(expected = "missing engines.node")]
fn engines_from_lockfile_panics_without_engines() {
    let runtime = tempfile_runtime_with_lock(
        "\
packages:
  '@changesets/parse@1.0.0':
    resolution: {integrity: sha512-deadbeef}
",
    );
    let _ = engines_from_lockfile(runtime.path(), "1.0.0");
}

/// Scratch lockfile dir for engines unit tests. Not `oakum-*`: the fixture leak
/// check fails unmarked dirs with that prefix, and these are not harness fixtures.
struct LockfileScratch {
    dir: PathBuf,
}

impl LockfileScratch {
    fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for LockfileScratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn tempfile_runtime_with_lock(lock: &str) -> LockfileScratch {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "changeset-foreign-engines-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("mkdir {}: {e}", dir.display()));
    fs::write(dir.join("pnpm-lock.yaml"), lock).unwrap_or_else(|e| panic!("write lock: {e}"));
    LockfileScratch { dir }
}
