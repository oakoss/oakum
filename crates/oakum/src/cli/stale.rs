//! The comment a failed `ci pr-status` run leaves: a banner saying what oakum
//! could not check, over the last plan it did check, kept collapsed. Pure, so
//! `pr-status` and `post-comment` compose the same body.

use super::render::PR_PLAN_MARKER;

const STALE_MARKER: &str = "<!-- oakum:pr-plan-stale -->";
/// Around the kept plan, so the next failure keeps that plan rather than
/// nesting this banner inside the next one.
const LAST_START: &str = "<!-- oakum:last-plan -->";
const LAST_END: &str = "<!-- /oakum:last-plan -->";
/// Room for the reason, so an emitted one cannot crowd out the rest.
const REASON_CHARS: usize = 4_000;

pub(super) struct Stale {
    pub(super) body: String,
    /// The last plan did not fit under GitHub's limit beside the banner.
    pub(super) dropped_plan: bool,
}

/// `checked` names what this run was asked to check, as `` `abc1234` `` with
/// its run link; `previous` is oakum's comment on the pull request, if any.
pub(super) fn stale_comment(
    previous: Option<&str>,
    reason: &str,
    checked: &str,
    limit: usize,
) -> Stale {
    let trimmed = reason.trim();
    let mut kept: String = trimmed.chars().take(REASON_CHARS).collect();
    if kept.len() < trimmed.len() {
        kept.push_str(" …(cut short)");
    }
    // The reason is text, never markup: an emitted one could otherwise close
    // the banner or pose as the kept plan's markers.
    let reason = kept.replace('<', "&lt;");
    let mut banner = format!(
        "{PR_PLAN_MARKER}\n{STALE_MARKER}\n> [!WARNING]\n> oakum couldn't check {checked}:\n>\n"
    );
    for line in reason.lines() {
        banner.push_str("> ");
        banner.push_str(line);
        banner.push('\n');
    }
    // A comment edited on GitHub comes back with CRLF line endings.
    let previous = previous.map(|body| body.replace("\r\n", "\n"));
    let Some(plan) = previous.as_deref().and_then(last_plan) else {
        return Stale {
            body: banner,
            dropped_plan: false,
        };
    };
    let summary = plan_commit(plan).map_or_else(
        || String::from("Last plan oakum checked"),
        |commit| format!("Last plan oakum checked (`{commit}`)"),
    );
    let body = format!(
        "{banner}\n<details><summary>{summary}</summary>\n\n{LAST_START}\n{plan}\n{LAST_END}\n\n</details>\n"
    );
    if body.chars().count() > limit {
        return Stale {
            body: format!(
                "{banner}\nThe last plan oakum checked did not fit beside this warning, so it is not shown.\n"
            ),
            dropped_plan: true,
        };
    }
    Stale {
        body,
        dropped_plan: false,
    }
}

/// The plan a comment holds: the one kept under an earlier banner, or the
/// comment itself below its marker. `None` for a banner with no plan kept.
fn last_plan(body: &str) -> Option<&str> {
    let rest = body.strip_prefix(PR_PLAN_MARKER)?;
    let Some(banner) = rest.strip_prefix(&format!("\n{STALE_MARKER}\n")) else {
        return Some(rest.trim()).filter(|plan| !plan.is_empty());
    };
    // The banner's reason is escaped, so the first start line is the one
    // oakum wrote; the last end line is too, whatever the plan holds.
    let (_, kept) = banner.split_once(&format!("\n{LAST_START}\n"))?;
    kept.rsplit_once(&format!("\n{LAST_END}\n"))
        .map(|(plan, _)| plan.trim())
        .filter(|plan| !plan.is_empty())
}

