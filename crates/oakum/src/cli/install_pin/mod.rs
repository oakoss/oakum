//! Read-only verification of install pins (ADR-0007).
//!
//! Oakum does not write workflow files (ADR-0003). `check` scans
//! `.github/workflows`, `.github/actions/*/action.yml` (or `.yaml`), the
//! root `package.json`, `.mise.toml` / `mise.toml`, and a Cargo workspace
//! member named `oakum` (self-host) for an exact oakum version, and the pins
//! it finds must agree (ADR-0038). The write gate compares this binary to
//! them. A missed look is `unverified`, not `ok`.

use std::io;
use std::path::{Path, PathBuf};

use cap_std::fs::Dir;
use oakum::discover::cargo_members;
use semver::Version;

use super::fs::{read_resolved_text, repo_path_display};
use super::repository::Repository;
use super::workflow::npm_workspace;
use super::CliError;

mod workflow_pins;

use workflow_pins::exact_version;
pub(super) use workflow_pins::{versions_in_workflow, PinError};

/// The published npm package; bare `oakum` on npm is not this tool.
const NPM_PACKAGE: &str = "@oakoss/oakum";

/// The mise key the no-pin advice names: it installs the prebuilt release
/// binary, where `cargo:` compiles and `npm:` needs Node (ADR-0038).
const MISE_GITHUB_KEY: &str = "github:oakoss/oakum";

/// The install-pin look: the pins agree with each other. Whether they name this
/// binary is the write gate's question, not this look's. Takes the caller's
/// read, so `check` reads the pins once for every look that needs them.
pub(super) fn agreement(
    read: &Result<Option<Pins>, CliError>,
    npm_workspace: bool,
) -> Result<(), CliError> {
    match read {
        Ok(Some(Pins::Agreed(AgreedPin {
            cargo_unread: None, ..
        }))) => Ok(()),
        Ok(Some(Pins::Agreed(AgreedPin {
            cargo_unread: Some(err),
            ..
        })))
        | Err(err) => Err(err.clone()),
        Ok(Some(Pins::Disagree(pins))) => Err(CliError::unverified(disagreement(pins))),
        Ok(None) => Err(CliError::unverified(no_pin(npm_workspace))),
    }
}

/// Where the pins leave this binary: whether `oakum upgrade` run from it would
/// be let through. Only `Settled` is.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Standing<'a> {
    Settled,
    Elsewhere(&'a Version),
    Unsettled,
}

pub(super) fn standing(read: &Result<Option<Pins>, CliError>) -> Standing<'_> {
    match read {
        Ok(None) => Standing::Settled,
        Ok(Some(Pins::Agreed(pin))) if pin.version != binary_version() => {
            Standing::Elsewhere(&pin.version)
        }
        Ok(Some(Pins::Agreed(pin))) if pin.cargo_unread.is_none() => Standing::Settled,
        Ok(Some(Pins::Agreed(_) | Pins::Disagree(_))) | Err(_) => Standing::Unsettled,
    }
}

fn binary_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("CARGO_PKG_VERSION is a semver version")
}

/// What a missing or unsettled pin does to a gated command (ADR-0038). Agreed
/// pins that name another version stop every policy with exit 1.
#[derive(Clone, Copy, Debug)]
pub(super) enum PinPolicy {
    /// `add`, `generate`, `version`, `ci version-pr`: no settled pin, no run.
    Writer,
    /// `release` reports a missing or unsettled pin in its own precondition.
    Release,
    /// `init` and `migrate` adopt oakum, often before a pin exists.
    Adopt,
    /// `upgrade` runs without a pin, but not with one it cannot settle.
    Upgrade,
}

/// A pin the gate let a command run without. The two exit differently: finding
/// nothing is the normal order for adopting oakum, while a look that failed
/// must not read as one that found nothing (ADR-0038).
#[derive(Debug)]
pub(super) enum Owed {
    /// Nothing pins oakum; carries the no-pin advice.
    Missing(CliError),
    /// Pins that disagree, or a source that could not be read or settled.
    Unsettled(CliError),
}

impl Owed {
    /// The step `init` and `migrate` print. `workflow` is the YAML this run
    /// prints, if any: when it installs oakum at a version and nothing else
    /// pins it, that line is the pin, so a second one is not advised.
    pub(super) fn step(&self, workflow: Option<&str>) -> String {
        match self {
            Self::Missing(advice) => workflow.and_then(workflow_pin_line).map_or_else(
                || advice.detail(),
                |line| {
                    format!(
                        "no oakum install pin yet; `{line}` in the printed workflow is the pin: \
                         commit the workflow under `.github/workflows/`"
                    )
                },
            ),
            Self::Unsettled(err) => err.detail(),
        }
    }

    /// The advice as it stands alone, with no workflow beside it.
    pub(super) fn detail(&self) -> String {
        self.step(None)
    }

    /// The refusal a run owes after its writes; a missing pin owes none.
    pub(super) fn refusal(&self) -> Option<CliError> {
        match self {
            Self::Missing(_) => None,
            Self::Unsettled(err) => Some(err.clone()),
        }
    }
}

/// The first line of `workflow` that installs oakum at the version `check`
/// reads from it.
fn workflow_pin_line(workflow: &str) -> Option<&str> {
    let version = versions_in_workflow(workflow).ok()?.into_iter().next()?;
    let spec = format!("oakum@{version}");
    workflow
        .lines()
        .map(str::trim)
        .find(|line| line.contains(&spec))
        .map(|line| line.trim_start_matches("- ").trim_start_matches("run: "))
}

/// The write gate: this binary against the repository's pin. `Ok(Some(owed))`
/// is a pin [`PinPolicy::Adopt`] or [`PinPolicy::Upgrade`] lets through that
/// the reader still owes.
pub(super) fn gate(repo: &Repository, policy: PinPolicy) -> Result<Option<Owed>, CliError> {
    let binary = binary_version();
    let owed = match read_pin(repo) {
        // A mismatch decides even when the Cargo source could not be read.
        Ok(Some(Pins::Agreed(pin))) => match (mismatch(&pin, &binary), pin.cargo_unread) {
            (Some(message), _) => return Err(CliError::new(message)),
            (None, None) => return Ok(None),
            (None, Some(err)) => Owed::Unsettled(err),
        },
        Ok(Some(Pins::Disagree(pins))) => {
            Owed::Unsettled(CliError::unverified(disagreement(&pins)))
        }
        Ok(None) => Owed::Missing(CliError::unverified(no_pin(npm_workspace(repo.path())))),
        Err(err) => Owed::Unsettled(err),
    };
    match (policy, owed) {
        (PinPolicy::Release, _) => Ok(None),
        (PinPolicy::Adopt, owed) | (PinPolicy::Upgrade, owed @ Owed::Missing(_)) => Ok(Some(owed)),
        (PinPolicy::Writer, Owed::Missing(err) | Owed::Unsettled(err))
        | (PinPolicy::Upgrade, Owed::Unsettled(err)) => Err(err),
    }
}

