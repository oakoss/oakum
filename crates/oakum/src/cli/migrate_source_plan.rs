//! Source-tool before-plan for `oakum migrate` (`okm-45t.1`).
//!
//! When knope / changesets / bumpy can supply a machine-readable plan, that is
//! the before fingerprint. When they cannot, the caller falls back to oakum
//! simulation and must exit unverified.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use oakum::changeset::resolve_package_name;
use oakum::detect::ReleaseTool;
use oakum::plan::{Ecosystem, PackageId, Workspace};
use semver::Version;
use serde::Deserialize;

#[derive(Debug)]
pub(super) enum SourceBeforePlan {
    Available {
        tool: ReleaseTool,
        fingerprint: BTreeMap<PackageId, (Version, Version)>,
        /// The child did not exit 0 and its output was read under a
        /// tool-specific convention rather than taken at face value. A
        /// difference measured against such a plan is not evidence the
        /// transform changed anything; only bumpy has such a convention.
        under_convention: bool,
    },
    /// Missing binary, failed run, or unusable output.
    Unavailable { tool: ReleaseTool, reason: String },
}

#[must_use]
pub(super) fn primary_plan_tool(knope: bool, bumpy: bool, changesets: bool) -> Option<ReleaseTool> {
    if knope {
        Some(ReleaseTool::Knope)
    } else if bumpy {
        Some(ReleaseTool::Bumpy)
    } else if changesets {
        Some(ReleaseTool::Changesets)
    } else {
        None
    }
}

pub(super) fn fetch_source_before_plan(
    tool: ReleaseTool,
    cwd: &Path,
    workspace: &Workspace,
) -> SourceBeforePlan {
    match tool {
        ReleaseTool::Bumpy => bumpy_status_json(cwd, workspace),
        ReleaseTool::Changesets => changesets_status_output(cwd, workspace),
        ReleaseTool::Knope => knope_release_dry_run(cwd, workspace),
        other => SourceBeforePlan::Unavailable {
            tool: other,
            reason: format!("{} has no migrate before-plan command", other.name()),
        },
    }
}

