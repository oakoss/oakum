# Carry the finding when several looks refuse at once

- Status: accepted
- Date: 2026-09-11
- Deciders: Jace Babin

## Context and Problem Statement

[ADR-0034](0034-exit-two-for-unverified.md) settles which number each `CliError` variant answers. It does not settle which variant a single run *carries* when more than one of its looks refuses, and `check` has seven that can. Which refusal should set the exit code when a run establishes a finding and also fails to make a look?

## Decision Drivers

- There was no rule, and the absence was a bug. `?` returned whichever refusal was written first in the source, so an unrelated leftover staging file turned a measured tag drift from `error` (exit 1) into `unverified` (exit 2) **and erased the `error:` line from stderr entirely** — measured on a fixture whose only delta was one `touch`.
- The distinction only pays for callers that branch on `1` versus `2`. For the common consumer — `oakum check` as a CI step — both are non-zero and the step fails identically.
- A number carries one fact. ADR-0034 already accepted this: *"The code classifies the outcome, not whether anything was written … What was written is in the message, where it can be said precisely; no exit code can carry it."*

## Considered Options

- **A finding outranks a look that did not happen**
- **A look that did not happen outranks a finding**
- **Keep source order** (the state this replaced)
- **A distinct code for "both"**

## Decision Outcome

Chosen option: **a finding outranks a look that did not happen**, because it is the branch a consumer can act on. Work the two plausible scripts against a run carrying both:

| script | finding wins | unverified wins |
| --- | --- | --- |
| retry on `2`, fail on `1` | exits `1`, names the drift | exits `2`, retries, fails identically — the drift never reaches the number |
| warn on `2`, block on `1` | exits `1`, blocks | exits `2`, warns, and a real drift ships |

Retrying cannot help a failure oakum measured, and warning through one is worse than useless. Ties among refusals of the same class keep source order, and every distinct refusal the run did not carry is still printed, prefixed `also`, so nothing is lost from the channel that can hold it.

### Consequences

- Good, because the exit code names something the caller can do, and a measured failure is that thing.
- Good, because a refusal is no longer erased by a sibling: `carry` prints every one, and the deciding line is the one without the `also` prefix.
- Bad, because in a mixed run the unverified outcome leaves the machine-readable channel and survives only in prose. This narrows ADR-0034's split — *"a CI step can now treat 'could not verify' differently from 'verified and failed'"* — in exactly the case where both are true. A caller that must see both needs a channel that can carry two facts; `check` has no `--json` today, and `okm-404.58` is where that goes.
- Bad, because the ordering is a second thing to keep true. It lives on `Outcome`'s `Ord` — `Error` before `Unverified` — so `CliError::class` remains the one place a variant is classified, and `carry` reads the ordering rather than restating it.
- Neutral, because the pure cases are unchanged: a run refusing only on findings exits `1`, and one refusing only on looks that did not happen exits `2`.

### Confirmation

`a_finding_outranks_an_unverified_look_for_the_exit_code` and `a_finding_before_an_unverified_look_in_source_order_still_decides` in `crates/oakum/tests/check.rs` pin the rule from both directions — the second because a test that places the finding *last* cannot tell "carry the highest-severity refusal" from "carry the last refusal". It places the finding first, so a `carry` returning the last refusal fails it (measured). `a_finding_that_loses_the_tie_is_still_reported` pins that an equal-severity loser still prints, and `a_sibling_refusal_does_not_discard_a_look_that_answered` pins that a look which answered is not thrown away when a sibling refuses.

Revisit if `check` grows a machine-readable channel, which would let both outcomes reach a caller without either losing.

## More Information

- [ADR-0034](0034-exit-two-for-unverified.md) — which number each variant answers, which this builds on
- [ADR-0016](0016-emit-release-state-render-it-never-deliver-it.md) — `status` reports and `check` decides, which is why only `check` needs this rule
