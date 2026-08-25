//! `oakum add`: write one bump file (ADR-0023 / specs/bump-files.md).

use std::fs::{self, OpenOptions};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use clap::Args;

use oakum::changeset::{
    default_stem, parse_packages_list, resolve_package_name, skipped_instruction_name, slugify,
    write, KnopePresence, PackageSpec, PackagesError, UnknownReason, WriteError,
};
use oakum::discover::{discover_cargo, discover_pnpm};
use oakum::plan::{BumpLevel, Package, Workspace};

use super::config::{enforce_tool_version, load_config};
use super::repository;
use super::CliError;

#[derive(Debug, Args)]
pub(super) struct AddArgs {
    /// Comma-separated `name:level` pairs (`core:minor,utils:patch`).
    #[arg(long, value_name = "LIST", allow_hyphen_values = true)]
    packages: Option<String>,

    /// Changelog note body.
    #[arg(long, default_value = "")]
    message: String,

    /// Filename stem (slugified). Defaults to a generated name.
    #[arg(long, value_name = "SLUG")]
    name: Option<String>,

    /// Guided prompts. Exits non-zero when stdin is not a terminal.
    #[arg(long, conflicts_with_all = ["packages", "empty", "none"])]
    interactive: bool,

    /// Write empty frontmatter (intentionally releaseless; ADR-0028).
    #[arg(long, conflicts_with_all = ["packages", "none", "interactive"])]
    empty: bool,

    /// Write `name: none` coverage entries (ADR-0028). Requires `--packages`.
    #[arg(long, conflicts_with_all = ["empty", "interactive"])]
    none: bool,
}

pub(super) fn run(args: AddArgs) -> Result<(), Box<dyn std::error::Error>> {
    if args.interactive {
        if !io::stdin().is_terminal() {
            return Err(Box::new(CliError::new(
                "`--interactive` needs a terminal; use `--packages <list>` (and optionally `--message` / `--name`) instead",
            )));
        }
        return run_interactive(args.message, args.name);
    }

    if args.empty {
        if args.packages.is_some() {
            return Err(Box::new(CliError::new(
                "`--empty` cannot be combined with `--packages`",
            )));
        }
        return write_bump_file(&[], &args.message, args.name.as_deref());
    }

    let Some(packages_text) = args.packages.as_deref() else {
        return Err(Box::new(CliError::new(if args.none {
            "`--none` needs `--packages` with `name:none` pairs"
        } else {
            "`oakum add` needs `--packages <list>`, `--empty`, `--none`, or `--interactive`"
        })));
    };

    let specs = parse_packages_list(packages_text).map_err(|err| packages_cli_error(&err))?;
    if args.none {
        for spec in &specs {
            if spec.level() != BumpLevel::None {
                return Err(Box::new(CliError::new(format!(
                    "`--none` requires every `--packages` entry to use level `none` (got `{}:{}`)",
                    spec.name(),
                    spec.level()
                ))));
            }
        }
    }

    write_bump_file(&specs, &args.message, args.name.as_deref())
}

fn run_interactive(
    message_flag: String,
    name_flag: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let repo = repository::discover()?;
    let config = load_config(&repo)?;
    enforce_tool_version(&config)?;
    let workspace = discover_workspace(repo.path())?;
    let package_names = package_names_sorted(&workspace);

    eprintln!("Packages in this workspace:");
    for name in &package_names {
        eprintln!("  {name}");
    }
    eprint!("Packages as name:level (comma-separated): ");
    io::stderr().flush()?;
    let packages_line = read_line()?;
    let specs = parse_packages_list(&packages_line).map_err(|err| packages_cli_error(&err))?;
    validate_specs(&specs, &workspace)?;

    let message = if message_flag.is_empty() {
        eprint!("Summary (changelog note): ");
        io::stderr().flush()?;
        read_line()?
    } else {
        message_flag
    };

    let name = if let Some(stem) = name_flag {
        Some(stem)
    } else {
        eprint!("Filename stem (empty to generate): ");
        io::stderr().flush()?;
        let line = read_line()?;
        if line.is_empty() {
            None
        } else {
            Some(line)
        }
    };

    write_bump_file_in(repo.path(), &workspace, &specs, &message, name.as_deref())
}

fn write_bump_file(
    specs: &[PackageSpec],
    message: &str,
    name: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let repo = repository::discover()?;
    let config = load_config(&repo)?;
    enforce_tool_version(&config)?;
    let workspace = discover_workspace(repo.path())?;
    write_bump_file_in(repo.path(), &workspace, specs, message, name)
}

