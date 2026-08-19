# Stop at the tag; do not roll back

- Status: accepted
- Date: 2026-08-18
- Deciders: Jace Babin

## Context and Problem Statement

A release can fail partway: some packages published, some tags pushed, a downstream workflow never triggered. Should oakum attempt to undo what it did? And how far past the tag should it reach — does it own the artifacts a tag triggers?

## Decision Drivers

- Nothing published, tagged, or released if a precondition fails ([ADR-0003](0003-write-only-what-a-command-owns.md)'s ordering, applied to the release path)
- A registry publish is not reversible, so "undo" is a promise that cannot be kept
- The user already has working artifact tooling; duplicating it is scope, not value

## Considered Options

- Roll back on failure — unpublish, delete tags, revert the version commit
- Stop at the tag, report precisely what completed, and leave recovery to the user
- Own the whole pipeline through artifact publication

## Decision Outcome

Chosen option: **stop at the tag, and never roll back**.

Oakum owns version math, changelogs, the version pull request, and the tag. cargo-dist reacts to the tag and owns everything after it. That boundary is deliberate and is why [ADR-0007](0007-pin-the-tool-version-in-config.md)'s pin needs verifying rather than owning — oakum does not write the workflow that consumes its tag.

**Rollback is out of scope, permanently.** A crates.io publish cannot be undone; yanking is a different operation with different semantics, and npm unpublish is time-limited and disruptive to consumers. A tool that deletes tags to "clean up" destroys the only durable record of what happened. Tags are what oakum manages, and reverting a release is the user's call — they have more context about what shipped and to whom than the tool ever will.

What replaces rollback is **not needing it**: preflight the entire set before publishing anything — credentials valid, every target version absent, every manifest parseable. Most partial-failure scenarios become a clean abort with nothing to recover. When something does fail mid-run, stop, and report exactly which packages published and which did not, so the remaining work is a short explicit list rather than a diff against a registry.

### Consequences

- Good, because the tool never destroys a record of what shipped
- Good, because the failure report is actionable without querying registries whose reads are stale by design
- Bad, because a partial release leaves the user with manual steps. That is the honest state of the world; a tool claiming otherwise would be lying about an irreversible operation.
- Neutral, because it keeps oakum's surface small enough that the tag handoff is the only integration point to verify

### Confirmation

A tag whose downstream workflow could not be confirmed is reported as `unverified`, never as `ok`. Three states, not two — see [downstream handoff](../research/downstream-handoff.md).

## More Information

- [Registry publish semantics](../research/registry-publish-semantics.md) — why "already published" is not machine-distinguishable on npm, why `cargo publish --workspace` is explicitly non-resumable, and how seven other tools behave here
- [ADR-0007](0007-pin-the-tool-version-in-config.md) — the pin that makes the handoff verifiable
