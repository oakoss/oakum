//! What `migrate` prints. Rendering only; every decision and every look at
//! the tree stays in `migrate`.

use oakum::plan::{format_versions, PlanComparison};

use super::ci::VERSION_BRANCH;
use super::init::{OwnedPlan, ReadmeState, SchemaState, README_REL};

/// The pending line for the owned files, from the same probe the writes use.
pub(super) fn pending_owned_line(owned: OwnedPlan) -> String {
    let schema = match owned.schema {
        SchemaState::Absent => "write .changeset/_schema.json",
        SchemaState::Stale => "replace the existing .changeset/_schema.json",
        SchemaState::Current => "leave the current .changeset/_schema.json",
    };
    if owned.readme == ReadmeState::Absent {
        format!("write .changeset/_config.toml and .changeset/README.md, and {schema}")
    } else {
        format!("write .changeset/_config.toml and {schema} (keeping the existing .changeset/README.md)")
    }
}

pub(super) fn print_pending(planned: &[(String, String)], dropped: &[String], owned: OwnedPlan) {
    println!("pending:");
    for (path, _) in planned {
        println!("  rewrite {path}");
    }
    println!("  {}", pending_owned_line(owned));
    for key in dropped {
        println!("  leave `{key}` behind in `.changeset/config.json` (not an oakum config key)");
    }
}

pub(super) fn print_left_alone(readme: ReadmeState, dropped: &[String]) {
    if readme == ReadmeState::Theirs {
        println!("kept {README_REL} (oakum did not write it; left as is)");
    }
    for key in dropped {
        println!(
            "not carried over: `{key}` (not an oakum config key; `.changeset/config.json` is untouched)"
        );
    }
}

pub(super) fn print_remaining_steps(
    detections: &[oakum::detect::Detection],
    knope: bool,
    foreign_changelogs: &[String],
) {
    println!("remaining (oakum does not perform these):");
    for report in foreign_changelogs {
        println!("- {report}");
    }
    println!("- add oakum to a workflow (YAML printed below)");
    println!(
        "- publish: `oakum release` only tags and creates the GitHub release; the old workflow's publish step (`npm publish`, `cargo publish`) needs a job of its own on the tag push (`on: push: tags`)"
    );
    println!(
        "- the version PR opens on branch `{VERSION_BRANCH}`; add it to any branch-name filters that need it"
    );
    for hit in detections {
        if let Some(path) = remaining_removal(hit.evidence()) {
            println!("- remove {path} ({})", hit.tool().name());
        }
    }
    println!("- remove the old tool's dependency and its workflow");
    if knope {
        println!(
            "- `.changeset/README.md` aborts knope until `knope.toml` and its workflow are removed"
        );
    }
}

/// `.changeset/` is oakum's directory after migrate. Only the old changesets
/// config file is still foreign.
fn remaining_removal(evidence: &str) -> Option<&str> {
    if evidence == ".changeset/" {
        return None;
    }
    if evidence.starts_with(".changeset/") && evidence != ".changeset/config.json" {
        return None;
    }
    Some(evidence)
}

/// One of three verdict lines plus the per-package differences; the
/// verdict itself is `migrate`'s.
pub(super) fn print_plan_comparison(
    comparison: &PlanComparison,
    before_label: &str,
    planned_by: &str,
    match_suffix: &str,
    planned: usize,
) {
    if let Some(diffs) = comparison.expected_knope_feature() {
        println!(
            "plan comparison: knope maps a pending feature on a pre-1.0 package to patch; oakum maps it to minor"
        );
        for diff in diffs {
            println!(
                "  {}: {} ({before_label}) vs {} (oakum)",
                diff.id(),
                format_versions(diff.before()),
                format_versions(diff.after()),
            );
        }
        return;
    }
    if let Some(parts) = comparison.unexpected() {
        println!("plan comparison: unexpected difference");
        for diff in parts.expected.iter().chain(parts.unexpected.iter()) {
            println!(
                "  {}: {} vs {}",
                diff.id(),
                format_versions(diff.before()),
                format_versions(diff.after()),
            );
        }
        return;
    }
    println!(
        "plan comparison: {planned} package(s) {planned_by} and by oakum; match{match_suffix}"
    );
}

#[cfg(test)]
mod pending_wording {
    use super::super::init::{OwnedPlan, ReadmeState, SchemaState};
    use super::pending_owned_line;

    fn plan(schema: SchemaState, readme: ReadmeState) -> OwnedPlan {
        OwnedPlan { schema, readme }
    }

    #[test]
    fn every_owned_file_state_has_its_sentence() {
        assert_eq!(
            pending_owned_line(plan(SchemaState::Absent, ReadmeState::Absent)),
            "write .changeset/_config.toml and .changeset/README.md, and write .changeset/_schema.json"
        );
        assert_eq!(
            pending_owned_line(plan(SchemaState::Stale, ReadmeState::Absent)),
            "write .changeset/_config.toml and .changeset/README.md, and replace the existing .changeset/_schema.json"
        );
        assert_eq!(
            pending_owned_line(plan(SchemaState::Current, ReadmeState::Absent)),
            "write .changeset/_config.toml and .changeset/README.md, and leave the current .changeset/_schema.json"
        );
        assert_eq!(
            pending_owned_line(plan(SchemaState::Absent, ReadmeState::Theirs)),
            "write .changeset/_config.toml and write .changeset/_schema.json (keeping the existing .changeset/README.md)"
        );
        assert_eq!(
            pending_owned_line(plan(SchemaState::Stale, ReadmeState::Ours)),
            "write .changeset/_config.toml and replace the existing .changeset/_schema.json (keeping the existing .changeset/README.md)"
        );
        assert_eq!(
            pending_owned_line(plan(SchemaState::Current, ReadmeState::Ours)),
            "write .changeset/_config.toml and leave the current .changeset/_schema.json (keeping the existing .changeset/README.md)"
        );
    }
}