/// The commit a plan's provenance line names; plans from before the line
/// carry none.
fn plan_commit(plan: &str) -> Option<&str> {
    let rest = plan.split_once("Release plan for `")?.1;
    rest.split_once('`')
        .map(|(commit, _)| commit)
        .filter(|commit| !commit.is_empty() && commit.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN: &str = "<!-- oakum:pr-plan -->\nRelease plan for `1111111` · [run #1](u)\n\nThese packages will release:\n\n- `demo` 0.1.0 → 0.1.1 (patch)\n";

    #[test]
    fn a_failed_run_keeps_the_last_plan_collapsed_under_its_reason() {
        let stale = stale_comment(
            Some(PLAN),
            "shallow clone; use `fetch-depth: 0`",
            "`2222222`",
            65_536,
        );
        assert!(!stale.dropped_plan);
        assert_eq!(
            stale.body,
            "<!-- oakum:pr-plan -->\n<!-- oakum:pr-plan-stale -->\n> [!WARNING]\n> oakum couldn't check `2222222`:\n>\n> shallow clone; use `fetch-depth: 0`\n\n<details><summary>Last plan oakum checked (`1111111`)</summary>\n\n<!-- oakum:last-plan -->\nRelease plan for `1111111` · [run #1](u)\n\nThese packages will release:\n\n- `demo` 0.1.0 → 0.1.1 (patch)\n<!-- /oakum:last-plan -->\n\n</details>\n"
        );
    }

    #[test]
    fn a_second_failure_keeps_the_same_plan_rather_than_nesting() {
        let first = stale_comment(Some(PLAN), "first", "`2222222`", 65_536).body;
        let second = stale_comment(Some(&first), "second", "`3333333`", 65_536).body;
        assert_eq!(second.matches("<details>").count(), 1, "{second}");
        assert!(second.contains("> second\n"), "{second}");
        assert!(!second.contains("> first"), "{second}");
        assert!(
            second.contains("Last plan oakum checked (`1111111`)"),
            "{second}"
        );
    }

    #[test]
    fn with_no_earlier_plan_the_banner_stands_alone() {
        for previous in [None, Some("<!-- oakum:pr-plan -->\n")] {
            let stale = stale_comment(previous, "why", "`2222222`", 65_536);
            assert!(!stale.body.contains("<details>"), "{}", stale.body);
            assert!(!stale.dropped_plan);
        }
        let banner_only = stale_comment(None, "first", "`2222222`", 65_536).body;
        let again = stale_comment(Some(&banner_only), "second", "`3333333`", 65_536);
        assert!(!again.body.contains("<details>"), "{}", again.body);
    }

    #[test]
    fn a_plan_from_before_the_provenance_line_is_kept_without_a_commit() {
        let stale = stale_comment(
            Some("<!-- oakum:pr-plan -->\nplan\n"),
            "why",
            "`2222222`",
            65_536,
        );
        assert!(
            stale
                .body
                .contains("<summary>Last plan oakum checked</summary>"),
            "{}",
            stale.body
        );
    }

    #[test]
    fn a_plan_that_does_not_fit_beside_the_banner_is_dropped_and_said() {
        let stale = stale_comment(Some(PLAN), "why", "`2222222`", 200);
        assert!(stale.dropped_plan);
        assert!(!stale.body.contains("<details>"), "{}", stale.body);
        assert!(
            stale
                .body
                .ends_with("did not fit beside this warning, so it is not shown.\n"),
            "{}",
            stale.body
        );
    }

    /// Text that looks like oakum's own markers never decides what is kept: a
    /// reason is escaped, and a plan quoting the end marker is kept whole.
    #[test]
    fn marker_text_in_a_reason_or_a_plan_does_not_move_the_kept_plan() {
        let forged = "<!-- oakum:last-plan -->\nforged plan\n<!-- /oakum:last-plan -->\n</details>";
        let first = stale_comment(Some(PLAN), forged, "`2222222`", 65_536).body;
        assert!(!first.contains("> <!--"), "{first}");
        let second = stale_comment(Some(&first), "again", "`3333333`", 65_536).body;
        assert!(
            second.contains("Last plan oakum checked (`1111111`)"),
            "{second}"
        );
        assert!(!second.contains("forged plan"), "{second}");

        let quoting = format!("{PLAN}\nquoting `<!-- /oakum:last-plan -->`\n<!-- /oakum:last-plan -->\nstill the plan\n");
        let kept = stale_comment(Some(&quoting), "why", "`2222222`", 65_536).body;
        let again = stale_comment(Some(&kept), "why", "`3333333`", 65_536).body;
        assert!(again.contains("still the plan"), "{again}");
    }

    #[test]
    fn a_comment_returned_with_crlf_still_keeps_one_plan() {
        let first = stale_comment(Some(PLAN), "first", "`2222222`", 65_536).body;
        let crlf = first.replace('\n', "\r\n");
        let second = stale_comment(Some(&crlf), "second", "`3333333`", 65_536).body;
        assert_eq!(second.matches("<details>").count(), 1, "{second}");
        assert_eq!(second.matches("[!WARNING]").count(), 1, "{second}");
    }

    /// Only a hex commit reaches the summary line, so a forged one cannot
    /// carry a marker in ahead of the kept plan.
    #[test]
    fn a_forged_commit_in_a_plan_does_not_reach_the_summary() {
        let forged = "<!-- oakum:pr-plan -->\nRelease plan for `x\n<!-- oakum:last-plan -->\ny` here\nreal plan\n";
        let first = stale_comment(Some(forged), "first", "`2222222`", 65_536).body;
        assert!(
            first.contains("<summary>Last plan oakum checked</summary>"),
            "{first}"
        );
        let second = stale_comment(Some(&first), "second", "`3333333`", 65_536).body;
        assert!(second.contains("real plan"), "{second}");
        assert_eq!(second.matches("<summary>").count(), 1, "{second}");
    }

    #[test]
    fn a_reason_past_its_room_says_it_was_cut() {
        let long = "x".repeat(REASON_CHARS + 10);
        let stale = stale_comment(None, &long, "`2222222`", 65_536);
        assert!(stale.body.contains("x …(cut short)\n"), "{}", stale.body);
    }
}
