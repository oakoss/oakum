//! `oakum init`: write oakum's three files and print everything else (ADR-0003 / ADR-0023).
//!
//! Version gate first. Detect foreign tools before any write. `--interactive`
//! is opt-in over `--versioning` and never auto-detects a terminal.

use std::io::{self, IsTerminal, Write};
use std::path::Path;

use cap_std::fs::Dir;
use clap::{Args, ValueEnum};
use oakum::changeset::instruction_occupants;
use oakum::config;
use oakum::discover::{discover_cargo, discover_pnpm, pnpm_version, DiscoverError};
use oakum::plan::Versioning;
use semver::Version;

use super::ci::VERSION_BRANCH;
use super::config::{enforce_tool_version, read_config_source, LoadedConfig};
use super::detect_tools;
use super::fs::{write_file_exclusive, write_file_via_rename};
use super::github;
use super::repository;
use super::CliError;

const CONFIG_REL: &str = ".changeset/_config.toml";
pub(super) const SCHEMA_REL: &str = ".changeset/_schema.json";
pub(super) const README_REL: &str = ".changeset/README.md";
const README: &str = include_str!("changeset-readme.md");

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum VersioningArg {
    #[value(name = "zero-major")]
    ZeroMajor,
    Semver,
}

impl VersioningArg {
    fn to_versioning(self) -> Versioning {
        match self {
            Self::ZeroMajor => Versioning::ZeroMajor,
            Self::Semver => Versioning::Semver,
        }
    }
}

#[derive(Debug, Args)]
pub(super) struct InitArgs {
    /// Version policy written into `_config.toml`. Default `zero-major`.
    #[arg(long, value_enum)]
    versioning: Option<VersioningArg>,
    /// Read release intent from bump files. Default `true`.
    #[arg(
        long,
        num_args = 0..=1,
        default_missing_value = "true",
        value_parser = clap::builder::BoolishValueParser::new()
    )]
    change_files: Option<bool>,
    /// Read release intent from conventional commits. Default `true`.
    #[arg(
        long,
        num_args = 0..=1,
        default_missing_value = "true",
        value_parser = clap::builder::BoolishValueParser::new()
    )]
    conventional_commits: Option<bool>,
    /// Guided prompts. Exits non-zero when stdin is not a terminal.
    #[arg(long)]
    interactive: bool,
}

struct ResolvedInit {
    change_files: bool,
    conventional_commits: bool,
    versioning: VersioningArg,
}

pub(super) fn run(args: &InitArgs) -> Result<(), Box<dyn std::error::Error>> {
    let repo = repository::discover()?;
    if let Some(source) = read_config_source(&repo)? {
        already_initialized(&repo, &source, args)?;
        refuse_interactive_without_tty(args.interactive)?;
        println!("already initialized");
        return Ok(());
    }
    refuse_interactive_without_tty(args.interactive)?;

    let report = detect_tools::scan(repo.dir())?;
    if !report.errors.is_empty() {
        for hit in &report.detections {
            println!("{}\t{}", hit.tool().name(), hit.evidence());
        }
        let joined = report
            .errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        return Err(Box::new(CliError::unverified(format!(
            "unverified: {joined}"
        ))));
    }
    if !report.detections.is_empty() {
        for hit in &report.detections {
            println!("{}\t{}", hit.tool().name(), hit.evidence());
        }
        return Err(Box::new(CliError::new("run oakum migrate")));
    }

    report_instruction_files(repo.dir())?;
    let packages = refuse_stray_workspace(&repo)?;

    let settings = resolve_init_settings(args)?;

    let binary = binary_version()?;
    let pins = WorkflowPins::lookup(repo.ambient_path()?)?;
    ensure_changeset_dir(repo.dir())?;
    let created = write_owned_files(
        repo.dir(),
        &binary,
        settings.change_files,
        settings.conventional_commits,
        settings.versioning.to_versioning(),
    )?;

    print_workflow_and_footer(&binary, &pins, &created.written);
    match packages {
        0 => println!("no packages found"),
        n => println!("{n} package(s) found"),
    }
    Ok(())
}

