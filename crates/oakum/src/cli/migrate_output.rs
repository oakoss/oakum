//! What `migrate` says — the printed steps and the text of the refusal a failed
//! look owes stderr. Rendering only: every decision and every look at the tree
//! stays in `migrate`, which chooses whether to raise what this module worded.

use oakum::plan::{format_versions, PlanComparison, Versioning};
use semver::Version;

use super::ci::VERSION_BRANCH;
use super::migrate::{BumpRewrite, GateLook, VersioningChoice};
use super::migrate_config::{chosen_commit_message, SourceConfig};
use super::owned_files::{
    commit_message_line, ConfigSettings, OwnedPlan, ReadmeState, SchemaState, README_REL,
};
use super::quoted;
use super::tag_shape::{ReadableTemplate, TagShape};
use super::CliError;

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
            "carried over: `tag-format = \"{}\"` (derived from the existing tags{})",
            template.as_str(),
            skipped_clause(shape)
        );
        return;
    }
    // Only the look that failed is reported here. Tags oakum read but could not
    // explain are an action the reader owes, so they go in the remaining steps.
    if let TagShape::Unread(why) = shape {
        println!("not derived: `tag-format` (could not read the existing tags: {why})");
    }
}

/// Tags the derivation stepped over, named on the line that reports what it
/// derived. A shape read off a subset while the rest went unmentioned would
/// claim a look at tags nobody weighed — the same collapse as reporting an
/// empty search as silence.
fn skipped_clause(shape: &TagShape) -> String {
    let TagShape::Derived { skipped, .. } = shape else {
        return String::new();
    };
    if skipped.is_empty() {
        return String::new();
    }
    format!(
        "; {} state no version and were not weighed",
        quoted(skipped)
    )
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
    Some(format!(
        "- set `tag-format` to match the existing tags ({why}); `release` refuses at the first tag rather than writing a shape the repository does not use\n  oakum reads {}",
        quoted(offerable.iter().map(|template| template.as_str()))
    ))
}

/// The `versioning` line, which is the one line in the written config a reader
/// cannot check by reading the config.
///
/// `semver` is the non-default ([ADR-0022](../../../../docs/decisions/0022-zero-major-versioning.md)),
/// it is what every source tool but knope implies, and below 1.0.0 it is the
/// difference between the next release being `0.18.0` and `1.0.0`. It was
/// written silently (`okm-404.9`).
pub(super) fn versioning_line(chosen: VersioningChoice) -> String {
    let versioning = chosen.versioning();
    let why = match (chosen, versioning) {
        (VersioningChoice::Requested(_), _) => {
            String::from("from `--versioning`, not the source tool")
        }
        (VersioningChoice::Inferred(from), Versioning::ZeroMajor) => {
            format!("{} holds a breaking change below 1.0.0", from.name())
        }
        (VersioningChoice::Inferred(from), Versioning::Semver) => format!(
            "{} takes 0.1.3 to 1.0.0, and renumbering an established release line is not a migration's job; oakum's own default is `zero-major`",
            from.name()
        ),
    };
    format!("  write `versioning = \"{versioning}\"` ({why})")
}

