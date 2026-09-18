# Report the whole `check` run as data, not just its verdict

- Status: accepted
- Date: 2026-09-17
- Deciders: Jace Babin

## Context and Problem Statement

[ADR-0035](0035-a-finding-outranks-an-unverified-look.md) decided that a finding outranks a look that did not happen, and named its own cost: *"in a mixed run the unverified outcome leaves the machine-readable channel and survives only in prose."* Its revisit trigger is this document — *"Revisit if `check` grows a machine-readable channel, which would let both outcomes reach a caller without either losing."* What shape should that channel take?

## Decision Drivers

- `AGENTS.md`'s first rule: never collapse "we didn't look" into "it's fine." A channel that carries only refusals cannot distinguish a look that ran and found nothing from one nobody asked for.
- The exit code carries one fact by construction. ADR-0034 accepted this: *"no exit code can carry it."*
- `check` already says all of this in prose, across two streams: `Scope` on stdout names the looks asked for and the looks not asked for, and each look's report goes to stderr.
- A second place to list the looks is a second place for them to disagree. The look table is already the single source the prose report is built from.
- [ADR-0016](0016-emit-release-state-render-it-never-deliver-it.md) holds `status` and `check` apart: `status` reports, `check` decides.

## Considered Options

- **Emit the whole run: scope, every look, and the verdict**
- **Emit the verdict alone** (deciding refusal plus shadowed ones)
- **Extend `ReleaseState`** with a verdict section
- **A third exit code for "both"**

## Decision Outcome

Chosen option: **emit the whole run**, as a versioned document behind `check --json`.

The verdict alone was the obvious reading of ADR-0035's cost, and it is not enough. A caller holding only refusals still cannot tell `coverage` ran and found nothing from `coverage` was never asked for — which is the collapse `AGENTS.md` forbids, reintroduced in a new channel. This project has made that mistake once already and recorded it: ADR-0016's own wire note says `coverage_checked` became `ran`/`failed`/`not-asked` because *"the boolean had one word for 'absent', so 'nobody asked' and 'we tried and failed' were the same value."* A refusals-only document is that boolean again, one level up.

Emitting the run costs nothing extra to compute. `Scope` already names the looks on both sides of the line, and `carry` already separates the deciding refusal from the shadowed ones; only the rendering flattens them. This is [ADR-0016](0016-emit-release-state-render-it-never-deliver-it.md)'s own principle — emit data, render text — applied to the verb that had only the text.

A third exit code was rejected for the reason ADR-0034 already gives: `2` collides with clap's usage error, so a `3` makes the numbering worse to carry a fact the number cannot hold anyway.

**`check --json` reports its own run, not the repository's state.** That is what keeps ADR-0016's split intact: `status` still owns "what is pending and at what version", and nothing here lets `check` answer that question. A caller wanting both runs both.

**The document replaces the prose report rather than joining it.** `Scope` is written to stdout, so a document written beside it would interleave two formats on one stream. `status --json` already established the pattern — *"Print the versioned `ReleaseState` JSON document instead of a render"* — and `check --json` follows it. Refusals continue to reach stderr unchanged, so `oakum check --json > run.json` yields a clean document and a human-readable failure from one invocation.

**The document is built from the look table.** Each entry is keyed by the `name` already on `Look`, and the set comes from the same `names()` the prose report uses. A look added to the table reaches the document without a second edit, and a test fails if one does not.

**`--json` does not change the exit code.** ADR-0035's ranking still decides it. The document is additive: `if ! oakum check` keeps working, and a caller that wants the shadowed outcome reads it from the document rather than from a number that cannot hold two.

**A run that never reached the looks emits no document.** The document describes what the looks established, so a config that failed to parse has nothing to describe; that run refuses on stderr and writes nothing to stdout, as it does today. The alternative — always emit, with an empty look list — trades a parsing cliff for a document that claims to have looked.

### Consequences

- Good, because both outcomes ADR-0035 forced apart now reach a caller: the deciding one and every shadowed one, each keyed by the look that raised it.
- Good, because a clean run is informative rather than empty — the document names what was checked, which is the difference between "nothing was wrong" and "nothing was examined."
- Good, because a look that reported without refusing has its own word. `ok` covering both "looked and found nothing" and "looked, said something, did not gate" would be the `coverage_checked` boolean again one level down, so the report lines travel keyed by their look and such a row reads `reported`.
- Good, because the look table stays the single source: the prose report and the document are two renders of one list.
- Bad, because it is a second versioned wire document to keep honest, with its own `schema_version`. [ADR-0016](0016-emit-release-state-render-it-never-deliver-it.md)'s amendment on pre-1.0 correction bounds that cost.
- Bad, because `check` now has a reporting surface, which ADR-0016 deliberately kept in `status`. The boundary held here is narrow and stated: `check --json` reports the run it performed, never the release state.
- Neutral, because `deciding` is not a unique key: two looks raising a byte-identical refusal share one block, so it is reported under each and marked deciding on both. Each look did raise it, so removing it from one would misreport which looks refused.
- Neutral, because nothing outside oakum consumes it yet, exactly as ADR-0016 records for `status --json`.

### Confirmation

A test asserts that every look in `LookPlan::check`'s table appears in the document, so a seventh look cannot reach the prose report and miss the document — the failure mode ADR-0034 describes for its own table, where *"the test that was supposed to catch the difference counted table rows rather than the variants in them."* A second drives a run that refuses on both a finding and a look that did not happen, and asserts the document carries both while the exit code carries the finding — the mixed run ADR-0035 could not serve.

Two more exist because review measured the first implementation failing them. `both_refusals_from_one_look_reach_the_document` covers one look raising several refusals, which `evaluate_coverage` does in a single tail: the mixed-run test uses two *different* looks, so it passed while that case dropped the second refusal entirely and the document was strictly less informative than the stderr it replaced. `a_run_that_only_could_not_look_says_so_in_the_document` pins the top-level outcome for an unverified-only run, without which `unverified` and `ok` were interchangeable to the suite — the pinning `AGENTS.md` asks for by name: *"a test drives the run that could not check X and asserts it reads differently from the run that checked and found nothing."*

Revisit if anything outside oakum consumes the document, which would end the pre-1.0 freedom to correct its shape in place.

## More Information

- [ADR-0035](0035-a-finding-outranks-an-unverified-look.md) — the ranking whose cost this pays, and whose revisit trigger this is
- [ADR-0034](0034-exit-two-for-unverified.md) — the three outcomes and why a number carries one fact
- [ADR-0016](0016-emit-release-state-render-it-never-deliver-it.md) — emit data, render text; `status` reports and `check` decides
- [ADR-0020](0020-one-precondition-path.md) — one precondition path, which is what makes one document per run reachable