fn already_initialized(
    repo: &super::repository::Repository,
    source: &super::config::ConfigSource,
    args: &InitArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let parsed = config::parse(source.text()).map_err(|err| {
        CliError::new(format!(
            "`.changeset/_config.toml` is not a valid oakum config: {err}"
        ))
    })?;
    let loaded = LoadedConfig::from_parsed(repo, parsed)?;
    enforce_tool_version(&loaded)?;
    if let Some(flag) = args.versioning {
        let wanted = flag.to_versioning();
        let have = loaded.versioning();
        if wanted != have {
            return Err(Box::new(CliError::new(format!(
                "`--versioning` is `{wanted}` but `.changeset/_config.toml` has `versioning = \"{have}\"`; change `versioning` in `.changeset/_config.toml` to `{wanted}`"
            ))));
        }
    }
    if let Some(wanted) = args.change_files {
        let have = loaded.change_files();
        if wanted != have {
            return Err(Box::new(CliError::new(format!(
                "`--change-files` is `{wanted}` but `.changeset/_config.toml` has `change-files = {have}`; change `change-files` in `.changeset/_config.toml` to `{wanted}`"
            ))));
        }
    }
    if let Some(wanted) = args.conventional_commits {
        let have = loaded.conventional_commits();
        if wanted != have {
            return Err(Box::new(CliError::new(format!(
                "`--conventional-commits` is `{wanted}` but `.changeset/_config.toml` has `conventional-commits = {have}`; change `conventional-commits` in `.changeset/_config.toml` to `{wanted}`"
            ))));
        }
    }
    Ok(())
}

fn refuse_interactive_without_tty(interactive: bool) -> Result<(), Box<dyn std::error::Error>> {
    if interactive && !io::stdin().is_terminal() {
        return Err(Box::new(CliError::new(
            "`--interactive` needs a terminal; use `--versioning <zero-major|semver>`, \
             `--change-files <true|false>`, and `--conventional-commits <true|false>` instead",
        )));
    }
    Ok(())
}

fn resolve_init_settings(args: &InitArgs) -> Result<ResolvedInit, Box<dyn std::error::Error>> {
    let change_files = if args.interactive && args.change_files.is_none() {
        prompt_yes_no("change-files", true)?
    } else {
        args.change_files.unwrap_or(true)
    };
    let conventional_commits = if args.interactive && args.conventional_commits.is_none() {
        prompt_yes_no("conventional-commits", true)?
    } else {
        args.conventional_commits.unwrap_or(true)
    };
    refuse_both_intent_disabled(change_files, conventional_commits)?;
    let versioning = if args.interactive && args.versioning.is_none() {
        prompt_versioning()?
    } else {
        args.versioning.unwrap_or(VersioningArg::ZeroMajor)
    };
    Ok(ResolvedInit {
        change_files,
        conventional_commits,
        versioning,
    })
}

fn refuse_both_intent_disabled(
    change_files: bool,
    conventional_commits: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if change_files || conventional_commits {
        return Ok(());
    }
    Err(Box::new(CliError::new(
        "both `change-files` and `conventional-commits` are disabled; enable one so the plan has intent to read (ADR-0019 / ADR-0029)",
    )))
}