pub(super) fn write_bump_file_in(
    repo: &Path,
    workspace: &Workspace,
    specs: &[PackageSpec],
    message: &str,
    name: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    if !specs.is_empty() {
        validate_specs(specs, workspace)?;
    }

    let knope = knope_presence(repo);
    let entries: Vec<(String, BumpLevel)> = specs
        .iter()
        .map(|spec| (String::from(spec.name()), spec.level()))
        .collect();
    let body = write(&entries, message, knope).map_err(|err| write_cli_error(&err))?;

    let stem = match name {
        Some(raw) => slugify(raw),
        None => default_stem(unique_seed()),
    };
    let file_name = format!("{stem}.md");
    if skipped_instruction_name(&file_name) {
        return Err(Box::new(CliError::new(format!(
            "refusing to write `{file_name}`: that name is reserved for instruction files in `.changeset/`"
        ))));
    }

    let dir = ensure_changeset_dir(repo)?;
    let path = dir.join(&file_name);
    write_new_file(&path, body.as_bytes()).map_err(|err| {
        if err.kind() == io::ErrorKind::AlreadyExists {
            Box::new(CliError::new(format!(
                "refusing to overwrite existing bump file `{}`",
                path.display()
            ))) as Box<dyn std::error::Error>
        } else {
            Box::new(err) as Box<dyn std::error::Error>
        }
    })?;
    println!("{}", path.strip_prefix(repo).unwrap_or(&path).display());
    Ok(())
}

fn ensure_changeset_dir(repo: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let canonical_repo = fs::canonicalize(repo)?;
    let dir = repo.join(".changeset");
    match fs::symlink_metadata(&dir) {
        Ok(_) => {
            let canonical_dir = fs::canonicalize(&dir).map_err(|err| {
                Box::new(CliError::new(format!("cannot resolve `.changeset`: {err}")))
                    as Box<dyn std::error::Error>
            })?;
            if !canonical_dir.starts_with(&canonical_repo) {
                return Err(Box::new(CliError::new(format!(
                    "`.changeset` resolves outside the repository (`{}`)",
                    canonical_dir.display()
                ))));
            }
            if !canonical_dir.is_dir() {
                return Err(Box::new(CliError::new(
                    "`.changeset` exists but is not a directory",
                )));
            }
            Ok(canonical_dir)
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(&dir)?;
            let canonical_dir = fs::canonicalize(&dir)?;
            if !canonical_dir.starts_with(&canonical_repo) {
                let _ = fs::remove_dir(&dir);
                return Err(Box::new(CliError::new(format!(
                    "`.changeset` resolves outside the repository (`{}`)",
                    canonical_dir.display()
                ))));
            }
            Ok(canonical_dir)
        }
        Err(err) => Err(Box::new(CliError::new(format!(
            "cannot inspect `.changeset`: {err}"
        )))),
    }
}

fn write_new_file(path: &Path, body: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    if let Err(err) = file.write_all(body).and_then(|()| file.flush()) {
        let _ = fs::remove_file(path);
        return Err(err);
    }
    Ok(())
}

fn validate_specs(
    specs: &[PackageSpec],
    workspace: &Workspace,
) -> Result<(), Box<dyn std::error::Error>> {
    for spec in specs {
        match resolve_package_name(spec.name(), workspace) {
            Ok(_) => {}
            Err(UnknownReason::Missing) => {
                return Err(Box::new(CliError::new(format!(
                    "package `{}` is not in the workspace",
                    spec.name()
                ))));
            }
            Err(UnknownReason::Ambiguous) => {
                return Err(Box::new(CliError::new(format!(
                    "package `{}` matches more than one workspace package",
                    spec.name()
                ))));
            }
        }
    }
    Ok(())
}

pub(super) const NOTHING_TO_DISCOVER: &str =
    "no Cargo.toml or package.json found; nothing to discover";

