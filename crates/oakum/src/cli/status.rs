//! `oakum status`: emit versioned release state, never deliver it (ADR-0016).

use std::fmt::Write;

use clap::Args;

use oakum::plan::{aggregate, compose, CascadeAs, Plan, Workspace};
use oakum::state::{BumpName, EcosystemName, ReleaseSource, ReleaseState, RenderTarget};

use super::add::discover_workspace;
use super::config::{load_config, LoadedConfig};
use super::git::Git;
use super::intent::load_plan_bump_files;
use super::repository;
use super::CliError;

#[derive(Debug, Args)]
pub(super) struct StatusArgs {
    /// Print the versioned `ReleaseState` JSON document.
    #[arg(long, conflicts_with = "template")]
    json: bool,
    /// Named render. Only `summary` is built in.
    #[arg(long, value_name = "NAME")]
    template: Option<String>,
    /// Git ref to scan from (exclusive). Same default as `generate` / `plan-intent`.
    #[arg(long, value_name = "REF")]
    from: Option<String>,
}

pub(super) fn run(args: &StatusArgs) -> Result<(), Box<dyn std::error::Error>> {
    let target = presentation(args)?;
    let repo = repository::discover()?;
    let config = load_config(&repo)?;
    let workspace = apply_package_overrides(&discover_workspace(&repo)?, &config)?;
    config.validate_workspace_selection(&workspace)?;
    let git = Git::at_repository(&repo)?;
    let files = load_plan_bump_files(&git, &repo, &workspace, &config, args.from.as_deref())?;
    let intent = aggregate(files);
    let mut plan = compose(
        &workspace,
        &intent,
        |id| config.versioning_for(&id.name),
        CascadeAs::Patch,
        |_, dep| Some(dep.range.clone()),
        |id| {
            workspace
                .get(id)
                .expect("compose only asks for workspace packages")
                .version()
                .clone()
        },
    )
    .map_err(|err| CliError::new(err.to_string()))?;
    apply_version_selection(&config, &workspace, &mut plan)?;

    // Coverage detection is okm-22h; this slice reports an empty uncovered list.
    let state = ReleaseState::from_plan(&plan, [], target);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&state)?);
        return Ok(());
    }
    print!("{}", render_summary(&state));
    Ok(())
}

fn presentation(args: &StatusArgs) -> Result<RenderTarget, CliError> {
    if args.json {
        return Ok(RenderTarget::Status);
    }
    match args.template.as_deref().unwrap_or("summary") {
        "summary" => Ok(RenderTarget::Summary),
        name => Err(CliError::new(format!(
            "unknown template `{name}`; known: summary"
        ))),
    }
}

pub(super) fn render_summary(state: &ReleaseState) -> String {
    let mut out = String::from("## Release plan\n\n");
    if state.packages().is_empty() {
        out.push_str("No packages planned.\n");
    } else {
        out.push_str("| Package | From | To | Bump | Source |\n");
        out.push_str("| --- | --- | --- | --- | --- |\n");
        for pkg in state.packages() {
            let _ = match pkg.source() {
                ReleaseSource::Intent => writeln!(
                    out,
                    "| {} (`{}`) | {} | {} | {} | intent |",
                    pkg.name(),
                    ecosystem_label(pkg.ecosystem()),
                    pkg.from_version(),
                    pkg.to_version(),
                    bump_label(pkg.bump()),
                ),
                ReleaseSource::Cascade { trigger } => writeln!(
                    out,
                    "| {} (`{}`) | {} | {} | {} | cascade from {} ({}) |",
                    pkg.name(),
                    ecosystem_label(pkg.ecosystem()),
                    pkg.from_version(),
                    pkg.to_version(),
                    bump_label(pkg.bump()),
                    trigger.name(),
                    ecosystem_label(trigger.ecosystem()),
                ),
            };
        }
    }
    // `status` still does not look at coverage; `ci pr-status` does.
    if !state.uncovered().is_empty() {
        out.push_str("\nUncovered: ");
        for (i, pkg) in state.uncovered().iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            let _ = write!(
                out,
                "{} (`{}`)",
                pkg.name(),
                ecosystem_label(pkg.ecosystem())
            );
        }
        out.push('\n');
    }
    out
}

pub(super) const PR_PLAN_MARKER: &str = "<!-- oakum:pr-plan -->";

pub(super) fn render_comment(state: &ReleaseState) -> String {
    let mut out = String::from(PR_PLAN_MARKER);
    out.push('\n');
    if !state.packages().is_empty() {
        out.push_str("\nThese packages will release:\n\n");
        for pkg in state.packages() {
            let _ = writeln!(
                out,
                "- `{}` {} → {} ({})",
                pkg.name(),
                pkg.from_version(),
                pkg.to_version(),
                bump_label(pkg.bump()),
            );
        }
    }
    if !state.uncovered().is_empty() {
        out.push_str("\nUncovered:\n\n");
        for pkg in state.uncovered() {
            let _ = writeln!(
                out,
                "- `{}` ({}) changed with no bump file",
                pkg.name(),
                ecosystem_label(pkg.ecosystem()),
            );
        }
    }
    out
}

const fn ecosystem_label(ecosystem: EcosystemName) -> &'static str {
    match ecosystem {
        EcosystemName::Cargo => "cargo",
        EcosystemName::Npm => "npm",
    }
}

const fn bump_label(bump: BumpName) -> &'static str {
    match bump {
        BumpName::Patch => "patch",
        BumpName::Minor => "minor",
        BumpName::Major => "major",
    }
}

pub(super) fn apply_package_overrides(
    workspace: &Workspace,
    config: &LoadedConfig,
) -> Result<Workspace, Box<dyn std::error::Error>> {
    let packages: Vec<_> = workspace
        .packages()
        .cloned()
        .map(
            |pkg| match config.resolves_dependencies_at(&pkg.id().name) {
                Some(at) => pkg.with_resolves_dependencies_at(at),
                None => pkg,
            },
        )
        .collect();
    Workspace::new(packages)
        .map(|built| built.with_discovery_paths(workspace))
        .map_err(|err| CliError::new(err.to_string()).into())
}

/// Drop unmanaged plan entries, including cascades that only reach Intent
/// through an unmanaged intermediate.
pub(super) fn apply_version_selection(
    config: &LoadedConfig,
    workspace: &Workspace,
    plan: &mut Plan,
) -> Result<(), CliError> {
    plan.retain_managed(|id, _| {
        workspace
            .get(id)
            .is_some_and(|package| config.version_managed(package))
    })
    .map_err(|id| {
        CliError::new(format!(
            "`{}` is named by intent but is not version-managed; adjust `include`/`exclude` or set `private-packages.version = true`",
            id.name
        ))
    })
}
