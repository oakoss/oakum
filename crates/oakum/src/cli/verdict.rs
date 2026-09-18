//! One verdict from what the looks established: the blocks a run prints,
//! the deciding line first. Reports in, one `Verdict` out.

use super::CliError;

/// What one look established: what it reports without refusing, and each
/// refusal with the detail that supports it. The tag evaluation travels
/// beside this, from the one look typed to produce it.
#[derive(Default)]
pub(super) struct LookReport {
    pub(super) lines: Vec<String>,
    pub(super) refusals: Vec<Refusal>,
}

/// A refusal and the evidence beneath it: one block of the verdict.
pub(super) struct Refusal {
    pub(super) error: CliError,
    pub(super) lines: Vec<String>,
}

impl Refusal {
    pub(super) fn bare(error: CliError) -> Self {
        Self {
            error,
            lines: Vec::new(),
        }
    }
}

impl LookReport {
    pub(super) fn from_result(result: Result<(), CliError>) -> Self {
        Self::refusing(result.err().into_iter().map(Refusal::bare).collect())
    }

    pub(super) fn refusing(refusals: Vec<Refusal>) -> Self {
        Self {
            refusals,
            ..Self::default()
        }
    }
}

/// One refusal as the run established it: which looks raised it, what it says,
/// and the evidence beneath. Distinct from [`Refusal`], which is what a single
/// look hands in before the fold knows whether a sibling said the same thing.
pub(crate) struct Block {
    looks: Vec<&'static str>,
    pub(super) error: CliError,
    pub(super) lines: Vec<String>,
}

impl Block {
    fn raised_by(look: &'static str, refusal: Refusal) -> Self {
        Self {
            looks: vec![look],
            error: refusal.error,
            lines: refusal.lines,
        }
    }

    fn also_raised_by(&mut self, look: &'static str) {
        if !self.looks.contains(&look) {
            self.looks.push(look);
        }
    }

    pub(super) fn raised_by_look(&self, look: &str) -> bool {
        self.looks.contains(&look)
    }
}

/// What one run established, kept as data so the prose and the `--json`
/// document are two renders of it rather than one derived from the other's
/// text (ADR-0036).
pub(super) struct Verdict {
    /// Each report line with the look that said it. Keyed rather than
    /// flattened: a look that reported without refusing is neither a refusal
    /// nor a clean look, and a document that cannot tell them apart says `ok`
    /// for a look that only partly happened.
    said: Vec<(&'static str, String)>,
    blocks: Vec<Block>,
    /// Indexes `blocks`; `None` exactly when nothing refused. Private, so the
    /// index cannot be read back and dereferenced by a caller that might hold
    /// a stale one — [`Self::refusals`] pairs each block with its own flag.
    deciding: Option<usize>,
    pub(super) error: Option<CliError>,
}

impl Verdict {
    /// Report lines in announced order, each with the look that said it.
    pub(super) fn said(&self) -> impl Iterator<Item = (&'static str, &str)> {
        self.said.iter().map(|(look, line)| (*look, line.as_str()))
    }