pub(super) fn write_owned_files(
    dir: &Dir,
    binary: &Version,
    change_files: bool,
    conventional_commits: bool,
    versioning: Versioning,
) -> Result<OwnedWrites, Box<dyn std::error::Error>> {
    let schema_existed = regular_file_exists(dir, SCHEMA_REL)?;
    let readme_existed = regular_file_exists(dir, README_REL)?;
    // A README byte-identical to the bundled one is oakum's (a run that
    // stopped before `_config.toml` leaves exactly that), so the uninstall
    // line names it.
    let readme_is_ours = readme_existed && dir.read_to_string(README_REL)? == README;
    // Each line prints as its write lands, so a failure part-way through
    // leaves an accurate record of what changed.
    write_file_via_rename(dir, Path::new(SCHEMA_REL), &config::schema_json())?;
    println!(
        "{} {SCHEMA_REL}",
        if schema_existed {
            "replaced"
        } else {
            "created"
        }
    );
    if !readme_existed {
        write_file_exclusive(dir, Path::new(README_REL), README)?;
        println!("created {README_REL}");
    }
    write_file_exclusive(
        dir,
        Path::new(CONFIG_REL),
        &config_body(binary, change_files, conventional_commits, versioning),
    )?;
    println!("created {CONFIG_REL}");
    let mut written = vec![SCHEMA_REL];
    if !readme_existed || readme_is_ours {
        written.push(README_REL);
    }
    written.push(CONFIG_REL);
    Ok(OwnedWrites { written })
}

/// Every file oakum now owns after [`write_owned_files`], including a
/// `_schema.json` it replaced: what the uninstall line must name.
pub(super) struct OwnedWrites {
    pub(super) written: Vec<&'static str>,
}

/// An owned file `migrate` can put back after it went missing. `_config.toml`
/// carries user settings and is never replaced, so it is not one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RestorableFile {
    Schema,
    Readme,
}

impl RestorableFile {
    const ALL: [Self; 2] = [Self::Schema, Self::Readme];

    pub(super) const fn rel(self) -> &'static str {
        match self {
            Self::Schema => SCHEMA_REL,
            Self::Readme => README_REL,
        }
    }

    fn body(self) -> String {
        match self {
            Self::Schema => config::schema_json(),
            Self::Readme => String::from(README),
        }
    }
}

/// The owned files a migrated repository no longer has.
pub(super) fn missing_owned_files(
    dir: &Dir,
) -> Result<Vec<RestorableFile>, Box<dyn std::error::Error>> {
    let mut missing = Vec::new();
    for file in RestorableFile::ALL {
        if !regular_file_exists(dir, file.rel())? {
            missing.push(file);
        }
    }
    Ok(missing)
}

/// Exclusive, so a file that appeared since the look is refused, not replaced.
pub(super) fn restore_owned_file(dir: &Dir, file: RestorableFile) -> io::Result<()> {
    write_file_exclusive(dir, Path::new(file.rel()), &file.body())
}

/// `a`, `a and b`, or `a, b, and c`; empty in, empty out (every caller lists
/// at least one path).
pub(super) fn list_paths(paths: &[&str]) -> String {
    match paths {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [first, second] => format!("{first} and {second}"),
        [init @ .., last] => format!("{}, and {last}", init.join(", ")),
    }
}

/// The footer line naming what to delete, with every path quoted.
pub(super) fn uninstall_line(owned: &[&str]) -> String {
    let quoted: Vec<String> = owned.iter().map(|path| format!("`{path}`")).collect();
    let quoted: Vec<&str> = quoted.iter().map(String::as_str).collect();
    format!("remove {} to uninstall", list_paths(&quoted))
}

fn config_body(
    binary: &Version,
    change_files: bool,
    conventional_commits: bool,
    versioning: Versioning,
) -> String {
    format!(
        "#:schema ./_schema.json\n\
tool-version = \"{binary}\"\n\
change-files = {change_files}\n\
conventional-commits = {conventional_commits}\n\
versioning = \"{versioning}\"\n"
    )
}

/// Action pins for the printed workflow, looked up at print time because a
/// baked-in major goes stale. `pnpm` is `Some` only for an npm workspace:
/// discovery asks pnpm for the packages, and `ubuntu-latest` does not ship it.
pub(super) struct WorkflowPins {
    checkout: String,
    pnpm: Option<PnpmSetup>,
}