pub(super) fn print_pending(
    planned: &[BumpRewrite],
    sources: &[SourceConfig],
    settings: &ConfigSettings,
    owned: OwnedPlan,
    chosen: VersioningChoice,
) {
    let carried = settings.private_packages;
    println!("pending:");
    for rewrite in planned {
        // `rewrite` claims a transformation in place, which a file pulled out
        // of the old tool's directory does not get (`okm-404.4`).
        match rewrite.leftover() {
            Some(source) => println!("  write {} from {source}", rewrite.dest()),
            None => println!("  rewrite {}", rewrite.dest()),
        }
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
    println!("{}", versioning_line(chosen));
    // Resolved once, and shown in the form the file receives: the write is
    // first-wins across source files, so announcing each `Carried` value told a
    // reader two messages were written when one was.
    if let Some((file, message)) = chosen_commit_message(sources) {
        println!(
            "  carry `versionCommitMessage` from `{file}` as `{}`",
            commit_message_line(message)
        );
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
    if let Some((file, _)) = chosen_commit_message(sources) {
        println!("carried over: `versionCommitMessage` from `{file}` as `commit-message`");
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
            // Deliberately silent about whether oakum has a counterpart. It has
            // one for some of these, and claiming otherwise forecloses a
            // question the reader should still ask. The keys oakum does map are
            // reported by name elsewhere rather than through this line.
            println!("not carried over: `{key}` (`{file}` is untouched)");
        }
    }
}

/// Everything `migrate` saw and will not act on.
///
/// One value rather than nine arguments, because the list only grows: each is
/// something the command knows and the reader has to be told, and adding the
/// next one should not be a decision about argument order.
pub(super) struct Remaining<'a> {
    pub(super) detections: &'a [oakum::detect::Detection],
    pub(super) knope: bool,
    pub(super) foreign_changelogs: &'a [String],
    pub(super) pinned: bool,
    pub(super) npm: bool,
    pub(super) binary: &'a Version,
    pub(super) shape: &'a TagShape,
    /// Source bump files copied into `.changeset/`, originals still on disk.
    pub(super) leftovers: &'a [&'a str],
    pub(super) gates: &'a GateLook,
    /// Remaining steps the source configs left the reader, in print order.
    /// Not only lossy mappings: a setting oakum reproduces exactly is reported
    /// here too, so the reader is not told it was lost.
    pub(super) owed: &'a [String],
}

pub(super) fn print_remaining_steps(remaining: &Remaining<'_>) {
    let Remaining {
        detections,
        knope,
        foreign_changelogs,
        pinned,
        npm,
        binary,
        shape,
        leftovers,
        gates,
        owed,
    } = *remaining;
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
    if let Some(step) = leftover_bump_files_step(leftovers) {
        println!("{step}");
    }
    if let Some(step) = bump_file_gate_step(gates) {
        println!("{step}");
    }
    for step in owed {
        println!("{step}");
    }
    println!("- remove the old tool's dependency and its workflow");
    if knope {
        println!(
            "- `.changeset/README.md` aborts knope until `knope.toml` and its workflow are removed"
        );
    }
}

/// The bump files `migrate` copied rather than moved. `migrate` does not own the
/// old tool's directory ([ADR-0003]), so the originals stay — and the old tool
/// goes on counting them.
///
/// Measured in a clone (`okm-404.4`): after `migrate` and `version` consumed the
/// `.changeset/` copies and released `review-cycle` 0.17.0 to 0.18.0, `bumpy
/// status` still showed 3 pending and would take it to 0.19.0 a second time. A
/// workflow still wired to the old tool on every push to `main` makes that a
/// duplicate release on the merge commit, not a hypothetical.
///
/// [ADR-0003]: ../../../../docs/decisions/0003-write-only-what-a-command-owns.md
fn leftover_bump_files_step(leftovers: &[&str]) -> Option<String> {
    if leftovers.is_empty() {
        return None;
    }
    Some(format!(
        "- remove the bump files oakum copied out and left behind ({}); the old tool still counts them, so a workflow still wired to it releases the same packages a second time",
        quoted(leftovers)
    ))
}

/// What the gate search does not reach, on every line that reports its result.
/// The arm asserting a negative needs this most, and used to carry the least of
/// it: a reader told "nothing names it" acts on the sentence, not on the search
/// behind it.
const UNSEARCHED: &str = "oakum searched git's index outside `.changeset/`, so a gate that is untracked, an unstaged edit, inside a submodule, or in `.git/hooks/` is not covered — check those before the first release";