    /// Every refusal with whether it decided the exit code.
    pub(super) fn refusals(&self) -> impl Iterator<Item = (&Block, bool)> {
        self.blocks
            .iter()
            .enumerate()
            .map(move |(index, block)| (block, self.deciding == Some(index)))
    }
}

/// Every refusal reported, and the one that decides the exit code chosen by
/// what it means rather than by where it sits in the source: a finding
/// outranks a look that did not happen, and among equals the announced order
/// decides. Each look is one block — its summary, then its detail beneath —
/// with the deciding block first and the rest marked `also`, so a reader
/// meets the verdict before what is subordinate to it, and evidence sits
/// under the line it supports. A look with detail and no refusal is a report,
/// returned for the caller to say before the verdict.
///
/// Measured before this shape: `?` carried out whichever refusal was written
/// first, so a stray staging file turned a tag drift from `error` into
/// `unverified` (ADR-0034's split run backwards); the `also` lines printed
/// before the line they were also-to; and a look's detail sat five lines from
/// its summary with three unrelated lines between.
pub(super) fn carry(reports: Vec<(&'static str, LookReport)>) -> Verdict {
    let mut said = Vec::new();
    let mut blocks: Vec<Block> = Vec::new();
    for (look, report) in reports {
        said.extend(report.lines.into_iter().map(|line| (look, line)));
        for refusal in report.refusals {
            // Two looks can fail identically — the tag look and the coverage
            // look both run `rev-parse --is-shallow-repository` — and `also`
            // reads as a second, different problem. Say it once, and keep the
            // evidence both brought, and both names: a merged block belongs to
            // every look that raised it, which is what the document reports.
            match blocks.iter_mut().find(|block| {
                block.error.class() == refusal.error.class()
                    && block.error.to_string() == refusal.error.to_string()
            }) {
                Some(block) => {
                    block.lines.extend(refusal.lines);
                    block.also_raised_by(look);
                }
                None => blocks.push(Block::raised_by(look, refusal)),
            }
        }
    }
    // `min_by_key` returns the first minimum, so equal-severity refusals keep
    // the announced order and the empty case is the `?`.
    let Some(deciding) = blocks
        .iter()
        .enumerate()
        .min_by_key(|(_, block)| block.error.class())
        .map(|(index, _)| index)
    else {
        return Verdict {
            said,
            blocks,
            deciding: None,
            error: None,
        };
    };
    let chosen = &blocks[deciding];
    let mut detail = first_line(&chosen.error.detail());
    indent_into(&mut detail, &continuation(&chosen.error.detail()));
    indent_into(&mut detail, &chosen.lines);
    for (index, also) in blocks.iter().enumerate() {
        if index == deciding {
            continue;
        }
        detail.push_str("\nalso ");
        detail.push_str(also.error.outcome());
        detail.push_str(": ");
        detail.push_str(&first_line(&also.error.detail()));
        indent_into(&mut detail, &continuation(&also.error.detail()));
        indent_into(&mut detail, &also.lines);
    }
    let error = chosen.error.recast(detail);
    Verdict {
        said,
        blocks,
        deciding: Some(deciding),
        error: Some(error),
    }
}

/// A summary is one line; whatever git said beneath it is detail like any
/// other, so a two-line refusal does not read as two blocks.
pub(super) fn first_line(detail: &str) -> String {
    detail.lines().next().unwrap_or_default().to_owned()
}

fn continuation(detail: &str) -> Vec<String> {
    detail.lines().skip(1).map(str::to_owned).collect()
}

fn indent_into(detail: &mut String, lines: &[String]) {
    for line in lines {
        detail.push('\n');
        if !line.is_empty() {
            detail.push_str("  ");
            detail.push_str(line);
        }
    }
}

/// An English list: comma-separated with a final `and`.
pub(super) fn named(looks: &[&str]) -> String {
    match looks {
        [] => String::new(),
        [only] => (*only).to_owned(),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

/// git's own diagnostics run to dozens of lines when it falls back to
/// `--no-index`, so the report takes one — but the verdict is the last
/// `fatal:`, not the first `warning:`. Measured on a corrupt loose object: the
/// first line is `error: inflate: data stream error`, while the lines it
/// preceded named the object and concluded `fatal: Not a valid commit name
/// main`.
pub(super) fn verdict_line(detail: &str) -> &str {
    detail
        .lines()
        .rev()
        .find(|line| line.starts_with("fatal:") || line.starts_with("error:"))
        .or_else(|| detail.lines().find(|line| !line.trim().is_empty()))
        // A blank-only diagnostic still owes one line, not the whole blob.
        .unwrap_or_else(|| detail.lines().next().unwrap_or(detail))
}

#[cfg(test)]
mod tests {
    use super::{carry, continuation, indent_into, LookReport, Refusal};
    use crate::cli::{CliError, Outcome};

    /// These tests exercise the fold, not look identity, so each report is
    /// named after its position — distinct names, so nothing merges by accident.
    fn reports_named(reports: Vec<LookReport>) -> Vec<(&'static str, LookReport)> {
        const NAMES: [&str; 4] = ["first", "second", "third", "fourth"];
        reports
            .into_iter()
            .enumerate()
            .map(|(index, report)| (NAMES[index], report))
            .collect()
    }

    /// A two-line refusal is one block: its continuation sits under its
    /// summary, indented like detail, so it cannot read as a second block.
    #[test]
    fn a_multi_line_refusal_stays_one_block() {
        let report = LookReport::refusing(vec![Refusal {
            error: CliError::unverified("unverified: first\nsecond"),
            lines: vec![String::from("detail")],
        }]);
        let verdict = carry(reports_named(vec![report])).error.expect("a refusal");
        assert_eq!(verdict.detail(), "first\n  second\n  detail");
    }

    /// A merged block belongs to every look that raised it. Dropping the second
    /// name makes that look read as one that passed.
    #[test]
    fn a_merged_block_names_both_looks() {
        let same = || CliError::unverified("unverified: same text");
        let first = LookReport::refusing(vec![Refusal::bare(same())]);
        let second = LookReport::refusing(vec![Refusal::bare(same())]);
        let verdict = carry(reports_named(vec![first, second]));
        let (block, _) = verdict.refusals().next().expect("one merged block");
        assert!(block.raised_by_look("first") && block.raised_by_look("second"));
    }

    /// An identical refusal from a second look is said once, and the evidence
    /// both looks brought survives under it — measured before the merge: the
    /// second look's lines were dropped with its duplicate summary.
    #[test]
    fn an_identical_refusal_merges_and_keeps_its_evidence() {
        let same = || CliError::unverified("unverified: same text");
        let first = LookReport::refusing(vec![Refusal {
            error: same(),
            lines: vec![String::from("from the first look")],
        }]);
        let second = LookReport::refusing(vec![Refusal {
            error: same(),
            lines: vec![String::from("from the second look")],
        }]);
        let verdict = carry(reports_named(vec![first, second]))
            .error
            .expect("a refusal");
        assert_eq!(
            verdict.detail(),
            "same text\n  from the first look\n  from the second look"
        );
    }

    /// A shadowed refusal's continuation sits under its `also` line too.
    #[test]
    fn a_shadowed_multi_line_refusal_stays_one_block() {
        let deciding = LookReport::refusing(vec![Refusal::bare(CliError::new("first"))]);
        let shadowed = LookReport::refusing(vec![Refusal::bare(CliError::unverified(
            "unverified: git failed\nfatal: why",
        ))]);
        let verdict = carry(reports_named(vec![deciding, shadowed]))
            .error
            .expect("a refusal");
        assert_eq!(
            verdict.detail(),
            "first\nalso unverified: git failed\n  fatal: why"
        );
    }

    /// Same words, different classes: a finding must not merge into an
    /// unverified look that happened to say the same thing, or exit 2 would
    /// hide exit 1.
    #[test]
    fn a_finding_does_not_merge_into_an_identical_unverified_look() {
        let look = LookReport::refusing(vec![Refusal::bare(CliError::unverified(
            "unverified: same words",
        ))]);
        let finding =
            LookReport::refusing(vec![Refusal::bare(CliError::new("unverified: same words"))]);
        let verdict = carry(reports_named(vec![look, finding]))
            .error
            .expect("a refusal");
        assert_eq!(verdict.class(), Outcome::Error);
        assert_eq!(verdict.detail(), "same words\nalso unverified: same words");
    }

    /// A look with detail and no refusal is a report, handed back to be said
    /// before the verdict; it is not a block.
    #[test]
    fn a_report_is_said_and_is_not_a_block() {
        let advisory = LookReport {
            lines: vec![String::from("changed with no covering intent")],
            ..LookReport::default()
        };
        let refusing = LookReport::refusing(vec![Refusal::bare(CliError::new("drift"))]);
        let verdict = carry(reports_named(vec![advisory, refusing]));
        assert_eq!(
            verdict.said().collect::<Vec<_>>(),
            vec![("first", "changed with no covering intent")]
        );
        assert_eq!(verdict.error.expect("a refusal").detail(), "drift");
    }

    /// A blank line inside a detail stays blank, not two spaces.
    #[test]
    fn a_blank_detail_line_carries_no_indent() {
        let mut detail = String::from("summary");
        indent_into(&mut detail, &continuation("summary\none\n\nthree"));
        assert_eq!(detail, "summary\n  one\n\n  three");
    }
}

#[cfg(test)]
mod conclusion {
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
