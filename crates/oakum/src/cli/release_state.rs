//! The one path from a repository to a [`ReleaseState`] for `status` and
//! `ci pr-status`, and the plan pipeline `version` shares with it.

use std::collections::BTreeMap;

use oakum::plan::{
    aggregate, compose, AggregatedBump, BumpFile, CascadeAs, PackageId, Plan, Workspace,
};
use oakum::state::{Coverage, CoverageOutcome, ReleaseState, RenderTarget};

use super::add::discover_workspace;
use super::config::{intent_names_unmanaged, LoadedConfig};
use super::coverage;
use super::git::Git;
use super::intent::load_plan_bump_files;
use super::preconditions;
use super::repository;
use super::say_err;
use super::verdict::verdict_line;
use super::CliError;

/// The workspace, the git handle and the bump files, discovered once for a
/// caller about to plan. Named apart from `preconditions::Loaded`, which is
/// `check`'s config-and-workspace pair: these are the three a planner needs,
/// and `check` deliberately skips the package overrides, so it is not a third
/// caller of this.
///
/// The selection check rides here rather than at each call site, because a
/// caller that discovers a workspace and forgets to validate it against the
/// config plans over packages the config never named.
pub(super) struct Discovered {
    workspace: Workspace,
    git: Git,
    files: Vec<BumpFile>,
}

impl Discovered {
    /// # Errors
    ///
    /// Discovery, a selection naming a package the workspace does not have,
    /// a git handle that cannot open, or bump files that cannot be read.
    pub(super) fn read(
        repo: &repository::Repository,
        config: &LoadedConfig,
        from: Option<&str>,
    ) -> Result<Self, CliError> {
        let workspace = apply_package_overrides(
            &discover_workspace(repo).map_err(CliError::from_boxed)?,
            config,
        )
        .map_err(CliError::from_boxed)?;
        config.validate_workspace_selection(&workspace)?;
        let git = Git::at_repository(repo).map_err(CliError::from_boxed)?;
        let files = load_plan_bump_files(&git, repo, &workspace, config, from)
            .map_err(CliError::from_boxed)?;
        Ok(Self {
            workspace,
            git,
            files,
        })
    }

    /// The three, for a caller about to plan. Private fields plus this are
    /// what make [`Self::read`] the only way in: a struct literal elsewhere
    /// would skip the selection check this type exists to carry, which was
    /// measured turning an unknown `include` from a refusal into `versioned
    /// nothing` at exit 0.
    pub(super) fn into_parts(self) -> (Workspace, Git, Vec<BumpFile>) {
        (self.workspace, self.git, self.files)
    }
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
///
/// Takes the config rather than loading one, so the config that chose the
/// channel is the config that built the plan; the workspace and git are
/// discovered here, once. Measured before this: `ci pr-status` opened
/// `_config.toml` twice.
pub(super) fn release_state(
    repo: &repository::Repository,
    config: &LoadedConfig,
    from: Option<&str>,
    target: RenderTarget,
    mode: CoverageMode,
) -> Result<ReleaseState, CliError> {
    let (workspace, git, files) = Discovered::read(repo, config, from)?.into_parts();
    let coverage = resolve_coverage(
        coverage::changed_by_standing(&git, &workspace, &files, from, |package| {
            config.standing(package)
        }),
        mode,
    )?;
    let plan = compose_plan(config, &workspace, &aggregate(files))?;
    let state = ReleaseState::from_plan(&plan, coverage, target);
    Ok(if preconditions::manages_nothing(config, &workspace) {
        state.managing_nothing()
    } else if preconditions::selection_is_empty(config, &workspace) {
        state.selection_emptied()
    } else {
        state
    })
}

/// One plan pipeline for every renderer: compose, then the version selection.
/// Measured before this: `version` carried its own copy, so two pipelines fed
/// one renderer.
pub(super) fn compose_plan(
    config: &LoadedConfig,
    workspace: &Workspace,
    intent: &BTreeMap<PackageId, AggregatedBump>,
) -> Result<Plan, CliError> {
    let mut plan = compose(
        workspace,
        intent,
        |id| config.versioning_for(&id.name),
        CascadeAs::Patch,
    )
    .map_err(|err| CliError::new(err.to_string()))?;
    apply_version_selection(config, workspace, &mut plan)?;
    Ok(plan)
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
            say_err(&format!(
                "unverified: coverage not checked: {}",
                verdict_line(&err.detail())
            ));
            Ok(CoverageOutcome::Failed)
        }
    }
}

fn apply_package_overrides(
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
