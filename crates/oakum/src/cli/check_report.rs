//! The `check --json` document: one run of `check` as data ([ADR-0036]).
//!
//! Every look the run could perform appears, so an empty refusal list reads as
//! "looked and found nothing" rather than "nobody looked" — the collapse
//! `AGENTS.md` forbids, and the one [ADR-0016] already corrected once when
//! `coverage_checked` became three values. The prose report and this document
//! are two renders of [`Verdict`]; neither is parsed from the other.
//!
//! [ADR-0036]: ../../../docs/decisions/0036-report-the-whole-check-run-as-data.md
//! [ADR-0016]: ../../../docs/decisions/0016-emit-release-state-render-it-never-deliver-it.md

use serde::Serialize;

use super::verdict::{first_line, Verdict};
use super::Outcome;

/// Bump when a consumer must distinguish shapes. Independent of the package
/// version: this answers "can I parse this", `tool_version` answers "what
/// wrote this". Frozen below 1.0.0 (ADR-0016, amended 2026-09-17).
pub(super) const SCHEMA_VERSION: u32 = 1;

/// What a look established. `Reported` is the state a refusals-only document
/// cannot express: the look ran, said something, and did not refuse — an
/// uncovered package under a run that does not gate, or a worktree read that
/// only partly succeeded. Folding it into `Ok` puts "we looked and found
/// nothing" and "we looked and could not finish" behind one word, which is the
/// collapse splitting `NotAsked` out of `Ok` was meant to end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum LookOutcome {
    Ok,
    Reported,
    Unverified,
    Error,
    NotAsked,
}

/// The one place an [`Outcome`] becomes a document word, so a variant cannot be
/// filed one way for stderr and another way here — the failure `CliError::class`
/// documents for its own table.
impl From<Outcome> for LookOutcome {
    fn from(outcome: Outcome) -> Self {
        match outcome {
            Outcome::Error => Self::Error,
            Outcome::Unverified => Self::Unverified,
        }
    }
}

/// One refusal a look raised. A look can raise several — `evaluate_coverage`
/// establishes an unmanaged-intent refusal and an uncovered-package refusal in
/// the same tail — and reporting only the first is the `also` prose gap this
/// document exists to close.
#[derive(Serialize)]
struct RefusalRow {
    outcome: LookOutcome,
    summary: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    detail: Vec<String>,
    /// Set on the refusal that decided the exit code, under ADR-0035. Two looks
    /// that raise a byte-identical refusal share one block, so it is reported
    /// under each of them and `deciding` is true on both: a consumer reading
    /// "the deciding refusal" must expect a set, not a single row.
    deciding: bool,
}

/// One look's row. `outcome` is the worst class it raised, so a caller can
/// branch on the row alone; `refusals` carries every one of them.
#[derive(Serialize)]
struct LookRow {
    look: &'static str,
    outcome: LookOutcome,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    refusals: Vec<RefusalRow>,
    /// What the look said without refusing. Present on a `Reported` row, and
    /// on a refusing row that also reported.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    reports: Vec<String>,
}

/// What the run covered, the half the prose report says on stdout. Without it
/// a document of clean rows cannot distinguish a run that examined one package
/// from one that examined none, and a consumer cannot attribute the coverage
/// verdict to a base it can name.
#[derive(Serialize)]
pub(super) struct ScopeRow {
    selected: usize,
    packages: usize,
    /// The ref the run diffed from, or `None` beside `base_unavailable` when it
    /// could not be named — never a ref the run does not have.
    #[serde(skip_serializing_if = "Option::is_none")]
    base: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_unavailable: Option<String>,
    /// Whether the coverage look gates or only reports (`--strict`).
    gating_coverage: bool,
}

