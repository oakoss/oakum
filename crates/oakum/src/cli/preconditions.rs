//! Shared readiness path (ADR-0020). Reports drift and names the fix; never
//! applies it (ADR-0003).

use super::config::{enforce_tool_version, load_config};
use super::install_pin;
use super::repository::{self, Repository};
use super::tags::{self, CommitTags};
use super::{add, CliError};

pub(super) fn run() -> Result<(), Box<dyn std::error::Error>> {
    let repo = repository::discover()?;
    evaluate(&repo)
}

fn evaluate(repo: &Repository) -> Result<(), Box<dyn std::error::Error>> {
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