fn bumpy_status_json(cwd: &Path, workspace: &Workspace) -> SourceBeforePlan {
    let Some(bin) = resolve_tool_bin("bumpy", cwd) else {
        return SourceBeforePlan::Unavailable {
            tool: ReleaseTool::Bumpy,
            reason: not_found("bumpy"),
        };
    };
    let output = match Command::new(&bin)
        .args(["status", "--json"])
        .current_dir(cwd)
        .output()
    {
        Ok(output) => output,
        Err(err) => {
            return SourceBeforePlan::Unavailable {
                tool: ReleaseTool::Bumpy,
                reason: format!("failed to run `bumpy status --json`: {err}"),
            };
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    match parse_bumpy_status_json(&stdout, workspace) {
        Ok(fingerprint) => {
            // Exit 1 with nothing pending and nothing said is bumpy's "no
            // releases"; still a usable plan. Anything else that failed is a
            // tool that did not answer, and reading its empty output as an
            // authoritative empty plan would let a crashed bumpy certify the
            // migration — the changesets and knope arms already refuse it.
            let quiet_nothing_pending = output.status.code() == Some(1)
                && fingerprint.is_empty()
                && output.stderr.is_empty();
            // Taking the carve-out is worth a line: a crash that happens to
            // print parseable JSON with no releases is indistinguishable from
            // the convention, so a later divergence must be attributable to a
            // before-plan that came from a child which did not exit 0.
            if quiet_nothing_pending {
                println!(
                    "plan comparison: `bumpy status --json` exited 1 with no releases, read as nothing pending"
                );
            }
            if !output.status.success() && !quiet_nothing_pending {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                return SourceBeforePlan::Unavailable {
                    tool: ReleaseTool::Bumpy,
                    reason: format!(
                        "`bumpy status --json` {}{}",
                        how_it_ended(output.status),
                        if stderr.is_empty() {
                            String::new()
                        } else {
                            format!(": {stderr}")
                        }
                    ),
                };
            }
            SourceBeforePlan::Available {
                tool: ReleaseTool::Bumpy,
                fingerprint,
                under_convention: quiet_nothing_pending,
            }
        }
        Err(reason) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            // How the child ended leads when it did not exit cleanly: a tool the
            // kernel killed writes nothing, and blaming the JSON for being empty
            // sends the reader after a parser rather than an OOM.
            let reason = if output.status.success() {
                reason
            } else {
                format!("{} ({reason})", how_it_ended(output.status))
            };
            let detail = if stderr.is_empty() {
                reason
            } else {
                format!("{reason}; stderr: {stderr}")
            };
            SourceBeforePlan::Unavailable {
                tool: ReleaseTool::Bumpy,
                reason: detail,
            }
        }
    }
}

/// `releases` is required. With `#[serde(default)]` any JSON object at all
/// deserializes to zero releases, so a crashing bumpy's error envelope —
/// measured: `{"error":"database is locked","code":"EBUSY"}` at exit 1 — read
/// as an empty plan and was certified as the before-plan. The spec licenses
/// "exit 1 with no releases", which is not the same as "not a status document".
#[derive(Debug, Deserialize)]
struct BumpyStatusJson {
    releases: Vec<NamedRelease>,
}

#[derive(Debug, Deserialize)]
struct NamedRelease {
    name: String,
    #[serde(rename = "oldVersion")]
    old_version: String,
    #[serde(rename = "newVersion")]
    new_version: String,
    #[serde(rename = "type", default)]
    release_type: Option<String>,
}

fn parse_bumpy_status_json(
    json: &str,
    workspace: &Workspace,
) -> Result<BTreeMap<PackageId, (Version, Version)>, String> {
    let parsed: BumpyStatusJson =
        serde_json::from_str(json).map_err(|err| format!("bumpy JSON: {err}"))?;
    releases_to_fingerprint(
        parsed
            .releases
            .into_iter()
            .filter(|r| r.release_type.as_deref() != Some("none"))
            .map(|r| (r.name, r.old_version, r.new_version)),
        workspace,
        "bumpy",
    )
}

fn changesets_status_output(cwd: &Path, workspace: &Workspace) -> SourceBeforePlan {
    let Some(bin) = resolve_tool_bin("changeset", cwd) else {
        return SourceBeforePlan::Unavailable {
            tool: ReleaseTool::Changesets,
            reason: not_found("changeset"),
        };
    };
    let out_path = match exclusive_temp_json() {
        Ok(path) => path,
        Err(reason) => {
            return SourceBeforePlan::Unavailable {
                tool: ReleaseTool::Changesets,
                reason,
            };
        }
    };
    let output = match Command::new(&bin)
        .args(["status", "--output"])
        .arg(&out_path)
        .current_dir(cwd)
        .output()
    {
        Ok(output) => output,
        Err(err) => {
            discard_temp(&out_path);
            return SourceBeforePlan::Unavailable {
                tool: ReleaseTool::Changesets,
                reason: format!("failed to run `changeset status --output`: {err}"),
            };
        }
    };
    if !output.status.success() {
        discard_temp(&out_path);
        // Non-success is not a verified plan, even when the file is parseable
        // (unlike bumpy's exit-1-with-empty-releases case).
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return SourceBeforePlan::Unavailable {
            tool: ReleaseTool::Changesets,
            reason: format!(
                "`changeset status` {}{}",
                how_it_ended(output.status),
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {stderr}")
                }
            ),
        };
    }
    let body = match fs::read_to_string(&out_path) {
        Ok(body) => {
            discard_temp(&out_path);
            body
        }
        Err(err) => {
            discard_temp(&out_path);
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            return SourceBeforePlan::Unavailable {
                tool: ReleaseTool::Changesets,
                reason: format!(
                    "changeset did not write `--output` file ({err}){}",
                    if stderr.is_empty() {
                        String::new()
                    } else {
                        format!("; stderr: {stderr}")
                    }
                ),
            };
        }
    };
    match parse_changesets_status_json(&body, workspace) {
        Ok(fingerprint) => SourceBeforePlan::Available {
            under_convention: false,
            tool: ReleaseTool::Changesets,
            fingerprint,
        },
        Err(reason) => SourceBeforePlan::Unavailable {
            tool: ReleaseTool::Changesets,
            reason,
        },
    }
}

#[derive(Debug, Deserialize)]
struct ChangesetsStatusJson {
    #[serde(default)]
    releases: Vec<ChangesetsRelease>,
}

