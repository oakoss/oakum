//! Shared readiness path (ADR-0020). Reports drift and names the fix; never
//! applies it (ADR-0003).

use clap::Args;

use super::config::{enforce_tool_version, load_config, PlanIntentSource};
use super::coverage;
use super::install_pin;
use super::intent::load_plan_bump_files;
use super::repository::{self, Repository};
use super::tags::{self, CommitTags};
use super::{add, CliError};

#[derive(Debug, Default, Args)]
pub(super) struct CheckArgs {
    /// Fail when a changed package is not named by the enabled intent mechanism.
    #[arg(long)]
    strict: bool,
    /// Git ref to diff from (exclusive). Same default as `generate` / `status`.
    #[arg(long, value_name = "REF")]
    from: Option<String>,
}

pub(super) fn run(args: &CheckArgs) -> Result<(), Box<dyn std::error::Error>> {
    let repo = repository::discover()?;
    evaluate_tags(&repo)?;
    evaluate_coverage(&repo, args)
}

pub(super) fn run_tags_only() -> Result<(), Box<dyn std::error::Error>> {
    let repo = repository::discover()?;
    evaluate_tags(&repo)
}

fn evaluate_tags(repo: &Repository) -> Result<(), Box<dyn std::error::Error>> {
    let config = load_config(repo)?;
    enforce_tool_version(&config)?;
    if let Some(expected) = config.tool_version() {
        install_pin::verify(repo.dir(), expected)?;
    }
    let _ = config.plan_intent_source()?;
    let groups = tags::reachable_tags(repo.path())?;
    let workspace = add::discover_workspace(repo.path())?;
    let owned: Vec<Vec<&str>> = groups
        .iter()
        .map(CommitTags::tags)
        .map(|tags| tags.iter().map(String::as_str).collect())
        .collect();
    let slices: Vec<&[&str]> = owned.iter().map(Vec::as_slice).collect();
    let tagged = oakum::tags::current_versions(&slices, &workspace)?;
    let found = oakum::tags::drift(&workspace, &tagged);
    let clobber = oakum::tags::untagged_ahead(&workspace, &tagged);
    if found.is_empty() && clobber.is_empty() {
        return Ok(());
    }
    for item in &found {
        eprintln!(
            "{}: manifest {} is above tagged {}",
            item.id(),
            item.manifest(),
            item.tagged()
        );
    }
    for (id, version) in &clobber {
        eprintln!("{id}: never released, but the manifest is {version}; tag the version you meant");
    }
    Err(Box::new(CliError::new(format!(
        "{} package(s) bumped without a tag",
        found.len() + clobber.len()
    ))))
}

fn evaluate_coverage(
    repo: &Repository,
    args: &CheckArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = load_config(repo)?;
    let workspace = add::discover_workspace(repo.path())?;
    let files = load_plan_bump_files(repo.path(), &workspace, &config, args.from.as_deref())?;
    let uncovered =
        coverage::uncovered_packages(repo.path(), &workspace, &files, args.from.as_deref())?;
    if uncovered.is_empty() {
        return Ok(());
    }
    let hint = match config.plan_intent_source()? {
        PlanIntentSource::ChangeFiles => {
            "add a bump file (or `none` / empty frontmatter under --strict)"
        }
        PlanIntentSource::CommitsOnly => {
            "name the package in a conventional commit (or a path that maps to it)"
        }
    };
    for id in &uncovered {
        eprintln!("{id}: changed with no covering intent; {hint}");
    }
    if args.strict {
        return Err(Box::new(CliError::new(format!(
            "{} package(s) changed with no covering intent",
            uncovered.len()
        ))));
    }
    Ok(())
}