struct PnpmSetup {
    pin: String,
    /// `pnpm/action-setup` refuses to run with neither a `version` input nor a
    /// `packageManager` (or `devEngines.packageManager`) field, and refuses a
    /// `version` input that disagrees with `packageManager`. Set only when
    /// `package.json` declares neither, from the pnpm that ran discovery.
    version: Option<String>,
}

impl WorkflowPins {
    pub(super) fn lookup(repo: &Path) -> Result<Self, CliError> {
        let checkout = github::latest_release_tag("actions", "checkout").map_err(CliError::from)?;
        let pnpm = if npm_workspace(repo) {
            let pin = github::latest_release_tag("pnpm", "action-setup").map_err(CliError::from)?;
            let version = if declares_package_manager(repo)? {
                None
            } else {
                Some(pnpm_version(repo).map_err(|err| {
                    CliError::unverified(format!(
                        "unverified: pnpm version for the workflow: {err}; declare `packageManager` (`pnpm@<version>`) in package.json so the workflow needs no version input, or fix pnpm on PATH"
                    ))
                })?)
            };
            Some(PnpmSetup { pin, version })
        } else {
            None
        };
        Ok(Self { checkout, pnpm })
    }

    /// `cargo-binstall` is not on `ubuntu-latest`; npm is. An npm workspace
    /// installs through the channel it already uses, and `check` reads the
    /// versioned line as its pin.
    fn install_step(&self, binary: &Version) -> String {
        if self.pnpm.is_some() {
            format!("      - run: npm i -g @oakoss/oakum@{binary}\n")
        } else {
            format!("      - run: cargo binstall --no-confirm oakum@{binary}\n")
        }
    }

    fn setup_steps(&self) -> String {
        match &self.pnpm {
            Some(PnpmSetup { pin, version: None }) => {
                format!("      - uses: pnpm/action-setup@{pin}\n")
            }
            Some(PnpmSetup {
                pin,
                version: Some(version),
            }) => format!(
                "      - uses: pnpm/action-setup@{pin}\n        with:\n          version: {version}\n"
            ),
            None => String::new(),
        }
    }
}

fn npm_workspace(repo: &Path) -> bool {
    repo.join("package.json").is_file() || repo.join("pnpm-workspace.yaml").is_file()
}

/// Whether the root `package.json` declares a pnpm version the way
/// `pnpm/action-setup` reads one; anything else makes the action demand its
/// `version` input. A workspace declared only by `pnpm-workspace.yaml` has no
/// manifest to read.
fn declares_package_manager(repo: &Path) -> Result<bool, CliError> {
    let path = repo.join("package.json");
    if !path.is_file() {
        return Ok(false);
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|err| CliError::unverified(format!("unverified: read `package.json`: {err}")))?;
    let manifest: serde_json::Value = serde_json::from_str(&text).map_err(|err| {
        CliError::unverified(format!(
            "unverified: `package.json` is not valid JSON: {err}"
        ))
    })?;
    Ok(declares_pnpm(&manifest))
}

fn declares_pnpm(manifest: &serde_json::Value) -> bool {
    let top_level = manifest
        .get("packageManager")
        .and_then(serde_json::Value::as_str)
        .and_then(|spec| spec.strip_prefix("pnpm@"))
        .is_some_and(|version| !version.split('+').next().unwrap_or("").is_empty());
    let dev_engines = manifest
        .get("devEngines")
        .and_then(|engines| engines.get("packageManager"))
        .is_some_and(|pm| {
            pm.get("name").and_then(serde_json::Value::as_str) == Some("pnpm")
                && pm
                    .get("version")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|version| !version.is_empty())
        });
    top_level || dev_engines
}