#[derive(Debug, Deserialize)]
struct ChangesetsRelease {
    name: String,
    #[serde(rename = "type", default)]
    release_type: String,
    #[serde(rename = "oldVersion")]
    old_version: Option<String>,
    #[serde(rename = "newVersion")]
    new_version: Option<String>,
}

fn parse_changesets_status_json(
    json: &str,
    workspace: &Workspace,
) -> Result<BTreeMap<PackageId, (Version, Version)>, String> {
    let parsed: ChangesetsStatusJson =
        serde_json::from_str(json).map_err(|err| format!("changesets JSON: {err}"))?;
    let triples = parsed
        .releases
        .into_iter()
        .filter(|r| r.release_type != "none")
        .filter_map(|r| {
            let old = r.old_version?;
            let new = r.new_version?;
            Some((r.name, old, new))
        });
    releases_to_fingerprint(triples, workspace, "changesets")
}

fn knope_release_dry_run(cwd: &Path, workspace: &Workspace) -> SourceBeforePlan {
    let Some(bin) = resolve_tool_bin("knope", cwd) else {
        return SourceBeforePlan::Unavailable {
            tool: ReleaseTool::Knope,
            reason: not_found("knope"),
        };
    };
    let workflow = match knope_workflow_name(cwd) {
        Ok(workflow) => workflow,
        Err(reason) => {
            return SourceBeforePlan::Unavailable {
                tool: ReleaseTool::Knope,
                reason,
            };
        }
    };
    let output = match Command::new(&bin)
        .args([&workflow, "--dry-run"])
        .current_dir(cwd)
        .output()
    {
        Ok(output) => output,
        Err(err) => {
            return SourceBeforePlan::Unavailable {
                tool: ReleaseTool::Knope,
                reason: format!("failed to run `knope {workflow} --dry-run`: {err}"),
            };
        }
    };
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    match parse_knope_dry_run(&combined, workspace) {
        Ok(fingerprint) if !fingerprint.is_empty() && output.status.success() => {
            SourceBeforePlan::Available {
                under_convention: false,
                tool: ReleaseTool::Knope,
                fingerprint,
            }
        }
        Ok(fingerprint) if !fingerprint.is_empty() => SourceBeforePlan::Unavailable {
            tool: ReleaseTool::Knope,
            reason: format!(
                "`knope {workflow} --dry-run` {} with version lines (not treated as verified evidence)",
                how_it_ended(output.status)
            ),
        },
        Ok(_) => SourceBeforePlan::Unavailable {
            tool: ReleaseTool::Knope,
            reason: format!(
                "`knope {workflow} --dry-run` produced no usable version lines ({})",
                how_it_ended(output.status)
            ),
        },
        Err(reason) => SourceBeforePlan::Unavailable {
            tool: ReleaseTool::Knope,
            reason,
        },
    }
}

/// The workflow whose dry run states the before-plan, or why oakum cannot say.
fn knope_workflow_name(cwd: &Path) -> Result<String, String> {
    let body = match fs::read_to_string(cwd.join("knope.toml")) {
        Ok(body) => body,
        // No file declares no workflow, so `release` is the answer, not a guess.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        // Anything else and oakum does not know which workflow states the
        // before-plan. Running `release` anyway would make a guess's output the
        // evidence behind a definitive "the release plan changed".
        Err(err) => return Err(format!("could not read `knope.toml`: {err}")),
    };
    if body.contains("prepare-release") {
        return Ok(String::from("prepare-release"));
    }
    Ok(String::from("release"))
}

/// Parse knope dry-run lines like `Would add the following to package.json: 1.0.1`,
/// `Would add the following to Cargo.toml: version = 1.1.0` (knope ≥0.23),
/// or `Would add the following to crates/core/Cargo.toml: 0.1.1`.
fn parse_knope_dry_run(
    text: &str,
    workspace: &Workspace,
) -> Result<BTreeMap<PackageId, (Version, Version)>, String> {
    let mut fingerprint = BTreeMap::new();
    let prefix = "Would add the following to ";
    for line in text.lines() {
        let Some(rest) = line.strip_prefix(prefix) else {
            continue;
        };
        let Some((path, version_text)) = rest.rsplit_once(':') else {
            continue;
        };
        let path = path.trim();
        let version_text = version_text.trim();
        if !path.ends_with("package.json") && !path.ends_with("Cargo.toml") {
            continue;
        }
        let to = parse_knope_version_payload(version_text)
            .map_err(|err| format!("knope dry-run version `{version_text}`: {err}"))?;
        let id = package_id_for_manifest_path(path, workspace).ok_or_else(|| {
            format!("knope dry-run named `{path}` which matches no workspace package")
        })?;
        let from = workspace
            .get(&id)
            .map(|pkg| pkg.version().clone())
            .ok_or_else(|| format!("workspace missing package for knope dry-run path `{path}`"))?;
        fingerprint.insert(id, (from, to));
    }
    Ok(fingerprint)
}