/// The fix follows the direction: an older binary is the stale side, a newer
/// one means the pin is.
fn mismatch(pin: &AgreedPin, binary: &Version) -> Option<String> {
    let mut sources: Vec<&Path> = Vec::new();
    for source in &pin.sources {
        if !sources.contains(&source.as_path()) {
            sources.push(source);
        }
    }
    let listed = sources
        .iter()
        .map(|source| repo_path_display(source))
        .collect::<Vec<_>>()
        .join(", ");
    let pinned = &pin.version;
    let fix = match binary.cmp(pinned) {
        std::cmp::Ordering::Equal => return None,
        std::cmp::Ordering::Less => {
            let pnpm_exec = if sources.contains(&Path::new("package.json")) {
                ", or run the pinned one with `pnpm exec oakum`"
            } else {
                ""
            };
            format!("install oakum `{pinned}`{pnpm_exec}")
        }
        std::cmp::Ordering::Greater => format!("raise the pin in {listed} to `{binary}`"),
    };
    Some(format!(
        "this binary is `{binary}` but the install pin is `{pinned}` ({listed}); {fix}"
    ))
}

/// Each version and source once, in the order found: a workflow that pins the
/// same version twice says nothing the first mention did not.
fn disagreement(pins: &[FoundPin]) -> String {
    let mut listed: Vec<(&Version, &Path)> = Vec::new();
    for pin in pins {
        let entry = (&pin.version, pin.source.as_path());
        if !listed.contains(&entry) {
            listed.push(entry);
        }
    }
    let listed = listed
        .iter()
        .map(|(version, source)| format!("`{version}` ({})", repo_path_display(source)))
        .collect::<Vec<_>>()
        .join(", ");
    format!("unverified: install pins disagree: {listed}; keep one exact version")
}

fn no_pin(npm_workspace: bool) -> String {
    let binary = env!("CARGO_PKG_VERSION");
    // pnpm 11 and 12 fail `pnpm add` and a later `pnpm install` on oakum's
    // unapproved install script; the flag writes `allowBuilds`.
    let pin = if npm_workspace {
        format!("`pnpm add -D --allow-build={NPM_PACKAGE} {NPM_PACKAGE}@{binary}`")
    } else {
        format!("add `\"{MISE_GITHUB_KEY}\" = \"{binary}\"` under `[tools]` in `.mise.toml`")
    };
    format!(
        "unverified: no oakum install pin in `.github/workflows`, `.github/actions`, \
         `package.json`, `.mise.toml`, `mise.toml`, or a Cargo workspace member named `oakum`; \
         pin this binary's version: {pin}"
    )
}

#[derive(Debug)]
pub(super) struct FoundPin {
    source: PathBuf,
    version: Version,
}

/// Every pin the look found, settled into one version or kept apart.
#[derive(Debug)]
pub(super) enum Pins {
    Agreed(AgreedPin),
    /// Two or more versions, each pin with its source, in the order found.
    Disagree(Vec<FoundPin>),
}

/// One version that every pin names. Built only by [`read_pin`] from at least
/// one pin, so `sources` is never empty.
#[derive(Debug)]
pub(super) struct AgreedPin {
    version: Version,
    /// One entry per pin found, so a file that pins twice is listed twice.
    sources: Vec<PathBuf>,
    /// The Cargo source could not be read, so the agreement holds only for the
    /// others: enough to name a mismatch, not to call the pin settled.
    cargo_unread: Option<CliError>,
}

/// The repository's oakum pin. `Ok(None)` means nothing pins oakum; `Err` is a
/// pin source that could not be read or settled, which is `unverified`.
/// Pins that disagree within `package.json`, a mise file, or the mise pair are
/// already `Err`; a workflow or action file that pins two versions reaches
/// [`Pins::Disagree`] with that file listed for each.
pub(super) fn read_pin(repo: &Repository) -> Result<Option<Pins>, CliError> {
    settle(collect_pins(repo.dir()), cargo_member_pin(repo))
}

/// A Cargo source that could not be read leaves the other pins standing: they
/// can still name a mismatch, and only when nothing else pins oakum is its
/// error the answer.
fn settle(
    files: Result<Vec<FoundPin>, Box<dyn std::error::Error>>,
    cargo: Result<Option<FoundPin>, CliError>,
) -> Result<Option<Pins>, CliError> {
    let mut pins = files.map_err(CliError::from_boxed)?;
    let cargo_unread = match cargo {
        Ok(pin) => {
            pins.extend(pin);
            None
        }
        Err(err) => Some(err),
    };
    let Some(first) = pins.first() else {
        return cargo_unread.map_or(Ok(None), Err);
    };
    if pins.iter().any(|pin| pin.version != first.version) {
        return Ok(Some(Pins::Disagree(pins)));
    }
    let version = first.version.clone();
    let sources = pins.into_iter().map(|pin| pin.source).collect();
    Ok(Some(Pins::Agreed(AgreedPin {
        version,
        sources,
        cargo_unread,
    })))
}

#[cfg(test)]
mod owed_step {
    use super::Owed;
    use crate::cli::CliError;

    const WORKFLOW: &str =
        "jobs:\n  release:\n    steps:\n      - run: npm i -g @oakoss/oakum@1.2.3\n";

    #[test]
    fn a_printed_install_line_is_the_pin_when_nothing_else_pins() {
        let owed = Owed::Missing(CliError::unverified(
            "unverified: no oakum install pin in x",
        ));
        assert_eq!(
            owed.step(Some(WORKFLOW)),
            "no oakum install pin yet; `npm i -g @oakoss/oakum@1.2.3` in the printed workflow \
             is the pin: commit the workflow under `.github/workflows/`"
        );
        assert_eq!(owed.step(None), "no oakum install pin in x");
        assert_eq!(
            owed.step(Some("jobs: {}\n")),
            "no oakum install pin in x",
            "a workflow with no install line"
        );
    }

    #[test]
    fn unsettled_pins_are_not_told_to_commit_another() {
        let owed = Owed::Unsettled(CliError::unverified("unverified: install pins disagree"));
        assert_eq!(owed.step(Some(WORKFLOW)), "install pins disagree");
        assert!(owed.refusal().is_some());
    }
}

#[cfg(test)]
mod mismatch_text {
    use std::path::PathBuf;

    use super::{mismatch, AgreedPin};

    fn text(pinned: &str, sources: &[&str], binary: &str) -> Option<String> {
        let pin = AgreedPin {
            version: pinned.parse().expect("pinned"),
            sources: sources.iter().map(PathBuf::from).collect(),
            cargo_unread: None,
        };
        mismatch(&pin, &binary.parse().expect("binary"))
    }