pub(super) fn print_workflow_and_footer(binary: &Version, pins: &WorkflowPins, owned: &[&str]) {
    let checkout = &pins.checkout;
    let setup = pins.setup_steps();
    let install = pins.install_step(binary);
    println!(
        "\
workflow (paste into `.github/workflows/`; oakum does not write it):
name: oakum
on:
  pull_request:
  push:
jobs:
  check:
    if: github.event_name == 'pull_request'
    runs-on: ubuntu-latest
    permissions:
      contents: read
      pull-requests: write
    steps:
      - uses: actions/checkout@{checkout}
        with:
          fetch-depth: 0
{setup}{install}      - run: oakum check --strict
        if: github.head_ref != '{VERSION_BRANCH}'
      - run: oakum ci pr-status
        if: success() || failure()
        continue-on-error: true
        env:
          GITHUB_TOKEN: ${{{{ secrets.GITHUB_TOKEN }}}}
  version:
    if: github.event_name == 'push' && github.ref == format('refs/heads/{{0}}', github.event.repository.default_branch)
    runs-on: ubuntu-latest
    permissions:
      contents: write
      pull-requests: write
    steps:
      - uses: actions/checkout@{checkout}
        with:
          fetch-depth: 0
{setup}{install}      - run: oakum ci version-pr
        env:
          GITHUB_TOKEN: ${{{{ secrets.GITHUB_TOKEN }}}}
  release:
    if: github.event_name == 'push' && github.ref == format('refs/heads/{{0}}', github.event.repository.default_branch)
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - uses: actions/checkout@{checkout}
        with:
          fetch-depth: 0
{setup}{install}      - run: |
          git config user.name \"github-actions[bot]\"
          git config user.email \"41898282+github-actions[bot]@users.noreply.github.com\"
      - run: oakum release
        env:
          GITHUB_TOKEN: ${{{{ secrets.GITHUB_TOKEN }}}}"
    );
    println!("{}", uninstall_line(owned));
    println!("`oakum init --interactive` is a guided wizard over these flags");
}

fn report_instruction_files(dir: &Dir) -> Result<(), Box<dyn std::error::Error>> {
    let names = changeset_file_names(dir)?;
    for occupant in instruction_occupants(names.iter().map(String::as_str)) {
        if let Some(message) = occupant.init_message() {
            println!("{message}");
        }
    }
    Ok(())
}

pub(super) fn changeset_file_names(dir: &Dir) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let entries = match dir.read_dir(".changeset") {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(Box::new(CliError::new(format!(
                "failed to read `.changeset/`: {err}"
            ))));
        }
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|err| CliError::new(format!("failed to read `.changeset/`: {err}")))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(Box::new(CliError::new(
                "a path under `.changeset/` is not valid UTF-8",
            )));
        };
        if name == "." || name == ".." {
            continue;
        }
        let meta = entry.metadata().map_err(|err| {
            CliError::new(format!("failed to inspect `.changeset/{name}`: {err}"))
        })?;
        if meta.is_file() {
            names.push(name.to_string());
        }
    }
    Ok(names)
}

pub(super) fn ensure_changeset_dir(dir: &Dir) -> Result<(), Box<dyn std::error::Error>> {
    match dir.create_dir(".changeset") {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
            let meta = dir
                .metadata(".changeset")
                .map_err(|err| CliError::new(format!("failed to inspect `.changeset`: {err}")))?;
            if meta.is_dir() {
                Ok(())
            } else {
                Err(Box::new(CliError::new(
                    "`.changeset` exists and is not a directory",
                )))
            }
        }
        Err(err) => Err(Box::new(CliError::new(format!(
            "failed to create `.changeset/`: {err}"
        )))),
    }
}

pub(super) fn regular_file_exists(
    dir: &Dir,
    path: &str,
) -> Result<bool, Box<dyn std::error::Error>> {
    match dir.symlink_metadata(path) {
        Ok(meta) if meta.is_file() => Ok(true),
        Ok(meta) if meta.file_type().is_symlink() => Err(Box::new(CliError::new(format!(
            "`{path}` is a symlink; replace it with a regular file or remove it so oakum can write its own"
        )))),
        Ok(_) => Err(Box::new(CliError::new(format!(
            "`{path}` exists and is not a regular file"
        )))),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(Box::new(CliError::new(format!(
            "failed to inspect `{path}`: {err}"
        )))),
    }
}