/// Accept bare `1.0.1` (documented tutorials) and `version = 1.1.0` / `version = "1.1.0"` (knope 0.23+).
fn parse_knope_version_payload(payload: &str) -> Result<Version, String> {
    let trimmed = payload.trim();
    let after_version = match trimmed.strip_prefix("version") {
        Some(rest) => {
            let rest = rest.trim_start();
            match rest.strip_prefix('=') {
                Some(value) => value.trim(),
                None => rest,
            }
            .trim_matches(|c| c == '"' || c == '\'')
        }
        None => trimmed,
    };
    // Extra assignments after the version (npm workspace dry-run noise): take the first token.
    let first = after_version
        .split_whitespace()
        .next()
        .unwrap_or(after_version)
        .trim_matches(|c| c == ',' || c == ';');
    Version::parse(first).map_err(|err| err.to_string())
}

fn package_id_for_manifest_path(path: &str, workspace: &Workspace) -> Option<PackageId> {
    let normalized = path.replace('\\', "/");
    let mut matches = Vec::new();
    for pkg in workspace.packages() {
        let file = match pkg.id().ecosystem {
            Ecosystem::Cargo => "Cargo.toml",
            Ecosystem::Npm => "package.json",
        };
        let expected = if pkg.manifest_dir().is_empty() {
            file.to_string()
        } else {
            format!("{}/{file}", pkg.manifest_dir().replace('\\', "/"))
        };
        if paths_equal_or_suffix(&normalized, &expected) {
            matches.push(pkg.id().clone());
        }
    }
    match matches.as_slice() {
        [only] => Some(only.clone()),
        _ => None,
    }
}

/// Exact match, or `…/<expected>` when `expected` is not a bare filename.
fn paths_equal_or_suffix(got: &str, expected: &str) -> bool {
    if got == expected {
        return true;
    }
    // Bare `Cargo.toml` / `package.json` only matches the repository-root package.
    if !expected.contains('/') {
        return false;
    }
    got.ends_with(&format!("/{expected}"))
}

fn releases_to_fingerprint(
    releases: impl IntoIterator<Item = (String, String, String)>,
    workspace: &Workspace,
    tool: &str,
) -> Result<BTreeMap<PackageId, (Version, Version)>, String> {
    let mut fingerprint = BTreeMap::new();
    for (name, old, new) in releases {
        let id = match resolve_package_name(&name, workspace) {
            Ok(id) => id,
            Err(oakum::changeset::UnknownReason::Missing) => {
                return Err(format!(
                    "{tool} release `{name}` matches no workspace package"
                ));
            }
            Err(oakum::changeset::UnknownReason::Ambiguous) => {
                return Err(format!(
                    "{tool} release `{name}` matches more than one workspace package"
                ));
            }
        };
        let from = Version::parse(&old)
            .map_err(|err| format!("{tool} oldVersion `{old}` for `{name}`: {err}"))?;
        let to = Version::parse(&new)
            .map_err(|err| format!("{tool} newVersion `{new}` for `{name}`: {err}"))?;
        fingerprint.insert(id, (from, to));
    }
    Ok(fingerprint)
}

/// How a child ended, for a reason line. `code()` is `None` exactly when a
/// signal killed it, and `-1` is not a status any shell reports — a reader
/// debugging an OOM kill would go looking for the wrong thing.
fn how_it_ended(status: std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exited {code}"),
        None => String::from("was terminated by a signal"),
    }
}

/// The temp file is created with `create_new` and named by pid and nanos, so a
/// failed unlink leaves one behind per run rather than colliding. Naming it is
/// the difference between a reader cleaning up and never knowing.
fn discard_temp(path: &Path) {
    if let Err(err) = fs::remove_file(path) {
        if err.kind() != std::io::ErrorKind::NotFound {
            eprintln!("could not remove `{}`: {err}", path.display());
        }
    }
}

