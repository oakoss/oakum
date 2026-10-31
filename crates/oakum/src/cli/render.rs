//! The renders of a [`ReleaseState`]: the `status` summary and the pull
//! request comment `ci pr-status` posts, with the marker that finds it again.

use std::fmt::Write;

use oakum::state::{BumpName, CoverageLook, EcosystemName, ReleaseSource, ReleaseState};

use super::config::{unresolved_scope_line, UNMANAGED_FIX};

/// Where intent comes from, which the state document does not carry, so the
/// renders can name the command that adds it. `None` where no one acts on the
/// plan: the version pull request body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum NextStep {
    BumpFiles {
        pending: usize,
        generate: bool,
        /// The quoted base `generate --from` covers this branch from, when
        /// `generate` is allowed and the branch has no bump file of its own.
        generate_from: Option<String>,
    },
    Commits,
}

impl NextStep {
    fn cover_uncovered(&self) -> String {
        match self {
            Self::BumpFiles { generate_from, .. } => {
                let mut line = String::from(
                    "Cover each with `oakum add --packages <package>:<level>`, or `<package>:none` to cover it without a release",
                );
                if let Some(base) = generate_from {
                    let _ = write!(
                        line,
                        "; `oakum generate --from {base}` writes them from conventional commits"
                    );
                }
                line.push('.');
                line
            }
            Self::Commits => {
                String::from("Name each in a conventional commit, or in a path that maps to it.")
            }
        }
    }
}

pub(super) fn render_summary(state: &ReleaseState, next: Option<&NextStep>) -> String {
    let mut out = String::from("## Release plan\n\n");
    if state.packages().is_empty() {
        out.push_str("No packages planned.\n");
        if let Some(NextStep::BumpFiles {
            pending: 0,
            generate,
            ..
        }) = next
        {
            if !state.manages_nothing() && !state.selection_empty() {
                out.push_str("\nNo bump file is pending in `.changeset`. `oakum add` writes one");
                if *generate {
                    out.push_str(", and `oakum generate` writes one from conventional commits");
                }
                out.push_str(".\n");
            }
        }
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
        if let Some(next) = next {
            let _ = writeln!(out, "{}", next.cover_uncovered());
        }
    }
    if !state.unmanaged().is_empty() {
        let _ = write!(
            out,
            "\nChanged but not version-managed: {}\n{UNMANAGED_FIX}\n",
            package_list(state.unmanaged())
        );
    }
    if !state.named_unmanaged().is_empty() {
        let _ = write!(
            out,
            "\nNamed by a commit but not version-managed: {}\n{UNMANAGED_FIX}\n",
            package_list(state.named_unmanaged())
        );
    }
    push_unresolved_scopes(&mut out, state);
    if !state.awaiting_tag().is_empty() {
        let _ = write!(
            out,
            "\nReleased, awaiting its tag: {}\n{AWAITING_TAG}\n",
            package_list(state.awaiting_tag())
        );
    }
    push_hand_set(&mut out, state);
    // The JSON separates a look that ran from one that did not; this render is
    // what a reviewer reads, and stderr does not travel into a step
    // summary or a pull-request body.
    if state.coverage() == CoverageLook::Failed {
        out.push_str(
            "\nCoverage was not checked: git could not diff this tree, so an empty uncovered list is not a clean result.\n",
        );
    }
    out
}

const AWAITING_TAG: &str = "no commits are planned for it until `oakum release` tags the release";

fn push_hand_set(out: &mut String, state: &ReleaseState) {
    if state.hand_set().is_empty() {
        return;
    }
    out.push_str(
        "\nBlocked by a version set by hand; `oakum version` refuses this plan until each is fixed:\n\n",
    );
    for entry in state.hand_set() {
        let _ = writeln!(out, "- {}", entry.fix());
    }
}