pub(super) fn discover_workspace(repo: &Path) -> Result<Workspace, Box<dyn std::error::Error>> {
    let mut packages = Vec::new();
    let mut errors = Vec::new();
    let cwd = std::env::current_dir()?;

    let mut cargo_workspace_root = None;
    let mut catalog_file = None;

    let cargo_dir = find_manifest_dir(&cwd, repo, "Cargo.toml");
    if cargo_dir.is_some() || repo.join("Cargo.toml").is_file() {
        let start = cargo_dir.as_deref().unwrap_or(repo);
        match discover_cargo(start, repo) {
            Ok(ws) => {
                cargo_workspace_root = ws.cargo_workspace_root().map(str::to_owned);
                packages.extend(ws.packages().cloned());
            }
            Err(err) => errors.push(format!("cargo: {err}")),
        }
    }

    let pnpm_marker = repo.join("pnpm-workspace.yaml").is_file()
        || repo.join("package.json").is_file()
        || find_manifest_dir(&cwd, repo, "package.json").is_some();
    if pnpm_marker {
        let start = find_manifest_dir(&cwd, repo, "package.json")
            .or_else(|| find_manifest_dir(&cwd, repo, "pnpm-workspace.yaml"))
            .unwrap_or_else(|| repo.to_path_buf());
        match discover_pnpm(&start, repo) {
            Ok(ws) => {
                catalog_file = ws.catalog_file().map(str::to_owned);
                packages.extend(ws.packages().cloned());
            }
            Err(err) => errors.push(format!("pnpm: {err}")),
        }
    }

    if !errors.is_empty() {
        return Err(Box::new(CliError::new(format!(
            "workspace discovery failed ({})",
            errors.join("; ")
        ))));
    }

    workspace_from_discovered(packages, cargo_workspace_root, catalog_file)
}

fn workspace_from_discovered(
    packages: Vec<Package>,
    cargo_workspace_root: Option<String>,
    catalog_file: Option<String>,
) -> Result<Workspace, Box<dyn std::error::Error>> {
    if packages.is_empty() {
        return Err(Box::new(CliError::new(NOTHING_TO_DISCOVER)));
    }

    let mut workspace = Workspace::new(packages).map_err(|err| -> Box<dyn std::error::Error> {
        Box::new(CliError::new(err.to_string()))
    })?;
    if let Some(dir) = cargo_workspace_root {
        workspace = workspace.with_cargo_workspace_root(dir);
    }
    if let Some(path) = catalog_file {
        workspace = workspace.with_catalog_file(path);
    }
    Ok(workspace)
}

pub(super) fn find_manifest_dir(start: &Path, stop: &Path, file_name: &str) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        if dir.join(file_name).is_file() {
            return Some(dir);
        }
        if dir == stop || !dir.pop() {
            return None;
        }
    }
}

pub(super) fn knope_presence(repo: &Path) -> KnopePresence {
    if repo.join("knope.toml").is_file() {
        KnopePresence::Present
    } else {
        KnopePresence::Absent
    }
}

fn package_names_sorted(workspace: &Workspace) -> Vec<String> {
    let mut names: Vec<String> = workspace
        .packages()
        .map(|pkg| pkg.id().name.clone())
        .collect();
    names.sort();
    names.dedup();
    names
}

fn unique_seed() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d
            .as_secs()
            .wrapping_mul(1_000_000_000)
            .wrapping_add(u64::from(d.subsec_nanos())),
        Err(_) => 0,
    }
}

fn read_line() -> Result<String, Box<dyn std::error::Error>> {
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

fn packages_cli_error(err: &PackagesError) -> Box<dyn std::error::Error> {
    Box::new(CliError::new(err.to_string()))
}

fn write_cli_error(err: &WriteError) -> Box<dyn std::error::Error> {
    Box::new(CliError::new(err.to_string()))
}

#[cfg(test)]
mod tests {
    use oakum::plan::{Ecosystem, Package, PackageId, ResolvesDependenciesAt};
    use semver::Version;

    use super::workspace_from_discovered;

    fn pkg(ecosystem: Ecosystem, name: &str) -> Package {
        Package::new(
            PackageId::new(ecosystem, name),
            Version::new(0, 1, 0),
            ResolvesDependenciesAt::Install,
            true,
            vec![],
        )
    }

    #[test]
    fn cargo_and_pnpm_merge_keeps_both_discovery_paths() {
        let workspace = workspace_from_discovered(
            vec![pkg(Ecosystem::Cargo, "core"), pkg(Ecosystem::Npm, "app")],
            Some("rust".into()),
            Some("js/pnpm-workspace.yaml".into()),
        )
        .expect("merge");
        assert_eq!(workspace.cargo_workspace_root(), Some("rust"));
        assert_eq!(workspace.catalog_file(), Some("js/pnpm-workspace.yaml"));
    }

    #[test]
    fn merge_without_paths_leaves_both_none() {
        let workspace = workspace_from_discovered(vec![pkg(Ecosystem::Cargo, "core")], None, None)
            .expect("merge");
        assert_eq!(workspace.cargo_workspace_root(), None);
        assert_eq!(workspace.catalog_file(), None);
    }
}