/// The source tool's binary: this repository's own `node_modules/.bin` first,
/// then `PATH`.
///
/// The local directory is not a nicety. `npm i -g` and Homebrew put `oakum` on
/// `PATH` and a devDependency's binaries nowhere near it, so the install the
/// docs recommend leaves the source tool unrunnable and the parity check
/// unverified — on the one command whose whole job is comparing against that
/// tool (`okm-404.3`). Only `pnpm exec`, which prepends the directory, made the
/// verified path reachable.
///
/// Cargo and mise need no arm here: `cargo install` writes to `~/.cargo/bin`
/// and mise installs behind shims, and both are on `PATH` or the tool does not
/// run at all.
fn resolve_tool_bin(name: &str, cwd: &Path) -> Option<PathBuf> {
    let bin_dir = cwd.join("node_modules").join(".bin");
    for candidate in local_bin_names(name) {
        let local = bin_dir.join(candidate);
        if local.is_file() {
            return Some(local);
        }
    }
    resolve_command(name)
}

/// On Windows npm writes an extensionless shell script beside the `.cmd` shim,
/// and only the shim is executable: `CreateProcess` refuses a file with no
/// recognized extension. Listing the bare name there would find the script
/// first and fail the spawn, so Windows gets the shims alone.
///
/// Inferred, not measured — this host is not Windows.
fn local_bin_names(name: &str) -> Vec<String> {
    if cfg!(windows) {
        return Vec::from([format!("{name}.cmd"), format!("{name}.CMD")]);
    }
    Vec::from([name.to_owned()])
}

/// Where [`resolve_tool_bin`] looked, for a reason line that a reader can act
/// on: the two places differ in what installing differently would fix.
fn not_found(name: &str) -> String {
    format!("`{name}` not found (no local `node_modules/.bin/{name}`, none on PATH)")
}

fn resolve_command(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| {
        if cfg!(windows) {
            String::from(".COM;.EXE;.BAT;.CMD")
        } else {
            String::new()
        }
    });
    resolve_on_path(name, &path, &pathext)
}

