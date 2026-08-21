# Derive facts from the repository; configure only preference

- Status: accepted
- Date: 2026-08-18
- Deciders: Jace Babin

## Context and Problem Statement

Which things belong in a config file, and which should be read from the repository on every run?

## Decision Drivers

- Config that describes the repository goes stale the moment the repository changes, and nothing detects it
- The tool must not require upkeep to stay correct

## Considered Options

- Conventional configurability — let users declare relationships, publishability, and release targets
- Split by kind: derive anything the repository already states; configure only what it does not

## Decision Outcome

Chosen option: **derive facts, configure preference**.

**Derived every run**: what depends on what, whether a package is publishable, whether it needs a bump, whether it is a delivery artifact or a library, and which package manager governs the workspace.

**Configured**: templates, titles, tag formats, commit messages, changelog shape. These describe *output*, not the repository, so they cannot become inconsistent with it.

**Amended 2026-08-19: the tag-format entry covers the tag oakum writes, not the tags it reads.** [ADR-0014](0014-tags-are-the-version-source-of-truth.md) derives the current version from existing tags, and [ADR-0012](0012-scope-v0-to-version-math-and-the-github-layer.md) enumerates four formats across the integration targets with three inside linesmith alone. Those shapes are a fact of the repository's history, so they fall on the derived side of this split rather than the configured side.

**Amended 2026-08-21:** the read rule is [ADR-0030](0030-derive-read-tag-shapes.md): parse the four known shapes, key by package, refuse leftover ambiguity. No list of read patterns.

Every failure examined while designing this came from configuration restating a derivable fact. knope's scope-to-package table restates the dependency graph, so it rots when a crate is added. bumpy assigns an npm publish target to a private package instead of reading `private: true`, then skips it at publish, leaving a status that never resolves and a false "Published to npm" line in the release notes. A `gitUser` setting restates an identity the token already carries.

A related consequence: the author's *ranges* are already a declaration. pnpm resolves `workspace:*` to an exact pin, `workspace:~` to a tilde range, and `workspace:^` to a caret range — so choosing between them is the author stating how far a dependency may drift before the dependent needs a release. That is read, not asked.

### Consequences

- Good, because config never needs updating in response to a repository change
- Good, because it gives a clear test for any proposed key: does it describe the repository, or the output?
- Bad, because derivation must be correct in cases a config key could have papered over; `--explain` exists so a wrong derivation is visible rather than mysterious
- Neutral, because a small number of genuinely underivable facts remain, such as whether a library bundles a dependency at build time. Those are named for the mechanism (`resolves-dependencies-at = "build"`), never for the effect.
