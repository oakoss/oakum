//! `oakum init`: write oakum's three files and print everything else (ADR-0003 / ADR-0023).
//!
//! Version gate first. Detect foreign tools before any write. `--interactive`
//! is opt-in over `--versioning` and never auto-detects a terminal.

use std::io::{self, IsTerminal};
use std::path::Path;

use cap_std::fs::Dir;
use clap::{Args, ValueEnum};
use oakum::changeset::instruction_occupants;
use oakum::config;
use oakum::discover::{discover_cargo, discover_pnpm, DiscoverError};
use oakum::plan::Versioning;
use semver::Version;

use super::config::{enforce_tool_version, read_config_source, LoadedConfig, ALL_PRIVATE_GUIDANCE};
use super::detect_tools;
use super::fs::report_stray_staging;
use super::owned_files::{write_owned_files, ConfigSettings, OwnedPlan, PrivatePackages, Records};
use super::repository;
use super::workflow::{npm_workspace, workflow_text, WorkflowPins};
use super::CliError;
use super::{ask, say_err, say_out};

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
    report_stray_staging(repo.dir())?;
    if let Some(source) = read_config_source(&repo)? {
        already_initialized(&repo, &source, args)?;
        refuse_interactive_without_tty(args.interactive)?;
        say_out("already initialized");
        return Ok(());
    }
    refuse_interactive_without_tty(args.interactive)?;

    let report = detect_tools::scan(repo.dir())?;
    if !report.errors.is_empty() {
        for hit in &report.detections {
            say_out(&format!("{}\t{}", hit.tool().name(), hit.evidence()));
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
            say_out(&format!("{}\t{}", hit.tool().name(), hit.evidence()));
        }
        return Err(Box::new(CliError::new("run oakum migrate")));
    }

    report_instruction_files(repo.dir())?;
    let packages = refuse_stray_workspace(&repo)?;

    let settings = resolve_init_settings(args)?;

    let binary = binary_version()?;
    let pins = WorkflowPins::lookup(repo.dir(), repo.ambient_path()?)?;
    ensure_changeset_dir(repo.dir())?;
    let plan = OwnedPlan::probe(repo.dir())?;
    let mut records = Records::default();
    let created = write_owned_files(
        repo.dir(),
        plan,
        &binary,
        ConfigSettings {
            change_files: settings.change_files,
            conventional_commits: settings.conventional_commits,
            versioning: settings.versioning.to_versioning(),
            private_packages: PrivatePackages::default(),
            tag_format: None,
            commit_message: None,
        },
        &mut records,
    )
    .map_err(|err| records.abandon(err))?;
    print_workflow_and_footer(&binary, &pins, &created.written, &mut records);
    match packages.total {
        0 => say_out("no packages found"),
        n => say_out(&format!("{n} package(s) found")),
    }
    // `check` refuses this state. Saying so here, where the config was just
    // written, beats letting the next command be the one to mention it.
    if packages.all_private() {
        say_err(ALL_PRIVATE_GUIDANCE);
    }
    records.finish()?;
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

/// The workflow is written nowhere else, so it is the deliverable, recorded
/// beside the files that were written; the caller's [`Records::finish`] says
/// whether it arrived.
pub(super) fn print_workflow_and_footer(
    binary: &Version,
    pins: &WorkflowPins,
    owned: &[&str],
    records: &mut Records,
) {
    records.deliver(
        "the workflow to paste",
        &format!(
            "workflow (paste into `.github/workflows/`; oakum does not write it):\n{}",
            workflow_text(binary, pins)
        ),
    );
    records.deliver("the uninstall line", &uninstall_line(owned));
    say_out("`oakum init --interactive` is a guided wizard over these flags");
}

fn report_instruction_files(dir: &Dir) -> Result<(), Box<dyn std::error::Error>> {
    let names = changeset_file_names(dir)?;
    for occupant in instruction_occupants(names.iter().map(String::as_str)) {
        if let Some(message) = occupant.init_message() {
            say_out(&message);
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
    // `read_dir` yields filesystem order, so an unsorted listing prints a
    // different plan on a different machine for the same repository.
    names.sort();
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

fn refuse_stray_workspace(
    repo: &repository::Repository,
) -> Result<PackageTally, Box<dyn std::error::Error>> {
    let path = repo.ambient_path()?;
    let count = package_count(path)?;
    let _ = repo.ambient_path()?;
    Ok(count)
}

/// How many packages discovery found, and how many a registry would accept.
/// `init` writes `private-packages` off, so a workspace with none of the second
/// is one every later command reads as managing nothing.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
struct PackageTally {
    total: usize,
    publishable: usize,
}

impl PackageTally {
    fn add(self, other: Self) -> Self {
        Self {
            total: self.total + other.total,
            publishable: self.publishable + other.publishable,
        }
    }

    const fn all_private(self) -> bool {
        self.total > 0 && self.publishable == 0
    }
}

fn package_count(repo: &Path) -> Result<PackageTally, Box<dyn std::error::Error>> {
    let mut tally = PackageTally::default();
    if repo.join("Cargo.toml").is_file() {
        tally = tally.add(workspace_len(discover_cargo(repo, repo))?);
    }
    if npm_workspace(repo) {
        tally = tally.add(workspace_len(discover_pnpm(repo, repo))?);
    }
    Ok(tally)
}

fn workspace_len(
    result: Result<oakum::plan::Workspace, DiscoverError>,
) -> Result<PackageTally, Box<dyn std::error::Error>> {
    match result {
        Ok(workspace) => Ok(PackageTally {
            total: workspace.packages().count(),
            publishable: workspace.packages().filter(|p| p.publishable()).count(),
        }),
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
    ask(&format!(
        "{name} [{default_label}/{alt}] (default {default_word}): "
    ))?;
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
    ask("versioning [zero-major/semver] (default zero-major): ")?;
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
        assert_eq!(count.total, 1);
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
