# Own the plan engine rather than depending on changesets

- Status: accepted
- Date: 2026-08-18
- Deciders: Jace Babin

## Context and Problem Statement

changesets publishes its release math as separate npm packages — `@changesets/assemble-release-plan` 7.0.0, `@changesets/get-dependents-graph` 3.0.0, `@changesets/apply-release-plan` 8.0.0 (verified 2026-08-18). That is years of tested logic for the exact problem oakum solves. Depending on it would be cheaper than rewriting it. Should oakum?

## Decision Drivers

- The cascade rules in [ADR-0008](0008-cascade-only-along-runtime-edges.md) through [ADR-0010](0010-derive-cascade-from-declared-ranges.md) are the reason this project exists
- Cargo is a first-class target from v0, not a later addition ([ADR-0012](0012-scope-v0-to-version-math-and-the-github-layer.md))
- A Rust binary that depends on npm packages needs a Node runtime wherever it runs
- [ADR-0002](0002-single-crate-until-io.md) keeps `plan` pure so its dependency list enforces purity instead of code review

## Considered Options

- Depend on `@changesets/*` and drive it from the binary
- Port the algorithm to Rust, keeping its structure
- Own the plan engine outright

## Decision Outcome

Chosen option: **own it**.

The borrowed math is npm-shaped in the places that matter. An explicit caret means the same thing in both ecosystems — `^0.1.3` stops at `0.2.0` either way, which is why [ADR-0010](0010-derive-cascade-from-declared-ranges.md) states that rule without qualifying it by ecosystem. What differs is what a *bare* version means and what protocols exist alongside it: Cargo reads `linesmith-core = "0.1.3"` as `^0.1.3` ([ADR-0009](0009-delivery-artifacts-always-cascade.md) turns on exactly that), whereas npm reads a bare version as an exact pin, and the `workspace:` protocol is a pnpm, yarn, and bun feature that neither npm nor Cargo has ([ADR-0012](0012-scope-v0-to-version-math-and-the-github-layer.md)). Using the borrowed math for Cargo would mean synthesizing a fake npm `Packages` shape out of `Cargo.toml`, then overriding wherever those differences bite. That impedance layer has to be maintained forever, and it sits exactly on top of the code oakum is supposed to be better at.

The dependency also breaks the runtime story. A Rust binary reaching users through crates.io, Homebrew, and npm ([ADR-0021](0021-distribute-through-three-channels.md)) cannot require Node to compute a version.

### Consequences

- Good, because the differentiator is owned rather than patched onto someone else's model
- Good, because `plan` keeps no I/O-capable dependency, which is the split trigger [ADR-0002](0002-single-crate-until-io.md) is written against
- Bad, because well-tested logic gets reimplemented, and version math has non-obvious edge cases. The mitigation is the confirmation bar below rather than confidence
- Neutral, because changesets remains the reference to read and compare against; not depending on it does not mean ignoring it

### Confirmation

The same bar as [ADR-0012](0012-scope-v0-to-version-math-and-the-github-layer.md): reproduce linesmith's release history and all eight silent misses with no false positives. A reimplementation that cannot do that has not earned the decision.

## More Information

- [ADR-0005](0005-write-the-changeset-format-intersection.md) — the file format is still borrowed, even though the engine is not. Reading what changesets reads and computing differently is the whole position.
