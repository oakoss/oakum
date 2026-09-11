//! `oakum status`: emit versioned release state, never deliver it (ADR-0016).

use std::fmt::Write;

use clap::Args;

use oakum::plan::{aggregate, compose, CascadeAs, Plan, Workspace};
use oakum::state::{
    BumpName, Coverage, CoverageLook, CoverageOutcome, EcosystemName, ReleaseSource, ReleaseState,
    RenderTarget,
};

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
    let state = release_state(
        &git,
        &repo,
        &config,
        &workspace,
        args.from.as_deref(),
        target,
        CoverageMode::Reported,
    )?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&state)?);
        return Ok(());
    }
    print!("{}", render_summary(&state));
    Ok(())
}

/// What a caller does when the coverage look fails. Named rather than implied:
/// left to whichever of `match` or `?` a call site happens to write, the
/// disposition is inherited rather than chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CoverageMode {
    /// A failed look refuses — the caller cannot answer without it.
    Required,
    /// A failed look is recorded as not run, and the report goes out saying so.
    Reported,
}

/// The one path from a repository to a [`ReleaseState`], for `status` and `ci
/// pr-status`. Building it twice is how two renderers of the same state come to
/// disagree. The version-PR body is not here: `ci::pr_body` renders a plan it
/// already holds, with no repository to look at, which is why its coverage is
/// `NotAsked` rather than a look that failed.
pub(super) fn release_state(
    git: &Git,
    repo: &repository::Repository,
    config: &LoadedConfig,
    workspace: &Workspace,
    from: Option<&str>,
    target: RenderTarget,
    mode: CoverageMode,
) -> Result<ReleaseState, CliError> {
    let files =
        load_plan_bump_files(git, repo, workspace, config, from).map_err(CliError::from_boxed)?;
    let coverage = resolve_coverage(
        coverage::changed_by_standing(git, workspace, &files, from, |package| {
            config.standing(package)
        }),
        mode,
    )?;
    let intent = aggregate(files);
    let mut plan = compose(
        workspace,
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
    apply_version_selection(config, workspace, &mut plan)?;
    let state = ReleaseState::from_plan(&plan, coverage, target);
    Ok(if preconditions::manages_nothing(config, workspace) {
        state.managing_nothing()
    } else {
        state
    })
}

/// What a failed look becomes, given the caller's disposition. Pure, so the one
/// distinction [`CoverageMode`] exists to draw is testable without a repository
/// — the alternative is a shallow-clone fixture and a subprocess, which is how
/// the disposition went unguarded in the first place.
fn resolve_coverage(
    look: Result<Coverage, CliError>,
    mode: CoverageMode,
) -> Result<CoverageOutcome, CliError> {
    match look {
        Ok(coverage) => Ok(CoverageOutcome::Ran(coverage)),
        Err(err) if mode == CoverageMode::Required => Err(err),
        Err(err) => {
            eprintln!(
                "unverified: coverage not checked: {}",
                verdict_line(&err.detail())
            );
            Ok(CoverageOutcome::Failed)
        }
    }
}

/// git's own diagnostics run to dozens of lines when it falls back to
/// `--no-index`, so the report takes one — but the verdict is the last
/// `fatal:`, not the first `warning:`. Measured on a corrupt loose object: the
/// first line is `error: inflate: data stream error`, while the lines it
/// preceded named the object and concluded `fatal: Not a valid commit name
/// main`.
fn verdict_line(detail: &str) -> &str {
    detail
        .lines()
        .rev()
        .find(|line| line.starts_with("fatal:") || line.starts_with("error:"))
        .or_else(|| detail.lines().find(|line| !line.trim().is_empty()))
        // A blank-only diagnostic still owes one line, not the whole blob.
        .unwrap_or_else(|| detail.lines().next().unwrap_or(detail))
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
    if state.coverage() == CoverageLook::Failed {
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
    if state.coverage() == CoverageLook::Failed {
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

/// The one line a reader gets from a multi-line git diagnostic.
/// The disposition [`CoverageMode`] names, at unit level. The same distinction
/// costs a shallow-clone fixture and a subprocess to observe end to end.
#[cfg(test)]
mod disposition {
    use oakum::state::{Coverage, CoverageOutcome};

    use super::{resolve_coverage, CoverageMode};
    use crate::cli::CliError;

    fn failed_look() -> Result<Coverage, CliError> {
        Err(CliError::unverified("unverified: shallow clone"))
    }

    #[test]
    fn a_required_look_that_failed_refuses() {
        assert!(resolve_coverage(failed_look(), CoverageMode::Required).is_err());
    }

    #[test]
    fn a_reported_look_that_failed_is_recorded_not_refused() {
        assert_eq!(
            resolve_coverage(failed_look(), CoverageMode::Reported).expect("reported"),
            CoverageOutcome::Failed
        );
    }

    #[test]
    fn a_look_that_ran_carries_its_answers_under_either_disposition() {
        for mode in [CoverageMode::Required, CoverageMode::Reported] {
            let outcome = resolve_coverage(Ok(Coverage::default()), mode).expect("ran");
            assert_eq!(outcome, CoverageOutcome::Ran(Coverage::default()));
        }
    }
}

#[cfg(test)]
mod verdict {
    use super::verdict_line;

    #[test]
    fn the_last_fatal_wins_over_an_earlier_warning() {
        let detail = "warning: could not open directory 'vendor/': Permission denied\n\
                      fatal: object database is unreadable; run `git fsck`";
        assert_eq!(
            verdict_line(detail),
            "fatal: object database is unreadable; run `git fsck`"
        );
    }

    /// The case measured against a corrupt loose object: the first line names
    /// the symptom, the last names what git concluded.
    #[test]
    fn an_inflate_error_does_not_outrank_the_conclusion() {
        let detail = "error: inflate: data stream error (incorrect header check)\n\
                      error: unable to unpack 2407861 header\n\
                      fatal: Not a valid commit name main";
        assert_eq!(verdict_line(detail), "fatal: Not a valid commit name main");
    }

    #[test]
    fn a_diagnostic_with_no_verdict_keeps_its_first_line() {
        assert_eq!(
            verdict_line("terminated by a signal"),
            "terminated by a signal"
        );
        assert_eq!(verdict_line(""), "");
    }

    /// The contract is one line, including when no line carries content.
    #[test]
    fn a_blank_diagnostic_still_yields_one_line() {
        assert_eq!(verdict_line("   \n  \n"), "   ");
    }
}

/// The renders, over states built in memory. Every other assertion about a
/// `ReleaseState`'s shape goes through a fixture repository and a subprocess,
/// which is why the emit gate could disagree with the renderer it guarded
/// without a single test failing (`okm-404.33`).
#[cfg(test)]
mod renders {
    use oakum::plan::{
        aggregate, compose, CascadeAs, Ecosystem, Package, PackageId, ResolvesDependenciesAt,
        Versioning, Workspace,
    };
    use oakum::state::{Coverage, CoverageOutcome, ReleaseState, RenderTarget};
    use semver::Version;

    use super::{render_comment, render_summary, PR_PLAN_MARKER};

    fn cargo(name: &str) -> PackageId {
        PackageId::new(Ecosystem::Cargo, name)
    }

    /// A state with no plan: the shape every branch below turns on.
    fn empty_state(target: RenderTarget, coverage: CoverageOutcome) -> ReleaseState {
        let workspace = Workspace::new([Package::new(
            cargo("demo"),
            Version::new(0, 1, 0),
            ResolvesDependenciesAt::Install,
            true,
            Vec::new(),
        )])
        .expect("workspace");
        let plan = compose(
            &workspace,
            &aggregate([]),
            |_| Versioning::ZeroMajor,
            CascadeAs::Patch,
            |_, _| None,
            |id| workspace.get(id).expect("package").version().clone(),
        )
        .expect("plan");
        ReleaseState::from_plan(&plan, coverage, target)
    }

    fn unmanaged_only() -> Coverage {
        Coverage {
            uncovered: Vec::new(),
            unmanaged: vec![cargo("beta")],
        }
    }

    #[test]
    fn a_comment_carrying_only_unmanaged_packages_is_not_empty() {
        let body = render_comment(&empty_state(
            RenderTarget::Comment,
            CoverageOutcome::Ran(unmanaged_only()),
        ))
        .expect("a state with something to say renders a body");
        assert!(body.contains("beta"), "{body}");
        assert_ne!(body.trim_end(), PR_PLAN_MARKER);
    }

    #[test]
    fn a_comment_with_nothing_to_say_is_none() {
        let quiet = empty_state(
            RenderTarget::Comment,
            CoverageOutcome::Ran(Coverage::default()),
        );
        assert_eq!(render_comment(&quiet), None);
    }

    /// A look that failed is not a look that found nothing, and the render is
    /// what travels — stderr reaches neither a step summary nor a comment.
    ///
    /// The summary arm is reachable today; the comment arm is not, because
    /// `ci pr-status` passes `CoverageMode::Required` and refuses before it
    /// could build such a state. It guards the renderer against a future caller
    /// choosing `Reported`.
    #[test]
    fn a_failed_coverage_look_is_named_in_both_renders() {
        let failed = empty_state(RenderTarget::Comment, CoverageOutcome::Failed);
        let body = render_comment(&failed).expect("a failed look is worth saying");
        assert!(body.contains("Coverage was not checked"), "{body}");
        assert!(
            render_summary(&empty_state(RenderTarget::Summary, CoverageOutcome::Failed))
                .contains("Coverage was not checked")
        );
    }

    /// The version-PR body renders a plan already in hand: no look was
    /// attempted and none could have been. Blaming git there is a false
    /// sentence in the most-read artifact oakum writes.
    #[test]
    fn a_look_nobody_asked_for_is_not_reported_as_a_failure() {
        let summary = render_summary(&empty_state(
            RenderTarget::Status,
            CoverageOutcome::NotAsked,
        ));
        assert!(
            !summary.contains("Coverage was not checked"),
            "nothing asked, so nothing failed: {summary}"
        );
    }

    #[test]
    fn a_config_that_manages_nothing_is_named_in_both_renders() {
        let state = empty_state(
            RenderTarget::Comment,
            CoverageOutcome::Ran(Coverage::default()),
        )
        .managing_nothing();
        let body = render_comment(&state).expect("a config that can never release is worth saying");
        assert!(body.contains("manages no package on either axis"), "{body}");
        let summary = empty_state(
            RenderTarget::Summary,
            CoverageOutcome::Ran(Coverage::default()),
        )
        .managing_nothing();
        assert!(render_summary(&summary).contains("manages no package on either axis"));
    }
}