    #[test]
    fn a_matching_pin_says_nothing() {
        assert_eq!(text("0.5.1", &[".mise.toml"], "0.5.1"), None);
    }

    #[test]
    fn a_binary_older_than_the_pin_names_installing_the_pin() {
        assert_eq!(
            text(
                "0.6.0",
                &[".github/workflows/release.yml", ".mise.toml"],
                "0.5.1"
            )
            .as_deref(),
            Some(
                "this binary is `0.5.1` but the install pin is `0.6.0` \
                 (.github/workflows/release.yml, .mise.toml); install oakum `0.6.0`"
            )
        );
    }

    #[test]
    fn a_package_json_pin_adds_pnpm_exec_to_the_install() {
        assert_eq!(
            text("0.6.0", &["package.json"], "0.5.1").as_deref(),
            Some(
                "this binary is `0.5.1` but the install pin is `0.6.0` (package.json); \
                 install oakum `0.6.0`, or run the pinned one with `pnpm exec oakum`"
            )
        );
    }

    #[test]
    fn a_binary_newer_than_the_pin_names_raising_it_in_each_source_once() {
        assert_eq!(
            text(
                "0.4.0",
                &[
                    ".github/workflows/release.yml",
                    ".github/workflows/release.yml",
                    "package.json",
                ],
                "0.5.1",
            )
            .as_deref(),
            Some(
                "this binary is `0.5.1` but the install pin is `0.4.0` \
                 (.github/workflows/release.yml, package.json); raise the pin in \
                 .github/workflows/release.yml, package.json to `0.5.1`"
            )
        );
    }
}

#[cfg(test)]
mod required_reads {
    /// A workflow listed by the directory walk and gone by the open is a look
    /// that did not happen, not an empty file with no pin in it.
    #[test]
    fn a_listed_file_that_is_gone_is_unverified_rather_than_empty() {
        let dir =
            cap_std::fs::Dir::open_ambient_dir(std::env::temp_dir(), cap_std::ambient_authority())
                .expect("temp dir");
        let err = super::read_text(&dir, std::path::Path::new("oakum-no-such-workflow.yml"))
            .expect_err("nothing to read");
        assert!(
            err.to_string().contains("disappeared during the look"),
            "{err}"
        );
    }
}

#[cfg(test)]
mod pin_reading {
    use std::path::PathBuf;

    use cap_std::fs::Dir;
    use semver::Version;

    use super::{collect_pins, settle, Pins};
    use crate::test_fixture::Fixture;

    /// The pins of every source but Cargo, which a fixture without a
    /// `Cargo.toml` would read as absent.
    fn read_files(dir: &Dir) -> Result<Option<Pins>, crate::cli::CliError> {
        settle(collect_pins(dir), Ok(None))
    }

    fn read(root: &Fixture) -> Option<Pins> {
        let dir = Dir::open_ambient_dir(root, cap_std::ambient_authority()).unwrap();
        read_files(&dir).expect("pins settle")
    }

    fn workflow_pin(root: &Fixture, name: &str, version: &str) {
        std::fs::create_dir_all(root.join(".github/workflows")).unwrap();
        std::fs::write(
            root.join(".github/workflows").join(name),
            format!("run: cargo binstall --no-confirm oakum@{version}\n"),
        )
        .unwrap();
    }