impl ScopeRow {
    /// Built from the `Scope` the prose renders, by field name. Two adjacent
    /// `usize` in a positional constructor transpose silently — measured: the
    /// whole suite stayed green with `selected` and `packages` swapped.
    pub(super) fn of(
        selected: usize,
        packages: usize,
        base: Result<&str, &str>,
        gating_coverage: bool,
    ) -> Self {
        let (base, base_unavailable) = match base {
            Ok(reference) => (Some(reference.to_owned()), None),
            Err(why) => (None, Some(why.to_owned())),
        };
        Self {
            selected,
            packages,
            base,
            base_unavailable,
            gating_coverage,
        }
    }
}

/// One run, as the caller can read it without parsing prose.
#[derive(Serialize)]
pub(super) struct CheckReport {
    schema_version: u32,
    /// The binary that wrote this, for a reader holding the document later.
    tool_version: &'static str,
    /// What the run established: the deciding refusal's class under ADR-0035's
    /// ranking, or `reported` when nothing refused but a look still said
    /// something. `ok` and `reported` both exit `0`, so this is the exit code
    /// plus what was said, never less than it.
    outcome: LookOutcome,
    scope: ScopeRow,
    looks: Vec<LookRow>,
}

impl CheckReport {
    /// `asked` is every look this run performed, in announced order; `universe`
    /// is every look the command could perform. Both come from the plan, so a
    /// look added to the table reaches this document without a second edit.
    pub(super) fn of(
        verdict: &Verdict,
        scope: ScopeRow,
        asked: &[&'static str],
        universe: &[&'static str],
    ) -> Self {
        // A look this run performed but the universe does not name would
        // vanish from the document. Appended rather than asserted against:
        // `debug_assert` is compiled out of the binary users run.
        let looks = universe
            .iter()
            .chain(asked.iter().filter(|look| !universe.contains(look)))
            .map(|name| row(verdict, asked, name))
            .collect();
        Self {
            schema_version: SCHEMA_VERSION,
            tool_version: env!("CARGO_PKG_VERSION"),
            outcome: verdict
                .refusals()
                .find_map(|(block, deciding)| deciding.then(|| block.error.class().into()))
                .unwrap_or_else(|| {
                    // A run that said something without refusing is not a run
                    // that found nothing — the distinction `Reported` exists
                    // for, which the headline field would otherwise undo.
                    if verdict.said().next().is_some() {
                        LookOutcome::Reported
                    } else {
                        LookOutcome::Ok
                    }
                }),
            scope,
            looks,
        }
    }
}

fn row(verdict: &Verdict, asked: &[&'static str], name: &'static str) -> LookRow {
    if !asked.contains(&name) {
        return LookRow {
            look: name,
            outcome: LookOutcome::NotAsked,
            refusals: Vec::new(),
            reports: Vec::new(),
        };
    }
    let raised: Vec<(&super::verdict::Block, bool)> = verdict
        .refusals()
        .filter(|(block, _)| block.raised_by_look(name))
        .collect();
    // `Outcome`'s `Ord` is where ADR-0035 put the ranking; take the minimum
    // while the values are still `Outcome` rather than restating it here.
    let worst = raised.iter().map(|(block, _)| block.error.class()).min();
    let refusals: Vec<RefusalRow> = raised
        .iter()
        .map(|(block, deciding)| RefusalRow {
            outcome: block.error.class().into(),
            summary: first_line(&block.error.detail()),
            detail: block
                .error
                .detail()
                .lines()
                .skip(1)
                .map(str::to_owned)
                .chain(block.lines.iter().cloned())
                .collect(),
            deciding: *deciding,
        })
        .collect();
    let reports: Vec<String> = verdict
        .said()
        .filter(|(look, _)| *look == name)
        .map(|(_, line)| line.to_owned())
        .collect();
    let outcome = worst.map_or(
        if reports.is_empty() {
            LookOutcome::Ok
        } else {
            LookOutcome::Reported
        },
        LookOutcome::from,
    );
    LookRow {
        look: name,
        outcome,
        refusals,
        reports,
    }
}

#[cfg(test)]
mod tests {
    use super::{CheckReport, ScopeRow};
    use crate::cli::verdict::{carry, LookReport, Refusal};
    use crate::cli::CliError;

    fn scope() -> ScopeRow {
        ScopeRow::of(1, 2, Ok("HEAD~1"), false)
    }

    fn document(reports: Vec<(&'static str, LookReport)>) -> serde_json::Value {
        let verdict = carry(reports);
        serde_json::to_value(CheckReport::of(
            &verdict,
            scope(),
            &["one"],
            &["one", "two"],
        ))
        .expect("a document")
    }

    /// The wire words are the contract; a rename reaches a consumer even when
    /// no git fixture happens to exercise that row.
    #[test]
    fn the_wire_words_are_kebab_case_and_distinct() {
        let refusing = LookReport::refusing(vec![Refusal::bare(CliError::unverified(
            "unverified: could not look\nwhy",
        ))]);
        let value = document(vec![("one", refusing)]);
        assert_eq!(value["outcome"], "unverified");
        assert_eq!(value["looks"][0]["outcome"], "unverified");
        assert_eq!(
            value["looks"][0]["refusals"][0]["summary"],
            "could not look"
        );
        assert_eq!(value["looks"][0]["refusals"][0]["detail"][0], "why");
        assert_eq!(value["looks"][0]["refusals"][0]["deciding"], true);
        // A look outside `asked` is neither ok nor absent.
        assert_eq!(value["looks"][1]["look"], "two");
        assert_eq!(value["looks"][1]["outcome"], "not-asked");
    }

    /// Absent rather than null: a consumer reading `refusals` on a clean row
    /// should find nothing there, not an empty ceremony.
    #[test]
    fn a_clean_row_carries_neither_refusals_nor_reports() {
        let value = document(vec![("one", LookReport::default())]);
        assert_eq!(value["outcome"], "ok");
        assert_eq!(value["looks"][0]["outcome"], "ok");
        assert!(value["looks"][0]["refusals"].is_null());
        assert!(value["looks"][0]["reports"].is_null());
    }

    /// The scope block is what tells a run that examined one package from one
    /// that examined none, so its two counts must not be interchangeable.
    #[test]
    fn the_scope_keeps_its_two_counts_apart() {
        let value = document(vec![("one", LookReport::default())]);
        assert_eq!(value["scope"]["selected"], 1);
        assert_eq!(value["scope"]["packages"], 2);
        assert_eq!(value["scope"]["base"], "HEAD~1");
        assert!(value["scope"]["base_unavailable"].is_null());
    }

    /// The half no git fixture in the suite reaches: a base that could not be
    /// named renders the reason, never a ref the run does not have.
    #[test]
    fn a_base_that_cannot_be_named_carries_why() {
        let verdict = carry(vec![("one", LookReport::default())]);
        let value = serde_json::to_value(CheckReport::of(
            &verdict,
            ScopeRow::of(1, 1, Err("no commit yet"), true),
            &["one"],
            &["one"],
        ))
        .expect("a document");
        assert!(value["scope"]["base"].is_null());
        assert_eq!(value["scope"]["base_unavailable"], "no commit yet");
        assert_eq!(value["scope"]["gating_coverage"], true);
    }

    /// Two looks raising an identical refusal share one block, so the deciding
    /// marker appears under both. Honest — each look did raise it — but it means
    /// `deciding` is not a unique key, which a consumer has to know.
    #[test]
    fn a_shared_deciding_refusal_is_marked_under_every_look_that_raised_it() {
        let same = || CliError::unverified("unverified: shallow clone");
        let value = serde_json::to_value(CheckReport::of(
            &carry(vec![
                ("one", LookReport::refusing(vec![Refusal::bare(same())])),
                ("two", LookReport::refusing(vec![Refusal::bare(same())])),
            ]),
            scope(),
            &["one", "two"],
            &["one", "two"],
        ))
        .expect("a document");
        for row in value["looks"].as_array().expect("looks") {
            assert_eq!(row["refusals"][0]["deciding"], true, "{value}");
        }
    }
}
