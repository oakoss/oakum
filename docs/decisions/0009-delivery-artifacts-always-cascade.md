# Delivery artifacts always cascade from a runtime dependency

- Status: accepted
- Date: 2026-08-18
- Deciders: Jace Babin

## Context and Problem Statement

A library whose published range still covers a dependency's new version does not need republishing — a consumer re-resolves at install time and gets the fix for free. Every tool that gates cascade on ranges relies on that. But a delivery artifact resolves its dependencies once, at build time, and ships them baked in. Nothing re-resolves afterward. Does the range gate apply to both?

## Decision Drivers

- A fix that is built must actually reach users
- Releasing a library whose consumers already resolve to the fix is pure noise
- Whatever distinguishes the two cases must be derivable, not declared per package
- This is the specific defect that motivated building oakum at all

## Considered Options

- Range-gate every cascade, matching the tools that gate at all
- Always cascade, ignoring ranges
- Range-gate libraries; always cascade delivery artifacts

## Decision Outcome

Chosen option: **range-gate libraries; always cascade delivery artifacts from a runtime dependency**.

This is the one rule no surveyed tool implements, and adopting the consensus would have reproduced the bug that motivated this project.

Applied to linesmith: the binary pins `linesmith-core = "0.1.3"`, which is `^0.1.3`. Core ships `0.1.4`. The range still resolves, so a range-gated cascade does **not** bump the binary. No bump means no tag, no tag means cargo-dist never runs, and the built artifact users download still contains `0.1.3`. That is the mechanism behind eight fixes that were released and never delivered — reconstructed exactly, not hypothesized.

**Derive which is which.** A package is a delivery artifact when the package manager reports it produces one: a `bin` target in `cargo metadata`, or a `bin` field in npm. Deriving from resolved targets rather than scanning for `src/main.rs` is what makes `autobins = false` and explicit `[[bin]]` entries come out right — see [workspace discovery](../research/workspace-discovery.md). This is correct for 100% of the packages in the repositories surveyed.

**One case needs a declaration**: a library that bundles its dependencies into its published output, which makes it a delivery artifact without producing a binary. That gets `resolves_dependencies_at = "build"` in config — named for the mechanism, never for the effect, so it does not read as a switch that turns cascading on.

### Consequences

- Good, because a dependency fix in a workspace with a binary actually reaches users
- Good, because libraries stay quiet, which is the behavior their consumers expect
- Bad, because a binary in a fast-moving workspace releases often. That is correct, not incidental: each release ships different bytes.
- Neutral, because it introduces one config key that is a fact about the ecosystem, not a preference — the exception [ADR-0004](0004-derive-facts-configure-preference.md) allows and names.

### Confirmation

Reproduce linesmith's history: all eight missed deliveries must be predicted, with no false positive against any library.

**Do not warn when a library skips a cascade.** It would fire on every correct library and become wallpaper. `--explain` states the reasoning for decisions *not* to bump, which is exactly what was missing when the eight fixes went undelivered — the tool was silent, and silence read as "nothing to do".

## Pros and Cons of the Options

### Range-gate everything

- Good, because it matches changesets, and is right for the library case
- Bad, because it silently under-releases every binary. The failure is invisible: the release succeeds, the changelog looks right, and the artifact is stale.

### Always cascade

- Good, because nothing is ever under-released
- Bad, because every patch to a shared library republishes its consumers for no change in their output

## More Information

- [ADR-0008](0008-cascade-only-along-runtime-edges.md) — which edges are eligible in the first place
- [ADR-0010](0010-derive-cascade-from-declared-ranges.md) — the range gate this rule overrides for artifacts
- [Workspace discovery](../research/workspace-discovery.md) — how targets are resolved