    fn package_json_pin(root: &Fixture, version: &str) {
        std::fs::write(
            root.join("package.json"),
            format!(r#"{{"devDependencies":{{"@oakoss/oakum":"{version}"}}}}"#),
        )
        .unwrap();
    }

    #[test]
    fn no_pin_is_none() {
        let root = Fixture::new("read-pin", "none");
        std::fs::write(root.join("package.json"), "{}").unwrap();
        assert!(read(&root).is_none());
    }

    #[test]
    fn one_pin_is_agreed_with_its_source() {
        let root = Fixture::new("read-pin", "one");
        package_json_pin(&root, "0.3.0");
        let Some(Pins::Agreed(pin)) = read(&root) else {
            panic!("one pin agrees with itself");
        };
        assert_eq!(pin.version, Version::new(0, 3, 0));
        assert_eq!(pin.sources, [PathBuf::from("package.json")]);
    }

    #[test]
    fn agreeing_pins_in_two_files_list_both_sources() {
        let root = Fixture::new("read-pin", "agree");
        workflow_pin(&root, "release.yml", "0.3.0");
        package_json_pin(&root, "0.3.0");
        let Some(Pins::Agreed(pin)) = read(&root) else {
            panic!("same version in both files agrees");
        };
        assert_eq!(pin.version, Version::new(0, 3, 0));
        assert_eq!(
            pin.sources,
            [
                PathBuf::from(".github/workflows/release.yml"),
                PathBuf::from("package.json"),
            ]
        );
    }

    #[test]
    fn a_disagreement_lists_each_version_and_source_once() {
        let root = Fixture::new("read-pin", "disagree-twice");
        std::fs::create_dir_all(root.join(".github/workflows")).unwrap();
        std::fs::write(
            root.join(".github/workflows/release.yml"),
            "- run: cargo binstall --no-confirm oakum@0.3.0\n- run: cargo install oakum@0.3.0\n",
        )
        .unwrap();
        package_json_pin(&root, "0.4.0");
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = super::agreement(&read_files(&dir), false).expect_err("two versions");
        assert_eq!(
            err.to_string(),
            "unverified: install pins disagree: `0.3.0` (.github/workflows/release.yml), \
             `0.4.0` (package.json); keep one exact version"
        );
    }

    #[test]
    fn agreeing_mise_files_are_both_sources() {
        let root = Fixture::new("read-pin", "both-mise");
        for name in [".mise.toml", "mise.toml"] {
            std::fs::write(
                root.join(name),
                "[tools]\n\"github:oakoss/oakum\" = \"0.0.1\"\n",
            )
            .unwrap();
        }
        let Some(Pins::Agreed(pin)) = read(&root) else {
            panic!("the same version in both mise files agrees");
        };
        assert_eq!(
            pin.sources,
            [PathBuf::from(".mise.toml"), PathBuf::from("mise.toml")]
        );
    }

    #[test]
    fn workflow_pins_are_listed_by_file_name() {
        let root = Fixture::new("read-pin", "by-name");
        for (name, version) in [("c.yml", "0.5.0"), ("b.yml", "0.4.0"), ("a.yml", "0.3.0")] {
            workflow_pin(&root, name, version);
        }
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = super::agreement(&read_files(&dir), false).expect_err("three versions");
        assert_eq!(
            err.to_string(),
            "unverified: install pins disagree: `0.3.0` (.github/workflows/a.yml), \
             `0.4.0` (.github/workflows/b.yml), `0.5.0` (.github/workflows/c.yml); \
             keep one exact version"
        );
    }

    #[test]
    fn disagreeing_pins_in_two_files_keep_each_version_and_source() {
        let root = Fixture::new("read-pin", "disagree");
        workflow_pin(&root, "release.yml", "0.3.0");
        package_json_pin(&root, "0.4.0");
        let Some(Pins::Disagree(pins)) = read(&root) else {
            panic!("two versions disagree");
        };
        let found: Vec<(Version, PathBuf)> = pins
            .into_iter()
            .map(|pin| (pin.version, pin.source))
            .collect();
        assert_eq!(
            found,
            [
                (
                    Version::new(0, 3, 0),
                    PathBuf::from(".github/workflows/release.yml"),
                ),
                (Version::new(0, 4, 0), PathBuf::from("package.json")),
            ]
        );
    }
}

/// Every pin source but the Cargo workspace, which [`cargo_member_pin`] reads.
fn collect_pins(dir: &Dir) -> Result<Vec<FoundPin>, Box<dyn std::error::Error>> {
    let mut pins = Vec::new();
    scan_workflows(dir, &mut pins)?;
    scan_composite_actions(dir, &mut pins)?;
    if let Some(pin) = read_package_json_pin(dir)? {
        pins.push(pin);
    }
    pins.extend(read_mise_pins(dir)?);
    Ok(pins)
}

/// Self-host pin (ADR-0007): the member named `oakum` of the Cargo workspace at
/// the repository root, as `cargo metadata` lists it, so membership is Cargo's
/// answer. Read from the root rather than the working directory, so every
/// subdirectory sees one pin.
fn cargo_member_pin(repo: &Repository) -> Result<Option<FoundPin>, CliError> {
    if !cargo_manifest_present(repo.dir())? {
        return Ok(None);
    }
    let unread = |detail: String| {
        CliError::unverified(format!(
            "unverified: could not read the Cargo workspace for a member named `oakum` (cargo: {detail})"
        ))
    };
    let root = repo.ambient_path().map_err(|err| unread(err.to_string()))?;
    let members = cargo_members(root, root).map_err(|err| unread(err.to_string()))?;
    let Some(member) = members.into_iter().find(|member| member.name == "oakum") else {
        return Ok(None);
    };
    let source = Path::new(&member.manifest_dir).join("Cargo.toml");
    require_declared_version(repo.dir(), &source)?;
    Ok(Some(FoundPin {
        source,
        version: member.version,
    }))
}

/// Cargo reports `0.0.0` for a package that declares no version, which would
/// read as a pin the manifest never wrote.
fn require_declared_version(dir: &Dir, source: &Path) -> Result<(), CliError> {
    let shown = repo_path_display(source);
    let text = read_pin_source(dir, source)?.ok_or_else(|| {
        CliError::unverified(format!("unverified: `{shown}` disappeared during the look"))
    })?;
    let manifest: toml::Value = toml::from_str(&text).map_err(|err| {
        CliError::unverified(format!("unverified: `{shown}` is not valid TOML: {err}"))
    })?;
    // Cargo still accepts the pre-2024 `[project]` spelling of `[package]`.
    let version = manifest
        .get("package")
        .or_else(|| manifest.get("project"))
        .and_then(|package| package.get("version"));
    let declared = version.is_some_and(|version| {
        version.is_str() || version.get("workspace").and_then(toml::Value::as_bool) == Some(true)
    });
    if declared {
        Ok(())
    } else {
        Err(CliError::unverified(format!(
            "unverified: `{shown}` package `oakum` has no exact `version`"
        )))
    }
}

/// Classified as every other pin source is, before cargo sees it: cargo would
/// block on a FIFO, and a dangling link must not read as no workspace.
fn cargo_manifest_present(dir: &Dir) -> Result<bool, CliError> {
    Ok(read_pin_source(dir, Path::new("Cargo.toml"))?.is_some())
}

fn scan_workflows(dir: &Dir, pins: &mut Vec<FoundPin>) -> Result<(), Box<dyn std::error::Error>> {
    let entries = match dir.read_dir(".github/workflows") {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            refuse_dangling(dir, Path::new(".github/workflows"))?;
            return Ok(());
        }
        Err(err) => {
            return Err(Box::new(CliError::unverified(format!(
                "unverified: failed to read `.github/workflows`: {err}"
            ))));
        }
    };
    // Sorted so a message listing pins reads the same on every filesystem.
    let mut entries = entries.collect::<Result<Vec<_>, _>>().map_err(|err| {
        CliError::unverified(format!(
            "unverified: failed to read `.github/workflows`: {err}"
        ))
    })?;
    entries.sort_by_key(cap_std::fs::DirEntry::file_name);
    for entry in entries {
        let name = entry.file_name();
        let path = Path::new(".github/workflows").join(&name);
        let Some(name) = name.to_str() else {
            return Err(Box::new(CliError::unverified(format!(
                "unverified: workflow path `{}` is not valid UTF-8",
                repo_path_display(&path)
            ))));
        };
        let is_yaml = Path::new(name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("yml") || ext.eq_ignore_ascii_case("yaml"));
        if !is_yaml {
            continue;
        }
        let file_type = entry.file_type().map_err(|err| {
            CliError::unverified(format!(
                "unverified: failed to inspect `{}`: {err}",
                repo_path_display(&path)
            ))
        })?;
        if !file_type.is_file() {
            return Err(Box::new(CliError::unverified(format!(
                "unverified: `{}` is not a file",
                repo_path_display(&path)
            ))));
        }
        let text = read_text(dir, &path)?;
        push_yaml_pins(&path, &text, pins)?;
    }
    Ok(())
}

fn scan_composite_actions(
    dir: &Dir,
    pins: &mut Vec<FoundPin>,
) -> Result<(), Box<dyn std::error::Error>> {
    let entries = match dir.read_dir(".github/actions") {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            refuse_dangling(dir, Path::new(".github/actions"))?;
            return Ok(());
        }
        Err(err) => {
            return Err(Box::new(CliError::unverified(format!(
                "unverified: failed to read `.github/actions`: {err}"
            ))));
        }
    };
    let mut entries = entries.collect::<Result<Vec<_>, _>>().map_err(|err| {
        CliError::unverified(format!(
            "unverified: failed to read `.github/actions`: {err}"
        ))
    })?;
    entries.sort_by_key(cap_std::fs::DirEntry::file_name);
    for entry in entries {
        let name = entry.file_name();
        let action_dir = Path::new(".github/actions").join(&name);
        let file_type = entry.file_type().map_err(|err| {
            CliError::unverified(format!(
                "unverified: failed to inspect `{}`: {err}",
                repo_path_display(&action_dir)
            ))
        })?;
        let is_action_dir = if file_type.is_dir() {
            true
        } else if file_type.is_symlink() {
            match dir.metadata(&action_dir) {
                Ok(meta) => meta.is_dir(),
                Err(err) if err.kind() == io::ErrorKind::NotFound => {
                    refuse_dangling(dir, &action_dir)?;
                    false
                }
                Err(err) => {
                    return Err(Box::new(CliError::unverified(format!(
                        "unverified: failed to inspect `{}`: {err}",
                        repo_path_display(&action_dir)
                    ))));
                }
            }
        } else {
            false
        };
        if !is_action_dir {
            continue;
        }
        for file_name in ["action.yml", "action.yaml"] {
            let path = action_dir.join(file_name);
            let Some(text) = read_pin_source(dir, &path)? else {
                continue;
            };
            push_yaml_pins(&path, &text, pins)?;
        }
    }
    Ok(())
}