/// A gate this repository points at the old bump-file directory.
///
/// From the claude-plugins field record (`okm-404.24`): adopting oakum broke
/// that repository's own commit gate, and fixing it there took a pathspec
/// widening, a narrowing against oakum's skip list, a `:(glob)`, and six
/// regression tests. None of that is oakum's work — but the gate's existence is
/// the fifth thing `migrate` could see and did not say, beside
/// `private-packages`, `extra-files`, `commit-message` and `tag-format`.
///
/// A failed look says so. A look that found nothing says what it looked at:
/// `git grep` reads tracked files only, and the classic place for a commit gate
/// is `.git/hooks/`, which git never tracks — so silence here would claim a
/// search of the one directory the search cannot reach (`okm-404.35`).
fn bump_file_gate_step(gates: &GateLook) -> Option<String> {
    match gates {
        GateLook::NothingToRepoint => None,
        GateLook::Failed(why) => Some(format!(
            "- unverified: oakum {}; a commit or CI gate pointed at it will reject oakum's bump files",
            gate_look_failed(why)
        )),
        GateLook::NothingTracked => Some(format!(
            "- git reports no commit and its index lists no file, so no tracked file could have gated anything. {UNSEARCHED}"
        )),
        GateLook::Found(paths) if paths.is_empty() => Some(format!(
            "- no file in the index outside `.changeset/` names the old bump-file directory. {UNSEARCHED}"
        )),
        GateLook::Found(paths) => {
            Some(format!(
                "- check what names the old bump-file directory ({}); oakum cannot tell a commit or CI gate from a mention in prose, and a gate matching those paths rejects the bump files oakum writes. {UNSEARCHED}",
                quoted(paths)
            ))
        }
    }
}

/// The fact a failed gate look establishes, in the words both readers get. The
/// printed step and the refusal below were two separately-authored sentences
/// about one outcome, each free to drift from the other.
fn gate_look_failed(why: &str) -> String {
    format!(
        "could not look for files gating on the old bump-file directory ({})",
        one_line(why)
    )
}

/// What a failed gate look owes stderr. Built here beside the step rather than
/// in `migrate`, which decides only *whether* to raise it — so there is no
/// sentence for a caller to hand-write, and the two cannot drift into
/// describing different failures. Git's own diagnostic starts with `error:`,
/// which is why the detail is flattened rather than interpolated raw.
pub(super) fn gate_look_refusal(why: &str) -> CliError {
    CliError::unverified(format!(
        "unverified: migrated files were kept; oakum {}",
        gate_look_failed(why)
    ))
}

/// One bullet from a diagnostic that may span lines, so a reader parsing the
/// list by its leading dash does not meet a stray one. Lines repeated verbatim
/// collapse; lines that differ are kept, since each may name a different path.
fn one_line(detail: &str) -> String {
    let mut seen: Vec<&str> = Vec::new();
    for line in detail
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if !seen.contains(&line) {
            seen.push(line);
        }
    }
    seen.join("; ")
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
mod gate_steps {
    use super::super::migrate::GateLook;
    use super::{bump_file_gate_step, gate_look_refusal, one_line};

    /// The four outcomes must not render alike. An empty look is not silence:
    /// the search reads the index outside `.changeset/`, so silence would claim
    /// a search of `.git/hooks/`, which it never performed.
    #[test]
    fn every_outcome_has_its_own_line() {
        assert_eq!(bump_file_gate_step(&GateLook::NothingToRepoint), None);

        let empty = bump_file_gate_step(&GateLook::Found(Vec::new())).expect("empty look speaks");
        assert!(empty.contains("no file in the index"), "{empty}");
        // The arm asserting a negative carries the whole caveat, not half of it.
        for unsearched in ["untracked", "unstaged edit", "submodule", ".git/hooks/"] {
            assert!(empty.contains(unsearched), "{unsearched}: {empty}");
        }

        let nothing =
            bump_file_gate_step(&GateLook::NothingTracked).expect("an empty index speaks");
        assert!(nothing.contains("git reports no commit"), "{nothing}");
        assert!(
            nothing.contains(".git/hooks/"),
            "every arm carries the caveat: {nothing}"
        );
        assert_ne!(
            nothing, empty,
            "searched nothing is not searched and found none"
        );

        // One file is the common case, and `name it` read wrong there.
        let one = bump_file_gate_step(&GateLook::Found(Vec::from([String::from("hook.sh")])))
            .expect("a hit speaks");
        assert!(
            one.contains("check what names the old bump-file directory (`hook.sh`)"),
            "{one}"
        );
        assert!(
            one.contains("cannot tell a commit or CI gate from a mention in prose"),
            "a match is a mention, not a proven gate: {one}"
        );
        assert_ne!(one, empty);

        let two = bump_file_gate_step(&GateLook::Found(Vec::from([
            String::from("hook.sh"),
            String::from("gate.yml"),
        ])))
        .expect("two hits speak");
        assert!(two.contains("(`hook.sh`, `gate.yml`)"), "{two}");

        let failed =
            bump_file_gate_step(&GateLook::Failed(String::from("boom"))).expect("a failure speaks");
        assert!(failed.starts_with("- unverified:"), "{failed}");
    }