fn resolve_on_path(name: &str, path: &OsStr, pathext: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains(['/', '\\']) {
        return None;
    }
    let exts: Vec<&str> = if cfg!(windows) {
        pathext.split(';').filter(|s| !s.is_empty()).collect()
    } else {
        vec![""]
    };
    for dir in std::env::split_paths(path) {
        for ext in &exts {
            let candidate = if ext.is_empty() {
                dir.join(name)
            } else {
                dir.join(format!("{name}{ext}"))
            };
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn exclusive_temp_json() -> Result<PathBuf, String> {
    use std::fs::OpenOptions;
    use std::time::{SystemTime, UNIX_EPOCH};

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let path = std::env::temp_dir().join(format!(
        "oakum-migrate-changeset-status-{}-{nanos}.json",
        std::process::id()
    ));
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|err| {
            format!(
                "failed to create exclusive changeset status file {}: {err}",
                path.display()
            )
        })?;
    Ok(path)
}

#[cfg(test)]
mod local_shims {
    use super::local_bin_names;

    /// The Windows arm cannot be exercised on a unix host, so each platform
    /// pins its own list and the Windows CI job measures the branch this one
    /// cannot. `Command` there can launch the `.cmd` shim and not the
    /// extensionless script npm writes beside it, so the bare name must be
    /// absent rather than merely later in the list.
    #[test]
    fn each_platform_looks_for_the_names_it_can_execute() {
        let names = local_bin_names("changeset");
        #[cfg(windows)]
        assert_eq!(names, ["changeset.cmd", "changeset.CMD"]);
        #[cfg(not(windows))]
        assert_eq!(names, ["changeset"]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oakum::plan::{Package, ResolvesDependenciesAt};

    fn cargo_ws(name: &str, version: &str) -> Workspace {
        let pkg = Package::new(
            PackageId::new(Ecosystem::Cargo, name),
            Version::parse(version).unwrap(),
            ResolvesDependenciesAt::Install,
            true,
            vec![],
        );
        Workspace::new(vec![pkg]).unwrap()
    }

    #[test]
    fn bumpy_json_parses_releases() {
        let ws = cargo_ws("core", "0.1.0");
        let json = r#"{
          "releases": [
            {"name": "core", "type": "minor", "oldVersion": "0.1.0", "newVersion": "0.2.0"}
          ],
          "packageNames": ["core"]
        }"#;
        let fp = parse_bumpy_status_json(json, &ws).unwrap();
        let id = PackageId::new(Ecosystem::Cargo, "core");
        assert_eq!(
            fp.get(&id),
            Some(&(Version::new(0, 1, 0), Version::new(0, 2, 0)))
        );
    }

    #[test]
    fn changesets_json_skips_none() {
        let ws = cargo_ws("core", "1.0.0");
        let json = r#"{
          "releases": [
            {"name": "other", "type": "none"},
            {"name": "core", "type": "patch", "oldVersion": "1.0.0", "newVersion": "1.0.1"}
          ]
        }"#;
        let fp = parse_changesets_status_json(json, &ws).unwrap();
        assert_eq!(fp.len(), 1);
    }

    #[test]
    fn knope_dry_run_root_manifest() {
        let ws = cargo_ws("core", "0.1.0");
        let text = "Would add the following to Cargo.toml: 0.1.1\nWould delete: .changeset/x.md\n";
        let fp = parse_knope_dry_run(text, &ws).unwrap();
        let id = PackageId::new(Ecosystem::Cargo, "core");
        assert_eq!(
            fp.get(&id),
            Some(&(Version::new(0, 1, 0), Version::new(0, 1, 1)))
        );
    }

    #[test]
    fn knope_dry_run_accepts_version_assignment() {
        let ws = cargo_ws("core", "0.1.0");
        let text = "Would add the following to Cargo.toml: version = 0.1.1\n";
        let fp = parse_knope_dry_run(text, &ws).unwrap();
        let id = PackageId::new(Ecosystem::Cargo, "core");
        assert_eq!(
            fp.get(&id),
            Some(&(Version::new(0, 1, 0), Version::new(0, 1, 1)))
        );
    }

    #[test]
    fn knope_path_does_not_suffix_match_sibling_names() {
        let core = Package::new(
            PackageId::new(Ecosystem::Cargo, "core"),
            Version::new(0, 1, 0),
            ResolvesDependenciesAt::Install,
            true,
            vec![],
        )
        .with_manifest_dir("core");
        let notcore = Package::new(
            PackageId::new(Ecosystem::Cargo, "notcore"),
            Version::new(0, 2, 0),
            ResolvesDependenciesAt::Install,
            true,
            vec![],
        )
        .with_manifest_dir("notcore");
        let ws = Workspace::new(vec![core, notcore]).unwrap();
        let text = "Would add the following to notcore/Cargo.toml: 0.2.1\n";
        let fp = parse_knope_dry_run(text, &ws).unwrap();
        assert_eq!(fp.len(), 1);
        assert!(fp.contains_key(&PackageId::new(Ecosystem::Cargo, "notcore")));
        assert!(!fp.contains_key(&PackageId::new(Ecosystem::Cargo, "core")));
    }

    #[test]
    fn knope_bare_manifest_rejects_when_no_root_package() {
        let left = Package::new(
            PackageId::new(Ecosystem::Cargo, "app"),
            Version::new(0, 1, 0),
            ResolvesDependenciesAt::Install,
            true,
            vec![],
        )
        .with_manifest_dir("app");
        let right = Package::new(
            PackageId::new(Ecosystem::Cargo, "aaa"),
            Version::new(0, 1, 0),
            ResolvesDependenciesAt::Install,
            true,
            vec![],
        )
        .with_manifest_dir("aaa");
        let ws = Workspace::new(vec![left, right]).unwrap();
        let text = "Would add the following to Cargo.toml: 0.1.1\n";
        let err = parse_knope_dry_run(text, &ws).unwrap_err();
        assert!(err.contains("Cargo.toml"), "{err}");
    }

    #[test]
    fn primary_prefers_knope() {
        assert_eq!(
            primary_plan_tool(true, true, true),
            Some(ReleaseTool::Knope)
        );
        assert_eq!(
            primary_plan_tool(false, true, true),
            Some(ReleaseTool::Bumpy)
        );
        assert_eq!(
            primary_plan_tool(false, false, true),
            Some(ReleaseTool::Changesets)
        );
        assert_eq!(primary_plan_tool(false, false, false), None);
    }
}