fn refuse_stray_workspace(
    repo: &repository::Repository,
) -> Result<usize, Box<dyn std::error::Error>> {
    let path = repo.ambient_path()?;
    let count = package_count(path)?;
    let _ = repo.ambient_path()?;
    Ok(count)
}

fn package_count(repo: &Path) -> Result<usize, Box<dyn std::error::Error>> {
    let mut count = 0;
    if repo.join("Cargo.toml").is_file() {
        count += workspace_len(discover_cargo(repo, repo))?;
    }
    if npm_workspace(repo) {
        count += workspace_len(discover_pnpm(repo, repo))?;
    }
    Ok(count)
}

fn workspace_len(
    result: Result<oakum::plan::Workspace, DiscoverError>,
) -> Result<usize, Box<dyn std::error::Error>> {
    match result {
        Ok(workspace) => Ok(workspace.packages().count()),
        Err(err @ DiscoverError::WorkspaceRootOutsideRepository { .. }) => {
            Err(Box::new(CliError::new(format!(
                "refusing to init: {err} (discovery would describe a different repository)"
            ))))
        }
        Err(err) => Err(Box::new(CliError::new(err.to_string()))),
    }
}

fn prompt_yes_no(name: &str, default: bool) -> Result<bool, Box<dyn std::error::Error>> {
    let default_label = if default { "Y" } else { "y" };
    let alt = if default { "n" } else { "Y" };
    let default_word = if default { "yes" } else { "no" };
    eprint!("{name} [{default_label}/{alt}] (default {default_word}): ");
    io::stderr().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    parse_yes_no(name, line.trim(), default)
        .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
}

fn parse_yes_no(name: &str, answer: &str, default: bool) -> Result<bool, CliError> {
    match answer {
        "" => Ok(default),
        "y" | "yes" | "Y" | "Yes" | "YES" | "true" => Ok(true),
        "n" | "no" | "N" | "No" | "NO" | "false" => Ok(false),
        other => Err(CliError::new(format!(
            "unknown answer `{other}` for `{name}`; use yes or no"
        ))),
    }
}

fn prompt_versioning() -> Result<VersioningArg, Box<dyn std::error::Error>> {
    eprint!("versioning [zero-major/semver] (default zero-major): ");
    io::stderr().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    parse_versioning(line.trim()).map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
}

fn parse_versioning(answer: &str) -> Result<VersioningArg, CliError> {
    match answer {
        "" | "zero-major" => Ok(VersioningArg::ZeroMajor),
        "semver" => Ok(VersioningArg::Semver),
        other => Err(CliError::new(format!(
            "unknown versioning `{other}`; use zero-major or semver"
        ))),
    }
}

pub(super) fn binary_version() -> Result<Version, Box<dyn std::error::Error>> {
    env!("CARGO_PKG_VERSION").parse::<Version>().map_err(|err| {
        Box::new(CliError::new(format!(
            "this binary reports a non-semver version: {err}"
        )))
        .into()
    })
}

#[cfg(test)]
mod prompts {
    use super::{parse_versioning, parse_yes_no, VersioningArg};

    #[test]
    fn yes_no_defaults_and_variants() {
        assert!(parse_yes_no("change-files", "", true).expect("default yes"));
        assert!(!parse_yes_no("change-files", "", false).expect("default no"));
        assert!(parse_yes_no("change-files", "y", false).expect("y"));
        assert!(!parse_yes_no("change-files", "no", true).expect("no"));
        assert!(parse_yes_no("change-files", "true", false).expect("true"));
        assert!(!parse_yes_no("change-files", "false", true).expect("false"));
    }

    #[test]
    fn yes_no_rejects_unknown() {
        let err = parse_yes_no("change-files", "maybe", true).expect_err("unknown");
        assert!(err.to_string().contains("maybe"));
    }

