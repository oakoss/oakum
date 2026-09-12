# Exit 2 when a verification could not be made

- Status: accepted
- Date: 2026-09-11
- Deciders: Jace Babin

## Context and Problem Statement

`AGENTS.md` carries one rule above the others: never collapse "we didn't look" into "it's fine." Every verification reports three outcomes — ok, unverified, error — and `CliError::Unverified` exists so those stay distinguishable in the type system.

They did not stay distinguishable at the process boundary. `main` exited `1` for every error, and the enum's own doc comment said so: *"Distinct variants so check outcomes stay distinguishable. All variants print and exit 1."* A caller reading only the exit code — a CI step, a shell script, another tool — saw the same number for "this repository failed the check" and for "the check could not be run." How should the third outcome reach a caller that is not parsing stderr?

## Decision Drivers

- The three-outcome rule is the project's own invariant; a channel that carries two of them is where the collapse happens.
- `migrate` made the cost concrete (`okm-404.3`): a cutover that wrote every file correctly and merely could not run the source tool for comparison exited `1` with `error: unverified: migrated files were kept`. That is the outcome of the *recommended* install — `npm i -g` and Homebrew put `oakum` on `PATH` but not a devDependency's binaries — so the common path reported failure for a migration that succeeded.
- Exit codes are the one machine-readable channel every caller already reads.
- Whatever is chosen has to be uniform. A word that means exit `2` under `migrate` and exit `1` under `check` is the drift this epic keeps filing bugs about.

## Considered Options

- **Exit 2 for `Unverified`, everywhere**
- **Exit 2 under `migrate` only**
- **Keep exit 1; distinguish in stderr text alone**
- **A distinct code per variant** (tag drift, uncovered, forbidden, …)

## Decision Outcome

Chosen option: **exit 2 for `Unverified`, everywhere**, because the distinction already exists in the type and only the last step discarded it. `CliError::exit_code` is now the single place that maps a variant to a number, and `Unverified` is the only variant that answers `2`.

The three outcomes are therefore: `0` ok, `2` unverified, `1` error.

A per-variant code was rejected as precision nobody asked for. Tag drift and uncovered packages are both *findings* — the repository is in a state the command refuses — and giving each its own number invents a vocabulary callers would have to learn to gain nothing. The split that carries meaning is between a finding and a look that did not happen.

### Consequences

- Good, because a CI step can now treat "could not verify" differently from "verified and failed" without parsing prose.
- Good, because the rule is uniform across `check`, `migrate`, `release`, `tag-drift`, `ci` and `reachable-tags`: one function decides. `CliError::exit_code`'s exhaustive `match` forces a new variant to state *a* code, and a source-scan test ties the enum's variant count to a table stating *which* — so a variant dropped into the wrong arm fails a test rather than shipping.
- Neutral, because every exit stays non-zero. `set -e`, `if ! oakum check`, and GitHub Actions' default step failure are all unaffected — they test zero against non-zero.
- Bad, because a script matching the exit code exactly (`[ $? -eq 1 ]`) stops matching on unverified outcomes. This is a breaking change and ships with a major bump note naming it.
- Bad, because `2` is conventionally "usage error" in some tool families. `oakum` has no usage-error code of its own — `clap` exits `2` on a parse failure before `main` ever sees a `CliError` — so the two do overlap. Both mean "this run did not answer your question"; neither means the repository failed a check, which is the distinction the code exists to draw.

The code classifies the *outcome*, not whether anything was written. `migrate` can exit `2` having applied every file and failed only to verify it, and can exit `2` having aborted before touching the tree — both mean "this run did not answer your question." What was written is in the message, where it can be said precisely; no exit code can carry it.

### Confirmation

`only_unverified_exits_two` in `crates/oakum/src/cli/mod.rs` asserts the code for every variant in the table, and `every_variant_states_an_exit_code` beside it fails when `CliError` gains a variant the table does not list — measured: adding one to the `=> 2` arm without a table row now fails, where the same perturbation left the whole bin suite green before the table existed. `assert_migrate_unverified_kept` in `crates/oakum/tests/migrate_cli.rs` asserts `Some(2)` at the process boundary across every migrate fixture that falls back to simulation, and `nothing_to_migrate_names_init` asserts `Some(1)` beside it. `shallow_clone_is_unverified` in `check.rs` and `tag_drift.rs`, and `no_token_cannot_confirm_a_tag_was_released` in `release_cli.rs`, assert the same code for their own commands — asserting only the word left the wiring unpinned in all three.

Revisit if `oakum` ever grows a usage-error code of its own, which would need a number that is not `2`.

## More Information

- [ADR-0035](0035-a-finding-outranks-an-unverified-look.md) — which refusal a run carries when several looks refuse at once, which this decision left open
- [ADR-0020](0020-one-precondition-path.md) — one precondition path, which is what makes a single exit-code rule reachable
- [ADR-0015](0015-layer-the-pr-status-channels.md) — the exit code from `check`, not the comment, is what fails a pull request