fn push_yaml_pins(
    path: &Path,
    text: &str,
    pins: &mut Vec<FoundPin>,
) -> Result<(), Box<dyn std::error::Error>> {
    for version in versions_in_workflow(text).map_err(|err| {
        let path = repo_path_display(path);
        CliError::unverified(match err {
            PinError::Unversioned => {
                format!("unverified: `{path}` installs oakum without a version")
            }
            PinError::UnparseableYaml => format!("unverified: `{path}` is not valid YAML"),
            PinError::WithNotObject => {
                format!("unverified: `{path}` has a `with` value that is not a mapping")
            }
            PinError::Inexact(raw) => {
                format!("unverified: `{path}` pins oakum as `{raw}`, which is not an exact version")
            }
        })
    })? {
        pins.push(FoundPin {
            source: path.to_path_buf(),
            version,
        });
    }
    Ok(())
}

/// A pin source that resolved to nothing. A symlink to a missing target at
/// `path` or any parent could hide a pin that disagrees with the others, so it
/// is `unverified` rather than absent (ADR-0038).
fn refuse_dangling(dir: &Dir, path: &Path) -> Result<(), CliError> {
    for prefix in path
        .ancestors()
        .filter(|prefix| !prefix.as_os_str().is_empty())
    {
        let is_link = dir
            .symlink_metadata(prefix)
            .is_ok_and(|meta| meta.is_symlink());
        if is_link
            && dir
                .metadata(prefix)
                .is_err_and(|err| err.kind() == io::ErrorKind::NotFound)
        {
            return Err(CliError::unverified(format!(
                "unverified: `{}` is a symlink to a missing target",
                repo_path_display(prefix)
            )));
        }
    }
    Ok(())
}

/// [`read_resolved_text`] for a pin source, where a dangling symlink is not absence.
fn read_pin_source(dir: &Dir, path: &Path) -> Result<Option<String>, CliError> {
    let text = read_resolved_text(dir, path)?;
    if text.is_none() {
        refuse_dangling(dir, path)?;
    }
    Ok(text)
}

fn read_text(dir: &Dir, path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    match read_resolved_text(dir, path)? {
        Some(text) => Ok(text),
        None => Err(Box::new(CliError::unverified(format!(
            "unverified: `{}` disappeared during the look",
            repo_path_display(path)
        )))),
    }
}

fn read_package_json_pin(dir: &Dir) -> Result<Option<FoundPin>, Box<dyn std::error::Error>> {
    let Some(text) = read_pin_source(dir, Path::new("package.json"))? else {
        return Ok(None);
    };
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|err| {
        CliError::unverified(format!(
            "unverified: `package.json` is not valid JSON: {err}"
        ))
    })?;
    let Some(object) = value.as_object() else {
        return Err(Box::new(CliError::unverified(
            "unverified: `package.json` is not a JSON object",
        )));
    };
    let mut pin: Option<(&str, Version)> = None;
    for section in [
        "dependencies",
        "devDependencies",
        "optionalDependencies",
        "peerDependencies",
    ] {
        let Some(section_value) = object.get(section) else {
            continue;
        };
        let Some(deps) = section_value.as_object() else {
            return Err(Box::new(CliError::unverified(format!(
                "unverified: `package.json` `{section}` is not an object"
            ))));
        };
        for name in [NPM_PACKAGE, "oakum"] {
            let Some(oakum) = deps.get(name) else {
                continue;
            };
            let Some(raw) = oakum.as_str() else {
                return Err(Box::new(CliError::unverified(format!(
                    "unverified: `package.json` pins `{name}` with a non-string value"
                ))));
            };
            let Some(version) = exact_version(raw) else {
                return Err(Box::new(CliError::unverified(format!(
                    "unverified: `package.json` pins `{name}` as `{raw}`, which is not an exact version"
                ))));
            };
            match &pin {
                None => pin = Some((name, version)),
                Some((first, existing)) if *existing != version => {
                    return Err(Box::new(CliError::unverified(format!(
                        "unverified: `package.json` pins `{first}` as `{existing}` and `{name}` as `{version}`; keep one exact version"
                    ))));
                }
                Some(_) => {}
            }
        }
    }
    Ok(pin.map(|(_, version)| FoundPin {
        source: PathBuf::from("package.json"),
        version,
    }))
}

/// Both mise files when they agree, so a remedy that names its sources names
/// every file it has to change.
fn read_mise_pins(dir: &Dir) -> Result<Vec<FoundPin>, Box<dyn std::error::Error>> {
    let mut found: Vec<FoundPin> = Vec::new();
    for name in [".mise.toml", "mise.toml"] {
        let Some(pin) = read_one_mise(dir, name)? else {
            continue;
        };
        if let Some(existing) = found.first() {
            if existing.version != pin.version {
                return Err(Box::new(CliError::unverified(format!(
                    "unverified: `{}` pins oakum as `{}` but `{}` pins `{pin_version}`",
                    repo_path_display(&existing.source),
                    existing.version,
                    repo_path_display(&pin.source),
                    pin_version = pin.version,
                ))));
            }
        }
        found.push(pin);
    }
    Ok(found)
}

fn read_one_mise(dir: &Dir, name: &str) -> Result<Option<FoundPin>, Box<dyn std::error::Error>> {
    let Some(text) = read_pin_source(dir, Path::new(name))? else {
        return Ok(None);
    };
    let value: toml::Value = toml::from_str(&text).map_err(|err| {
        CliError::unverified(format!("unverified: `{name}` is not valid TOML: {err}"))
    })?;
    let Some(tools_value) = value.get("tools") else {
        return Ok(None);
    };
    let Some(tools) = tools_value.as_table() else {
        return Err(Box::new(CliError::unverified(format!(
            "unverified: `{name}` has a `tools` value that is not a table"
        ))));
    };
    let mut pin = None;
    for (key, spec) in tools {
        if !is_mise_oakum_key(key) {
            continue;
        }
        let raw = mise_version_spec(spec).ok_or_else(|| {
            CliError::unverified(format!(
                "unverified: `{name}` pins `{key}` with a value that is not an exact version"
            ))
        })?;
        let Some(version) = exact_version(raw) else {
            return Err(Box::new(CliError::unverified(format!(
                "unverified: `{name}` pins oakum as `{raw}`, which is not an exact version"
            ))));
        };
        match &pin {
            None => pin = Some(version),
            Some(existing) if *existing != version => {
                return Err(Box::new(CliError::unverified(format!(
                    "unverified: `{name}` pins oakum as both `{existing}` and `{version}`"
                ))));
            }
            Some(_) => {}
        }
    }
    Ok(pin.map(|version| FoundPin {
        source: PathBuf::from(name),
        version,
    }))
}

