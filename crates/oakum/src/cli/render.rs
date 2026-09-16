//! The renders of a [`ReleaseState`]: the `status` summary and the pull
//! request comment `ci pr-status` posts, with the marker that finds it again.

use std::fmt::Write;

use oakum::state::{BumpName, CoverageLook, EcosystemName, ReleaseSource, ReleaseState};

use super::config::UNMANAGED_FIX;

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
        // `check` stays silent here on purpose: a gate must not refuse a
        // decision the config wrote down. `status` is not a gate (`okm-404.32`).
        if state.selection_empty() {
            out.push_str(
                "\n`include`/`exclude` leave no package selected, so no plan can ever name one. `check` does not refuse this, because it is a decision this config states.\n",
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
    if state.selection_empty() {
        out.push_str(
            "\n`include`/`exclude` leave no package selected, so no plan can ever name one.\n",
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

    /// Its sibling. `check` stays silent on an emptied selection, so the render
    /// is the only place a reader learns of it — in both targets, since the
    /// comment render is what a pull request shows.
    #[test]
    fn an_emptied_selection_is_named_in_both_renders() {
        let summary = render_summary(
            &empty_state(RenderTarget::Status, CoverageOutcome::NotAsked).selection_emptied(),
        );
        assert!(
            summary.contains("`include`/`exclude` leave no package selected"),
            "{summary}"
        );
        assert!(
            summary.contains("`check` does not refuse this"),
            "the divergence is the point: {summary}"
        );
        let comment = render_comment(
            &empty_state(RenderTarget::Comment, CoverageOutcome::NotAsked).selection_emptied(),
        )
        .expect("an emptied selection is worth a comment");
        assert!(
            comment.contains("`include`/`exclude` leave no package selected"),
            "{comment}"
        );
    }
}
