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
use oakum::discover::{discover_cargo, discover_pnpm, DiscoverError};
use oakum::plan::Versioning;
use semver::Version;

use super::config::{
    enforce_tool_version, read_config_source, write_file_exclusive, write_file_via_rename,
    LoadedConfig,
};
use super::detect_tools;
use super::repository;
use super::CliError;

const CONFIG_REL: &str = ".changeset/_config.toml";
const SCHEMA_REL: &str = ".changeset/_schema.json";
const README_REL: &str = ".changeset/README.md";

const README: &str = "\
# Changesets

Bump files live in this directory. Each is a markdown file whose front matter
names packages and bump levels:

```markdown
---
my-package: minor
---

What a reader should do differently after this release.
```

Or run `oakum add --packages \"my-package:minor\" --message \"…\"`.

`README.md` (any case), `AGENTS.md`, `CLAUDE.md`, and `GEMINI.md` (exact names)
are not bump files. Other `.md` files here are. knope has no skip list — do not
run knope against this directory.
";

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
    /// Guided prompts. Exits non-zero when stdin is not a terminal.
    #[arg(long)]
    interactive: bool,
}

pub(super) fn run(args: &InitArgs) -> Result<(), Box<dyn std::error::Error>> {
    let repo = repository::discover()?;
    if let Some(source) = read_config_source(&repo)? {
        already_initialized(&repo, &source, args.versioning)?;
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
    let packages = refuse_stray_workspace(repo.path())?;

    let versioning = if args.interactive && args.versioning.is_none() {
        prompt_versioning()?
    } else {
        args.versioning.unwrap_or(VersioningArg::ZeroMajor)
    };

    let binary = binary_version()?;
    ensure_changeset_dir(repo.dir())?;
    let created = write_owned_files(repo.dir(), &binary, versioning.to_versioning())?;

    for path in &created {
        println!("created {path}");
    }
    print_workflow_and_footer(&binary);
    match packages {
        0 => println!("no packages found"),
        n => println!("{n} package(s) found"),
    }
    Ok(())
}

fn already_initialized(
    repo: &super::repository::Repository,
    source: &super::config::ConfigSource,
    versioning_flag: Option<VersioningArg>,
) -> Result<(), Box<dyn std::error::Error>> {
    let parsed = config::parse(source.text()).map_err(|err| {
        CliError::new(format!(
            "`.changeset/_config.toml` is not a valid oakum config: {err}"
        ))
    })?;
    let loaded = LoadedConfig::from_parsed(repo, parsed)?;
    enforce_tool_version(&loaded)?;
    if let Some(flag) = versioning_flag {
        let wanted = flag.to_versioning();
        let have = loaded.versioning();
        if wanted != have {
            return Err(Box::new(CliError::new(format!(
                "`--versioning` is `{wanted}` but `.changeset/_config.toml` has `versioning = \"{have}\"`; change `versioning` in `.changeset/_config.toml` to `{wanted}`"
            ))));
        }
    }
    Ok(())
}

fn refuse_interactive_without_tty(interactive: bool) -> Result<(), Box<dyn std::error::Error>> {
    if interactive && !io::stdin().is_terminal() {
        return Err(Box::new(CliError::new(
            "`--interactive` needs a terminal; use `--versioning <zero-major|semver>` instead",
        )));
    }
    Ok(())
}

pub(super) fn write_owned_files(
    dir: &Dir,
    binary: &Version,
    versioning: Versioning,
) -> Result<Vec<&'static str>, Box<dyn std::error::Error>> {
    let mut created = Vec::new();
    let schema_existed = regular_file_exists(dir, SCHEMA_REL)?;
    write_file_via_rename(dir, Path::new(SCHEMA_REL), &config::schema_json())?;
    if !schema_existed {
        created.push(SCHEMA_REL);
    }
    if !regular_file_exists(dir, README_REL)? {
        write_file_exclusive(dir, Path::new(README_REL), README)?;
        created.push(README_REL);
    }
    write_file_exclusive(dir, Path::new(CONFIG_REL), &config_body(binary, versioning))?;
    created.push(CONFIG_REL);
    Ok(created)
}

fn config_body(binary: &Version, versioning: Versioning) -> String {
    format!(
        "#:schema ./_schema.json\n\
tool-version = \"{binary}\"\n\
change-files = true\n\
conventional-commits = true\n\
versioning = \"{versioning}\"\n"
    )
}

pub(super) fn print_workflow_and_footer(binary: &Version) {
    println!(
        "\
workflow (paste into `.github/workflows/`; oakum does not write it):
name: oakum
on:
  pull_request:
  push:
    branches: [main]
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: cargo binstall --no-confirm oakum@{binary}
      - run: oakum check
remove `.changeset/_config.toml`, `.changeset/_schema.json`, and `.changeset/README.md` to uninstall
`oakum init --interactive` is a guided wizard over these flags"
    );
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

fn regular_file_exists(dir: &Dir, path: &str) -> Result<bool, Box<dyn std::error::Error>> {
    match dir.symlink_metadata(path) {
        Ok(meta) if meta.is_file() => Ok(true),
        Ok(_) => Err(Box::new(CliError::new(format!(
            "`{path}` exists and is not a regular file"
        )))),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(Box::new(CliError::new(format!(
            "failed to inspect `{path}`: {err}"
        )))),
    }
}

fn refuse_stray_workspace(repo: &Path) -> Result<usize, Box<dyn std::error::Error>> {
    package_count(repo)
}

fn package_count(repo: &Path) -> Result<usize, Box<dyn std::error::Error>> {
    let mut count = 0;
    if repo.join("Cargo.toml").is_file() {
        count += workspace_len(discover_cargo(repo, repo))?;
    }
    if repo.join("package.json").is_file() || repo.join("pnpm-workspace.yaml").is_file() {
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

fn prompt_versioning() -> Result<VersioningArg, Box<dyn std::error::Error>> {
    eprint!("versioning [zero-major/semver] (default zero-major): ");
    io::stderr().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    match line.trim() {
        "" | "zero-major" => Ok(VersioningArg::ZeroMajor),
        "semver" => Ok(VersioningArg::Semver),
        other => Err(Box::new(CliError::new(format!(
            "unknown versioning `{other}`; use zero-major or semver"
        )))),
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
