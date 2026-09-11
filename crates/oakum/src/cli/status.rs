//! `oakum status`: emit versioned release state, never deliver it (ADR-0016).

use std::fmt::Write;

use clap::Args;

use oakum::plan::{aggregate, compose, CascadeAs, Plan, Workspace};
use oakum::state::{BumpName, EcosystemName, ReleaseSource, ReleaseState, RenderTarget};

use super::add::discover_workspace;
use super::config::{intent_names_unmanaged, load_config, LoadedConfig, UNMANAGED_FIX};
use super::coverage;
use super::git::Git;
use super::intent::load_plan_bump_files;
use super::preconditions;
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
    /// Git ref to scan from (exclusive). Same default as `generate` / `check`.
    #[arg(long, value_name = "REF")]
    from: Option<String>,
}

pub(super) fn run(args: &StatusArgs) -> Result<(), Box<dyn std::error::Error>> {
    let target = presentation(args)?;
    let repo = repository::discover()?;
    let config = load_config(&repo)?;
    if config.is_default() {
        eprintln!("{}", super::config::DEFAULTS_NOTE);
    }
    let workspace = apply_package_overrides(&discover_workspace(&repo)?, &config)?;
    config.validate_workspace_selection(&workspace)?;
    let git = Git::at_repository(&repo)?;
    let files = load_plan_bump_files(&git, &repo, &workspace, &config, args.from.as_deref())?;
    // A tree git cannot diff is not a tree with nothing uncovered. `status`
    // reports either way — it is not a gate — but it says which happened
    // instead of printing an empty list for both.
    let coverage = match coverage::changed_by_standing(
        &git,
        &workspace,
        &files,
        args.from.as_deref(),
        |package| config.standing(package),
    ) {
        Ok(coverage) => Some(coverage),
        Err(err) => {
            eprintln!(
                "unverified: coverage not checked: {}",
                first_line(&err.detail())
            );
            None
        }
    };
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

    let state = ReleaseState::from_plan(&plan, coverage, target);
    // `status` reports; it does not gate. This changes what an empty `packages`
    // means, not the exit code.
    let state = if preconditions::manages_nothing(&config, &workspace) {
        state.managing_nothing()
    } else {
        state
    };
    if args.json {
        println!("{}", serde_json::to_string_pretty(&state)?);
        return Ok(());
    }
    print!("{}", render_summary(&state));
    Ok(())
}

/// git's own diagnostics run to dozens of lines when it falls back to
/// `--no-index`; the report is one line, and the command and exit code are
/// already in it.
fn first_line(detail: &str) -> &str {
    detail.lines().next().unwrap_or(detail)
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
        // Without this the same sentence covers two states a reader must act on
        // differently: waiting for intent, and a config that will never produce
        // any. `check` refuses on the second; `status` only says so.
        if state.manages_nothing() {
            out.push_str(
                "\nThis config manages no package on either axis, so no plan can ever be non-empty; set `private-packages.version` / `private-packages.tag`, or adjust `include`/`exclude`.\n",
            );
        }
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
    if !state.uncovered().is_empty() {
        out.push_str("\nUncovered: ");
        out.push_str(&package_list(state.uncovered()));
        out.push('\n');
    }
    if !state.unmanaged().is_empty() {
        let _ = write!(
            out,
            "\nChanged but not version-managed: {}\n{UNMANAGED_FIX}\n",
            package_list(state.unmanaged())
        );
    }
    // The JSON separates a look that ran from one that did not; this render is
    // what a reviewer actually reads, and stderr does not travel into a step
    // summary or a pull-request body.
    if !state.coverage_checked() {
        out.push_str(
            "\nCoverage was not checked: git could not diff this tree, so an empty uncovered list is not a clean result.\n",
        );
    }
    out
}

fn package_list(packages: &[oakum::state::PackageRef]) -> String {
    packages
        .iter()
        .map(|pkg| format!("{} (`{}`)", pkg.name(), ecosystem_label(pkg.ecosystem())))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) const PR_PLAN_MARKER: &str = "<!-- oakum:pr-plan -->";

/// `None` when the body would be nothing but the invisible marker. Emptiness is
/// the renderer's to decide: a separate predicate can agree with this today and
/// disagree after one of them grows a case, which posts a blank comment.
pub(super) fn render_comment(state: &ReleaseState) -> Option<String> {
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
    if !state.unmanaged().is_empty() {
        out.push_str("\nChanged but not version-managed:\n\n");
        for pkg in state.unmanaged() {
            let _ = writeln!(
                out,
                "- `{}` ({}) — {UNMANAGED_FIX}",
                pkg.name(),
                ecosystem_label(pkg.ecosystem()),
            );
        }
    }
    if state.manages_nothing() {
        out.push_str(
            "\nThis config manages no package on either axis, so no plan can ever be non-empty.\n",
        );
    }
    if !state.coverage_checked() {
        out.push_str(
            "\nCoverage was not checked: git could not diff this tree, so an empty uncovered list is not a clean result.\n",
        );
    }
    (out.trim_end() != PR_PLAN_MARKER).then_some(out)
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
    .map_err(|id| CliError::new(intent_names_unmanaged(&id.name)))
}