    /// Verbatim from a `git grep` over an unreadable tracked file: two lines,
    /// identical, inside what has to stay one bullet.
    #[test]
    fn a_multi_line_git_diagnostic_becomes_one_bullet() {
        let detail = "exit 1: error: failed to stat 'gate.sh': Permission denied\nerror: failed to stat 'gate.sh': Permission denied";
        assert_eq!(
            one_line(detail),
            "exit 1: error: failed to stat 'gate.sh': Permission denied; error: failed to stat 'gate.sh': Permission denied"
        );
        let step = bump_file_gate_step(&GateLook::Failed(String::from(detail))).expect("a line");
        assert_eq!(step.lines().count(), 1, "{step}");
    }

    /// The step and the refusal were separately-authored sentences about one
    /// outcome. They still read differently — one is a bullet, one sets the
    /// exit code — but the fact they state now comes from one place, so a
    /// reader cannot be told the look failed for two different reasons.
    #[test]
    fn the_step_and_the_refusal_state_one_fact() {
        let why = "exit 1: error: failed to stat 'gate.sh': Permission denied";
        let fact = "could not look for files gating on the old bump-file directory";
        let step = bump_file_gate_step(&GateLook::Failed(String::from(why))).expect("a step");
        let refusal = gate_look_refusal(why).to_string();
        for said in [&step, &refusal] {
            assert!(said.contains(fact), "{said}");
            assert!(said.contains(one_line(why).as_str()), "{said}");
        }
        assert!(
            refusal.starts_with("unverified: migrated files were kept"),
            "{refusal}"
        );
        assert_eq!(refusal.matches("unverified").count(), 1, "{refusal}");
    }

    /// Which arm refuses is the type's to say. Asked at two call sites by hand,
    /// it held only until one of them was edited.
    #[test]
    fn only_a_failed_look_is_a_failure() {
        assert_eq!(
            GateLook::Failed(String::from("boom")).failure(),
            Some("boom")
        );
        for looked in [
            GateLook::Found(Vec::new()),
            GateLook::Found(Vec::from([String::from("hook.sh")])),
            GateLook::NothingTracked,
            GateLook::NothingToRepoint,
        ] {
            assert_eq!(looked.failure(), None, "{looked:?}");
        }
    }
}

#[cfg(test)]
mod versioning_wording {
    use oakum::detect::ReleaseTool;
    use oakum::plan::Versioning;

    use super::super::migrate::VersioningChoice;
    use super::versioning_line;

    /// Every arm, and the rendered mode spelled the way the config file spells
    /// it — a line quoting a value the config does not contain would send a
    /// reader looking for a key that is not there.
    #[test]
    fn each_provenance_names_the_mode_and_what_settled_it() {
        assert_eq!(
            versioning_line(VersioningChoice::Inferred(ReleaseTool::Bumpy)),
            "  write `versioning = \"semver\"` (bumpy takes 0.1.3 to 1.0.0, and renumbering an established release line is not a migration's job; oakum's own default is `zero-major`)"
        );
        assert_eq!(
            versioning_line(VersioningChoice::Inferred(ReleaseTool::Knope)),
            "  write `versioning = \"zero-major\"` (knope holds a breaking change below 1.0.0)"
        );
        assert_eq!(
            versioning_line(VersioningChoice::Requested(Versioning::ZeroMajor)),
            "  write `versioning = \"zero-major\"` (from `--versioning`, not the source tool)"
        );
    }
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