fn push_unresolved_scopes(out: &mut String, state: &ReleaseState) {
    if state.unresolved_scopes().is_empty() {
        return;
    }
    out.push_str("\nReleased by a commit but reaching no package:\n\n");
    for commit in state.unresolved_scopes() {
        let _ = writeln!(out, "- {}", unresolved_scope_line(commit));
    }
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
pub(super) fn render_comment(state: &ReleaseState, next: Option<&NextStep>) -> Option<String> {
    let mut out = String::from(PR_PLAN_MARKER);
    out.push('\n');
    // Before the list, so the comment does not first promise a release that
    // `version` will refuse.
    push_hand_set(&mut out, state);
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
        if let Some(next) = next {
            let _ = write!(out, "\n{}\n", next.cover_uncovered());
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
    if !state.named_unmanaged().is_empty() {
        out.push_str("\nNamed by a commit but not version-managed:\n\n");
        for pkg in state.named_unmanaged() {
            let _ = writeln!(
                out,
                "- `{}` ({}) — {UNMANAGED_FIX}",
                pkg.name(),
                ecosystem_label(pkg.ecosystem()),
            );
        }
    }
    push_unresolved_scopes(&mut out, state);
    if !state.awaiting_tag().is_empty() {
        out.push_str("\nReleased, awaiting its tag:\n\n");
        for pkg in state.awaiting_tag() {
            let _ = writeln!(
                out,
                "- `{}` ({}) — {AWAITING_TAG}",
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
    use oakum::state::{Coverage, CoverageOutcome, HandSet, ReleaseState, RenderTarget};
    use semver::Version;

    use super::{NextStep, PR_PLAN_MARKER};

    fn render_summary(state: &ReleaseState) -> String {
        super::render_summary(state, None)
    }

    fn render_comment(state: &ReleaseState) -> Option<String> {
        super::render_comment(state, None)
    }

    fn bump_files(pending: usize, generate: bool, generate_from: Option<&str>) -> NextStep {
        NextStep::BumpFiles {
            pending,
            generate,
            generate_from: generate_from.map(String::from),
        }
    }

    /// After `init`, an empty plan names the command that starts one.
    #[test]
    fn an_empty_plan_with_no_bump_file_names_add() {
        let state = empty_state(RenderTarget::Status, CoverageOutcome::NotAsked);
        let summary = super::render_summary(&state, Some(&bump_files(0, false, None)));
        assert!(
            summary.contains("No bump file is pending in `.changeset`. `oakum add` writes one.\n"),
            "{summary}"
        );
        assert!(
            super::render_summary(&state, Some(&bump_files(0, true, None)))
                .contains("and `oakum generate` writes one from conventional commits.")
        );
        for quiet in [
            super::render_summary(&state, Some(&bump_files(1, true, None))),
            super::render_summary(&state, Some(&NextStep::Commits)),
            super::render_summary(&state, None),
            super::render_summary(
                &state.clone().managing_nothing(),
                Some(&bump_files(0, false, None)),
            ),
            super::render_summary(
                &state.clone().selection_emptied(),
                Some(&bump_files(0, false, None)),
            ),
        ] {
            assert!(!quiet.contains("oakum add"), "{quiet}");
        }
    }

    /// `post-comment` and `pr-status` own only a comment that starts with the
    /// marker, so a comment that does not would be posted again every run.
    #[test]
    fn the_comment_starts_with_the_marker() {
        let state = empty_state(
            RenderTarget::Comment,
            CoverageOutcome::Ran(Coverage {
                uncovered: vec![cargo("demo")],
                unmanaged: Vec::new(),
            }),
        );
        for next in [None, Some(&NextStep::Commits)] {
            let comment = super::render_comment(&state, next).expect("comment");
            assert!(comment.starts_with(PR_PLAN_MARKER), "{comment}");
        }
    }

    /// `check` names the fix for an uncovered package; the summary and the
    /// comment a contributor reads do too.
    #[test]
    fn an_uncovered_package_names_the_command_that_covers_it() {
        let state = empty_state(
            RenderTarget::Comment,
            CoverageOutcome::Ran(Coverage {
                uncovered: vec![cargo("demo")],
                unmanaged: Vec::new(),
            }),
        );
        let add = "Cover each with `oakum add --packages <package>:<level>`, or `<package>:none` to cover it without a release.";
        let summary = super::render_summary(&state, Some(&bump_files(0, false, None)));
        assert!(
            summary.contains(&format!("Uncovered: demo (`cargo`)\n{add}")),
            "{summary}"
        );
        let comment =
            super::render_comment(&state, Some(&bump_files(0, false, None))).expect("comment");
        assert!(comment.contains(add), "{comment}");
        let generate = super::render_summary(&state, Some(&bump_files(0, true, Some("abc123"))));
        assert!(
            generate.contains(
                "; `oakum generate --from abc123` writes them from conventional commits."
            ),
            "{generate}"
        );
        let commits = super::render_summary(&state, Some(&NextStep::Commits));
        assert!(
            commits.contains("Name each in a conventional commit"),
            "{commits}"
        );
        assert!(!render_summary(&state).contains("Cover each"));
    }

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

    /// The pull-request comment is where a commits-only repository reads it;
    /// a coverage look that did not run does not hide it.
    #[test]
    fn a_package_named_by_a_commit_reaches_both_renders() {
        let state = empty_state(RenderTarget::Comment, CoverageOutcome::NotAsked)
            .with_named_unmanaged(vec![cargo("beta")]);
        let body = render_comment(&state).expect("a named package is worth saying");
        assert!(
            body.contains("Named by a commit but not version-managed:\n\n- `beta` (cargo)"),
            "{body}"
        );
        let summary = render_summary(&state);
        assert!(
            summary.contains("Named by a commit but not version-managed: beta (`cargo`)"),
            "{summary}"
        );
    }

    #[test]
    fn a_commit_reaching_no_package_reaches_both_renders() {
        let state = empty_state(RenderTarget::Comment, CoverageOutcome::NotAsked)
            .with_unresolved_scopes(vec![oakum::state::UnresolvedScope {
                commit: String::from("0123456789abcdef"),
                scope: String::from("gamma"),
            }]);
        let line = "- commit 0123456 asks for a release under scope `gamma`";
        let body = render_comment(&state).expect("a dropped release is worth saying");
        assert!(body.contains(line), "{body}");
        assert!(render_summary(&state).contains(line), "{state:?}");
    }

    #[test]
    fn a_package_awaiting_its_tag_reaches_both_renders() {
        let state = empty_state(RenderTarget::Comment, CoverageOutcome::NotAsked)
            .with_awaiting_tag(vec![cargo("alpha")]);
        let body = render_comment(&state).expect("a pending tag is worth saying");
        assert!(
            body.contains("Released, awaiting its tag:\n\n- `alpha` (cargo)"),
            "{body}"
        );
        assert!(
            render_summary(&state).contains("Released, awaiting its tag: alpha (`cargo`)"),
            "{state:?}"
        );
    }

    #[test]
    fn a_hand_set_version_reaches_both_renders_before_the_release() {
        let planned: ReleaseState = serde_json::from_str(
            r#"{"schema_version":1,"target":"comment","uncovered":[],
                "packages":[{"name":"demo","ecosystem":"cargo","from":"0.1.1","to":"0.1.2",
                             "bump":"patch","source":{"kind":"intent"}}]}"#,
        )
        .expect("state");
        let state = planned.with_hand_set(vec![HandSet::new(
            cargo("demo"),
            Version::new(0, 1, 1),
            String::from("demo (cargo): tag it"),
        )]);
        let body = render_comment(&state).expect("a block is worth saying");
        let block = body.find("- demo (cargo): tag it").expect(&body);
        assert!(
            block < body.find("These packages will release").expect(&body),
            "{body}"
        );
        assert!(
            render_summary(&state).contains("version set by hand; `oakum version` refuses this plan until each is fixed:\n\n- demo (cargo): tag it"),
            "{state:?}"
        );
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
