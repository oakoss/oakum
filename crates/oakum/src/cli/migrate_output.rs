//! What `migrate` prints. Rendering only; every decision and every look at
//! the tree stays in `migrate`.

use oakum::plan::{format_versions, PlanComparison};
use semver::Version;

use super::ci::VERSION_BRANCH;
use super::migrate_config::SourceConfig;
use super::owned_files::{ConfigSettings, OwnedPlan, ReadmeState, SchemaState, README_REL};
use super::tag_shape::{ReadableTemplate, TagShape};

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

/// What the existing tags settled, once the writes are done. Silent when the
/// repository has no tags and when the shape they settle is the default
/// `release` applies anyway — a line restating the default is noise.
pub(super) fn print_tag_shape(shape: &TagShape, written: Option<ReadableTemplate>) {
    if let Some(template) = written {
        println!(
            "carried over: `tag-format = \"{}\"` (derived from the existing tags)",
            template.as_str()
        );
        return;
    }
    // Only the look that failed is reported here. Tags oakum read but could not
    // explain are an action the reader owes, so they go in the remaining steps.
    if let TagShape::Unread(why) = shape {
        println!("not derived: `tag-format` (could not read the existing tags: {why})");
    }
}

/// The remaining step for tags oakum read and could not explain: nothing was
/// written, so the shape is the reader's to set before the first release.
fn undecided_tag_step(shape: &TagShape) -> Option<String> {
    let TagShape::Undecided { why, offerable } = shape else {
        return None;
    };
    // The reader is asked for a value, so the shapes this repository could
    // adopt travel with the ask; which of them is right depends on tags oakum
    // could not reconcile.
    let shapes: Vec<String> = offerable
        .iter()
        .map(|template| format!("`{}`", template.as_str()))
        .collect();
    Some(format!(
        "- set `tag-format` to match the existing tags ({why}); `release` refuses at the first tag rather than writing a shape the repository does not use\n  oakum reads {}",
        shapes.join(", ")
    ))
}

pub(super) fn print_pending(
    planned: &[(String, String)],
    sources: &[SourceConfig],
    settings: ConfigSettings,
    owned: OwnedPlan,
) {
    let carried = settings.private_packages;
    println!("pending:");
    for (path, _) in planned {
        println!("  rewrite {path}");
    }
    println!("  {}", pending_owned_line(owned));
    for source in sources {
        let file = source.file;
        if let Some(private) = source.carried_private_packages() {
            println!(
                "  carry {} from `{file}`",
                private
                    .axis_names()
                    .iter()
                    .map(|axis| format!("`privatePackages.{axis}`"))
                    .collect::<Vec<_>>()
                    .join(" and ")
            );
        }
        for key in &source.dropped {
            println!("  leave `{key}` behind in `{file}` (not carried)");
        }
    }
    // The line the write produces, once. Naming a whole line per source would
    // quote something no file receives when two sources contribute different
    // axes and the write is their union.
    if carried.any() {
        println!("  write `{}`", carried.toml_line());
    }
    // Derived from the repository rather than from a source config, in the
    // voice of the carried `privatePackages` line above.
    if let Some(template) = settings.tag_format {
        println!(
            "  carry the existing tag shape as `tag-format = \"{}\"`",
            template.as_str()
        );
    }
}

pub(super) fn print_left_alone(
    readme: ReadmeState,
    sources: &[SourceConfig],
    unreadable: &[String],
) {
    if readme == ReadmeState::Theirs {
        println!("kept {README_REL} (oakum did not write it; left as is)");
    }
    // Repeated here, not only where the file was reached: this is the one case
    // where a carried setting may have been lost, and the summary a reader
    // scrolls back to is where the record has to be.
    for line in unreadable {
        println!("{line}");
    }
    for source in sources {
        let file = source.file;
        if let Some(private) = source.carried_private_packages() {
            println!(
                "carried over: {} from `{file}`",
                private
                    .axis_names()
                    .iter()
                    .map(|axis| format!("`privatePackages.{axis}`"))
                    .collect::<Vec<_>>()
                    .join(" and ")
            );
        }
        for key in &source.dropped {
            // Deliberately silent about whether oakum has a counterpart. It
            // has one for some of these — `versionCommitMessage` is
            // `commit-message` — and claiming otherwise forecloses a question
            // the reader should still ask.
            println!("not carried over: `{key}` (`{file}` is untouched)");
        }
    }
}

pub(super) fn print_remaining_steps(
    detections: &[oakum::detect::Detection],
    knope: bool,
    foreign_changelogs: &[String],
    pinned: bool,
    npm: bool,
    binary: &Version,
    shape: &TagShape,
) {
    println!("remaining (oakum does not perform these):");
    for report in foreign_changelogs {
        println!("- {report}");
    }
    if let Some(step) = undecided_tag_step(shape) {
        println!("{step}");
    }
    // `migrate` writes `tool-version` and every later command refuses without
    // a matching pin, so a reader who installed globally would meet that
    // refusal with the migration already applied.
    if !pinned {
        let install = if npm {
            format!("pnpm add -D @oakoss/oakum@{binary}")
        } else {
            format!("cargo binstall --no-confirm oakum@{binary}")
        };
        println!(
            "- pin the same version as `tool-version` (`{binary}`): the workflow below carries one, or add `{install}` to this repository"
        );
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

/// One of four verdict lines plus the per-package differences; the
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
    // Reached only for `Equal`, so an empty plan here means both sides were
    // empty. A repository is usually migrated right after a release, which makes
    // this the common case rather than the edge one — and a comparison of two
    // empty plans proves nothing about the transform (`okm-404.6`).
    if planned == 0 {
        println!(
            "plan comparison: nothing pending under {before_label} or oakum; the transform was not exercised{match_suffix}"
        );
        return;
    }
    println!(
        "plan comparison: {planned} package(s) {planned_by} and by oakum; match{match_suffix}"
    );
}

#[cfg(test)]
mod pending_wording {
    use super::super::owned_files::{OwnedPlan, ReadmeState, SchemaState};
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