    #[test]
    fn versioning_defaults_and_variants() {
        assert_eq!(
            parse_versioning("").expect("default"),
            VersioningArg::ZeroMajor
        );
        assert_eq!(
            parse_versioning("semver").expect("semver"),
            VersioningArg::Semver
        );
    }

    #[test]
    fn versioning_rejects_unknown() {
        let err = parse_versioning("calver").expect_err("unknown");
        assert!(err.to_string().contains("calver"));
    }
}

#[cfg(all(test, unix))]
mod identity {
    use std::fs;
    use std::path::Path;

    use crate::cli::repository::discover_from;
    use crate::test_fixture::Fixture;

    use super::refuse_stray_workspace;

    fn git_repo(label: &str) -> Fixture {
        let root = Fixture::new("init", label);
        fs::create_dir(root.join(".git")).expect("git marker");
        root
    }

    fn replace_root(root: &Path) {
        let moved = root.with_file_name("moved");
        fs::rename(root, &moved).expect("rename repository");
        fs::create_dir(root).expect("replacement root");
        fs::create_dir(root.join(".git")).expect("replacement git marker");
    }

    #[test]
    fn refuse_stray_workspace_after_root_replacement_fails_closed() {
        let root = git_repo("stray-replaced");
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"original\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .expect("original manifest");
        let repository = discover_from(&root).expect("discover repository");
        replace_root(&root);

        let error = refuse_stray_workspace(&repository)
            .expect_err("empty replacement must not look like no workspace");
        let message = error.to_string();
        assert!(
            message.contains("no longer the directory originally opened"),
            "{message}"
        );
        assert!(!message.contains("nothing to discover"), "{message}");
    }

    #[test]
    fn refuse_stray_workspace_counts_the_original_tree() {
        let root = git_repo("stray-ok");
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"original\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
        )
        .expect("manifest");
        fs::create_dir(root.join("src")).expect("src");
        fs::write(root.join("src/lib.rs"), "").expect("lib");
        let repository = discover_from(&root).expect("discover repository");
        let count = refuse_stray_workspace(&repository).expect("count original tree");
        assert_eq!(count, 1);
    }

    #[test]
    fn declares_pnpm_mirrors_action_setup() {
        use serde_json::json;
        for declared in [
            json!({ "packageManager": "pnpm@10.0.0" }),
            json!({ "packageManager": "pnpm@10.0.0+sha512.abc" }),
            json!({ "devEngines": { "packageManager": { "name": "pnpm", "version": "10" } } }),
        ] {
            assert!(super::declares_pnpm(&declared), "{declared}");
        }
        for undeclared in [
            json!({}),
            json!({ "packageManager": "" }),
            json!({ "packageManager": "pnpm" }),
            json!({ "packageManager": "pnpm@" }),
            json!({ "packageManager": "npm@10" }),
            json!({ "packageManager": 42 }),
            json!({ "devEngines": { "packageManager": null } }),
            json!({ "devEngines": { "packageManager": "" } }),
            json!({ "devEngines": { "packageManager": { "name": "pnpm" } } }),
            json!({ "devEngines": { "packageManager": { "version": "10" } } }),
            json!({ "devEngines": { "packageManager": [{ "name": "pnpm", "version": "10" }] } }),
        ] {
            assert!(!super::declares_pnpm(&undeclared), "{undeclared}");
        }
    }
}

#[cfg(test)]
mod wording {
    use super::{list_paths, uninstall_line};

    #[test]
    fn list_paths_joins_like_prose() {
        assert_eq!(list_paths(&[]), "");
        assert_eq!(list_paths(&["a"]), "a");
        assert_eq!(list_paths(&["a", "b"]), "a and b");
        assert_eq!(list_paths(&["a", "b", "c"]), "a, b, and c");
    }

    #[test]
    fn uninstall_line_quotes_every_path() {
        assert_eq!(
            uninstall_line(&[".changeset/_schema.json", ".changeset/_config.toml"]),
            "remove `.changeset/_schema.json` and `.changeset/_config.toml` to uninstall"
        );
    }
}