fn is_mise_oakum_key(key: &str) -> bool {
    key == "oakum" || key == "cargo:oakum" || key == "npm:@oakoss/oakum" || key == MISE_GITHUB_KEY
}

fn mise_version_spec(spec: &toml::Value) -> Option<&str> {
    spec.as_str()
        .or_else(|| spec.get("version").and_then(toml::Value::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test_fixture::Fixture;

    fn scratch(label: &str) -> Fixture {
        Fixture::new("pin", label)
    }

    #[test]
    fn package_json_later_section_cannot_hide_an_inexact_pin() {
        let root = scratch("pkg-conflict");
        std::fs::write(
            root.join("package.json"),
            r#"{"dependencies":{"oakum":"0.0.0"},"devDependencies":{"oakum":"latest"}}"#,
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("latest in later section");
        assert!(err.to_string().contains("latest"), "{err}");
    }

    #[test]
    fn package_json_peer_section_cannot_hide_an_inexact_pin() {
        let root = scratch("pkg-peer");
        std::fs::write(
            root.join("package.json"),
            r#"{"dependencies":{"oakum":"0.0.0"},"peerDependencies":{"oakum":"latest"}}"#,
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("peer latest");
        assert!(err.to_string().contains("latest"), "{err}");
    }

    #[test]
    fn package_json_range_is_not_an_exact_pin() {
        let root = scratch("pkg-range");
        std::fs::write(
            root.join("package.json"),
            r#"{"devDependencies":{"oakum":"^0.1.0"}}"#,
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("range");
        assert!(err.to_string().contains("not an exact version"), "{err}");
        assert!(err.to_string().contains("^0.1.0"), "{err}");
    }

    #[test]
    fn inexact_pin_is_unverified_even_when_another_pin_matches() {
        let root = scratch("latest-and-exact");
        std::fs::create_dir_all(root.join(".github/workflows")).unwrap();
        std::fs::write(
            root.join(".github/workflows/release.yml"),
            "run: cargo binstall --no-confirm oakum@0.0.0\n",
        )
        .unwrap();
        std::fs::write(
            root.join(".github/workflows/ci.yml"),
            "run: cargo binstall --no-confirm oakum@latest\n",
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("latest");
        assert!(err.to_string().contains("latest"), "{err}");
        assert!(err.to_string().contains("not an exact version"), "{err}");
    }

    #[test]
    fn invalid_workflow_yaml_is_unverified() {
        let root = scratch("workflow-invalid-yaml");
        std::fs::create_dir_all(root.join(".github/workflows")).unwrap();
        std::fs::write(root.join(".github/workflows/ci.yml"), "on: [\n").unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"devDependencies":{"oakum":"0.0.0"}}"#,
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("invalid yaml");
        assert!(err.to_string().contains("not valid YAML"), "{err}");
        assert!(err.to_string().contains("ci.yml"), "{err}");
    }

    #[test]
    fn unversioned_workflow_message_uses_forward_slashes() {
        let root = scratch("unversioned-slash");
        std::fs::create_dir_all(root.join(".github/workflows")).unwrap();
        std::fs::write(
            root.join(".github/workflows/ci.yml"),
            "- uses: oakoss/install-action@v1\n  with:\n    tool: oakum\n",
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("unversioned");
        let text = err.to_string();
        assert!(
            text.contains(".github/workflows/ci.yml"),
            "unversioned pin must name the workflow with /: {text}"
        );
        assert!(
            !text.contains(".github/workflows\\ci.yml"),
            "unversioned pin must not use a backslash: {text}"
        );
    }

    #[test]
    fn invalid_package_json_is_unverified() {
        let root = scratch("pkg-invalid");
        std::fs::write(root.join("package.json"), "{").unwrap();
        std::fs::create_dir_all(root.join(".github/workflows")).unwrap();
        std::fs::write(
            root.join(".github/workflows/release.yml"),
            "run: cargo binstall --no-confirm oakum@0.0.0\n",
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("invalid json");
        assert!(err.to_string().contains("not valid JSON"), "{err}");
    }

    #[test]
    fn package_json_exact_pin_is_collected() {
        let root = scratch("pkg-exact");
        std::fs::write(
            root.join("package.json"),
            r#"{"devDependencies":{"oakum":"0.4.2"}}"#,
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let pins = collect_pins(&dir).unwrap();
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].source, PathBuf::from("package.json"));
        assert_eq!(pins[0].version, Version::parse("0.4.2").unwrap());
    }

    #[test]
    fn package_json_conflicting_exact_versions_are_unverified() {
        let root = scratch("pkg-both-exact");
        std::fs::write(
            root.join("package.json"),
            r#"{"dependencies":{"oakum":"0.0.0"},"devDependencies":{"oakum":"1.0.0"}}"#,
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("conflicting exacts");
        assert!(
            err.to_string()
                .contains("pins `oakum` as `0.0.0` and `oakum` as `1.0.0`"),
            "{err}"
        );
    }

    #[test]
    fn package_json_non_string_pin_is_unverified() {
        let root = scratch("pkg-object");
        std::fs::write(
            root.join("package.json"),
            r#"{"devDependencies":{"oakum":{"version":"0.0.0"}}}"#,
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("non-string");
        assert!(err.to_string().contains("non-string"), "{err}");
    }

    #[test]
    fn package_json_optional_section_inexact_pin_is_unverified() {
        let root = scratch("pkg-optional");
        std::fs::write(
            root.join("package.json"),
            r#"{"optionalDependencies":{"oakum":"latest"}}"#,
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("optional latest");
        assert!(err.to_string().contains("latest"), "{err}");
    }

    #[test]
    fn mise_exact_pin_is_collected() {
        let root = scratch("mise-exact");
        std::fs::write(root.join(".mise.toml"), "[tools]\noakum = \"0.4.2\"\n").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let pins = collect_pins(&dir).unwrap();
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].source, PathBuf::from(".mise.toml"));
        assert_eq!(pins[0].version, Version::parse("0.4.2").unwrap());
    }

    #[test]
    fn mise_cargo_backend_and_table_form_are_pins() {
        let root = scratch("mise-cargo");
        std::fs::write(
            root.join("mise.toml"),
            "[tools]\n\"cargo:oakum\" = { version = \"0.1.0\" }\n",
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let pins = collect_pins(&dir).unwrap();
        assert_eq!(pins[0].source, PathBuf::from("mise.toml"));
        assert_eq!(pins[0].version, Version::parse("0.1.0").unwrap());
    }

    #[test]
    fn mise_latest_is_unverified() {
        let root = scratch("mise-latest");
        std::fs::write(root.join(".mise.toml"), "[tools]\noakum = \"latest\"\n").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("latest");
        assert!(err.to_string().contains("latest"), "{err}");
    }

    #[test]
    fn mise_conflicting_files_are_unverified() {
        let root = scratch("mise-conflict");
        std::fs::write(root.join(".mise.toml"), "[tools]\noakum = \"0.1.0\"\n").unwrap();
        std::fs::write(root.join("mise.toml"), "[tools]\noakum = \"0.2.0\"\n").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("conflict");
        assert!(err.to_string().contains("0.1.0"), "{err}");
        assert!(err.to_string().contains("0.2.0"), "{err}");
    }

    #[test]
    fn mise_tools_that_is_not_a_table_is_unverified() {
        let root = scratch("mise-tools-string");
        std::fs::create_dir_all(root.join(".github/workflows")).unwrap();
        std::fs::write(
            root.join(".github/workflows/release.yml"),
            "run: cargo binstall --no-confirm oakum@0.0.0\n",
        )
        .unwrap();
        std::fs::write(root.join(".mise.toml"), "tools = \"oakum@latest\"\n").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("tools not a table");
        assert!(err.to_string().contains("not a table"), "{err}");
    }

    fn matching_workflow(root: &Path) {
        std::fs::create_dir_all(root.join(".github/workflows")).unwrap();
        std::fs::write(
            root.join(".github/workflows/release.yml"),
            "run: cargo binstall --no-confirm oakum@0.0.0\n",
        )
        .unwrap();
    }

    #[test]
    fn composite_action_run_step_is_a_pin() {
        let root = scratch("composite-pin");
        std::fs::create_dir_all(root.join(".github/actions/setup-oakum")).unwrap();
        std::fs::write(
            root.join(".github/actions/setup-oakum/action.yml"),
            "runs:\n  using: composite\n  steps:\n    - run: cargo binstall --no-confirm oakum@0.7.0\n      shell: bash\n",
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let pins = collect_pins(&dir).unwrap();
        assert_eq!(pins.len(), 1);
        assert_eq!(
            pins[0].source,
            PathBuf::from(".github/actions/setup-oakum/action.yml")
        );
        assert_eq!(pins[0].version, Version::parse("0.7.0").unwrap());
    }

    #[test]
    fn composite_action_yaml_extension_is_a_pin() {
        let root = scratch("composite-yaml-ext");
        std::fs::create_dir_all(root.join(".github/actions/setup-oakum")).unwrap();
        std::fs::write(
            root.join(".github/actions/setup-oakum/action.yaml"),
            "runs:\n  using: composite\n  steps:\n    - run: cargo binstall --no-confirm oakum@0.8.0\n      shell: bash\n",
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let pins = collect_pins(&dir).unwrap();
        assert_eq!(pins.len(), 1);
        assert_eq!(
            pins[0].source,
            PathBuf::from(".github/actions/setup-oakum/action.yaml")
        );
        assert_eq!(pins[0].version, Version::parse("0.8.0").unwrap());
    }

    #[test]
    #[cfg(unix)]
    fn composite_action_directory_symlink_is_a_pin() {
        let root = scratch("composite-symlink");
        std::fs::create_dir_all(root.join("real/setup-oakum")).unwrap();
        std::fs::write(
            root.join("real/setup-oakum/action.yml"),
            "runs:\n  using: composite\n  steps:\n    - run: cargo binstall --no-confirm oakum@0.9.0\n      shell: bash\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".github/actions")).unwrap();
        std::os::unix::fs::symlink(
            "../../real/setup-oakum",
            root.join(".github/actions/setup-oakum"),
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let pins = collect_pins(&dir).unwrap();
        assert_eq!(pins.len(), 1);
        assert_eq!(
            pins[0].source,
            PathBuf::from(".github/actions/setup-oakum/action.yml")
        );
        assert_eq!(pins[0].version, Version::parse("0.9.0").unwrap());
    }

    /// Each place a pin can live, as a link to nothing: read as absent, it
    /// would hide a pin that disagrees with the others.
    #[test]
    #[cfg(unix)]
    fn a_dangling_symlink_at_a_pin_source_is_unverified() {
        for (label, parent, link) in [
            ("dangling-workflows", ".github", ".github/workflows"),
            ("dangling-actions", ".github", ".github/actions"),
            (
                "dangling-action-dir",
                ".github/actions",
                ".github/actions/setup",
            ),
            (
                "dangling-action-yml",
                ".github/actions/setup",
                ".github/actions/setup/action.yml",
            ),
            ("dangling-mise", "", ".mise.toml"),
            ("dangling-bare-mise", "", "mise.toml"),
            ("dangling-package-json", "", "package.json"),
        ] {
            let root = scratch(label);
            std::fs::create_dir_all(root.join(parent)).unwrap();
            std::os::unix::fs::symlink("nowhere", root.join(link)).unwrap();
            let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
            let err = collect_pins(&dir).expect_err(link);
            assert_eq!(
                err.to_string(),
                format!("unverified: `{link}` is a symlink to a missing target"),
                "{link}"
            );
        }
    }

    /// A root `Cargo.toml` cargo cannot be handed is refused before cargo
    /// runs; absent, it is no Cargo workspace.
    #[test]
    fn a_root_cargo_toml_that_is_not_a_file_is_unverified() {
        let absent = scratch("cargo-absent");
        let dir = Dir::open_ambient_dir(&absent, cap_std::ambient_authority()).unwrap();
        assert!(!super::cargo_manifest_present(&dir).expect("absent"));

        let directory = scratch("cargo-directory");
        std::fs::create_dir(directory.join("Cargo.toml")).unwrap();
        let dir = Dir::open_ambient_dir(&directory, cap_std::ambient_authority()).unwrap();
        let err = super::cargo_manifest_present(&dir).expect_err("directory");
        assert_eq!(
            err.to_string(),
            "unverified: `Cargo.toml` is not a regular file"
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_dangling_root_cargo_toml_is_unverified() {
        let dangling = scratch("cargo-dangling");
        std::os::unix::fs::symlink("nowhere", dangling.join("Cargo.toml")).unwrap();
        let dir = Dir::open_ambient_dir(&dangling, cap_std::ambient_authority()).unwrap();
        let err = super::cargo_manifest_present(&dir).expect_err("dangling");
        assert_eq!(
            err.to_string(),
            "unverified: `Cargo.toml` is a symlink to a missing target"
        );
    }

    /// The Cargo source failing leaves the others to decide: a mismatch they
    /// agree on still reads, and with nothing else its error is the answer.
    #[test]
    fn an_unread_cargo_source_does_not_blank_the_others() {
        let unread = || Err(crate::cli::CliError::unverified("unverified: cargo"));
        let pin = super::FoundPin {
            source: PathBuf::from(".mise.toml"),
            version: Version::new(9, 9, 9),
        };
        match super::settle(Ok(vec![pin]), unread()) {
            Ok(Some(Pins::Agreed(agreed))) => {
                assert_eq!(agreed.version, Version::new(9, 9, 9));
                assert!(agreed.cargo_unread.is_some());
            }
            other => panic!("{other:?}"),
        }
        let err = super::settle(Ok(Vec::new()), unread()).expect_err("nothing else");
        assert_eq!(err.to_string(), "unverified: cargo");
        let pin_err = super::agreement(
            &super::settle(
                Ok(vec![super::FoundPin {
                    source: PathBuf::from(".mise.toml"),
                    version: Version::new(9, 9, 9),
                }]),
                unread(),
            ),
            false,
        )
        .expect_err("check does not pass an unread source");
        assert_eq!(pin_err.to_string(), "unverified: cargo");

        // Pins at this binary beside an unread Cargo source: `upgrade` would
        // stop, so the schema note must not call the pin settled.
        let at_binary = super::settle(
            Ok(vec![super::FoundPin {
                source: PathBuf::from(".mise.toml"),
                version: super::binary_version(),
            }]),
            unread(),
        );
        assert_eq!(super::standing(&at_binary), super::Standing::Unsettled);
        let settled = super::settle(
            Ok(vec![super::FoundPin {
                source: PathBuf::from(".mise.toml"),
                version: super::binary_version(),
            }]),
            Ok(None),
        );
        assert_eq!(super::standing(&settled), super::Standing::Settled);
    }

    #[test]
    #[cfg(unix)]
    fn a_symlink_to_an_existing_pin_file_is_followed() {
        let root = scratch("linked-pin-files");
        std::fs::create_dir_all(root.join("real")).unwrap();
        std::fs::write(
            root.join("real/mise.toml"),
            "[tools]\n\"github:oakoss/oakum\" = \"0.9.0\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("real/package.json"),
            r#"{"devDependencies":{"@oakoss/oakum":"0.9.0"}}"#,
        )
        .unwrap();
        std::os::unix::fs::symlink("real/mise.toml", root.join(".mise.toml")).unwrap();
        std::os::unix::fs::symlink("real/package.json", root.join("package.json")).unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let pins = collect_pins(&dir).unwrap();
        let sources: Vec<_> = pins.iter().map(|pin| pin.source.clone()).collect();
        assert_eq!(
            sources,
            [PathBuf::from("package.json"), PathBuf::from(".mise.toml")]
        );
    }

    #[test]
    fn invalid_composite_action_yaml_is_unverified() {
        let root = scratch("composite-bad-yaml");
        matching_workflow(&root);
        std::fs::create_dir_all(root.join(".github/actions/broken")).unwrap();
        std::fs::write(root.join(".github/actions/broken/action.yml"), "runs: [\n").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("invalid composite yaml");
        assert!(err.to_string().contains("not valid YAML"), "{err}");
        assert!(
            err.to_string()
                .contains(".github/actions/broken/action.yml"),
            "{err}"
        );
    }

    #[test]
    fn yaml_named_directory_is_unverified() {
        let root = scratch("yaml-dir");
        matching_workflow(&root);
        std::fs::create_dir_all(root.join(".github/workflows/ci.yml")).unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("yaml directory");
        assert!(err.to_string().contains("not a file"), "{err}");
    }

    #[test]
    fn package_json_non_object_root_is_unverified() {
        let root = scratch("pkg-array");
        matching_workflow(&root);
        std::fs::write(root.join("package.json"), "[]").unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("array root");
        assert!(err.to_string().contains("not a JSON object"), "{err}");
    }

    #[test]
    fn package_json_non_object_section_is_unverified() {
        let root = scratch("pkg-section-string");
        matching_workflow(&root);
        std::fs::write(
            root.join("package.json"),
            r#"{"devDependencies":"oakum@latest"}"#,
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).expect_err("section string");
        assert!(err.to_string().contains("not an object"), "{err}");
    }

    #[test]
    fn a_spec_named_like_a_parse_failure_is_reported_as_a_spec() {
        for spec in ["unversioned", "unparseable-yaml", "with-not-object"] {
            let err = push_yaml_pins(
                Path::new(".github/workflows/ci.yml"),
                &format!("run: npm i -g @oakoss/oakum@{spec}\n"),
                &mut Vec::new(),
            )
            .expect_err(spec)
            .to_string();
            assert!(
                err.contains(&format!(
                    "pins oakum as `{spec}`, which is not an exact version"
                )),
                "{err}"
            );
        }
    }

    #[test]
    fn package_json_scoped_pin_is_collected() {
        let root = scratch("npm-scoped");
        std::fs::write(
            root.join("package.json"),
            r#"{"devDependencies":{"@oakoss/oakum":"0.4.2"}}"#,
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let pins = collect_pins(&dir).unwrap();
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].version, Version::parse("0.4.2").unwrap());
    }

    #[test]
    fn package_json_npm_alias_spec_is_not_an_exact_pin() {
        let root = scratch("npm-alias");
        std::fs::write(
            root.join("package.json"),
            r#"{"devDependencies":{"oakum":"npm:@oakoss/oakum@0.4.2"}}"#,
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).unwrap_err();
        assert!(
            err.to_string().contains(
                "pins `oakum` as `npm:@oakoss/oakum@0.4.2`, which is not an exact version"
            ),
            "{err}"
        );
    }

    #[test]
    fn package_json_scoped_and_bare_names_must_agree() {
        let root = scratch("npm-both");
        std::fs::write(
            root.join("package.json"),
            r#"{"devDependencies":{"@oakoss/oakum":"0.4.2","oakum":"0.4.3"}}"#,
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let err = collect_pins(&dir).unwrap_err();
        assert!(
            err.to_string()
                .contains("pins `@oakoss/oakum` as `0.4.2` and `oakum` as `0.4.3`"),
            "{err}"
        );
    }

    #[test]
    fn mise_github_backend_is_a_pin() {
        let root = scratch("mise-github");
        std::fs::write(
            root.join(".mise.toml"),
            "[tools]\n\"github:oakoss/oakum\" = \"0.4.2\"\n",
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let pins = collect_pins(&dir).unwrap();
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].version, Version::parse("0.4.2").unwrap());
    }

    #[test]
    fn mise_npm_backend_is_a_pin() {
        let root = scratch("mise-npm");
        std::fs::write(
            root.join(".mise.toml"),
            "[tools]\n\"npm:@oakoss/oakum\" = \"0.4.2\"\n",
        )
        .unwrap();
        let dir = Dir::open_ambient_dir(&root, cap_std::ambient_authority()).unwrap();
        let pins = collect_pins(&dir).unwrap();
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].version, Version::parse("0.4.2").unwrap());
    }
}
