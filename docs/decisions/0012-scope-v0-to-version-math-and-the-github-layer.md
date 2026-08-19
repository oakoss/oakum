# Scope v0 to version math and the GitHub layer

- Status: accepted
- Date: 2026-08-18
- Deciders: Jace Babin

## Context and Problem Statement

Existing release tools already publish to registries competently. What oakum has that they lack is the cascade rules in [ADR-0008](0008-cascade-only-along-runtime-edges.md) through [ADR-0010](0010-derive-cascade-from-declared-ranges.md). How much of the release pipeline should v0 own before it is worth using?

## Decision Drivers

- The differentiator is the planner; everything else is table stakes someone already built
- Publishing is irreversible, so it is the worst place to be learning
- A tool nobody can adopt incrementally will not get adopted
- Building the wrong thing for months is a real risk and needs a stopping rule

## Considered Options

- Plan only — emit a release plan, let existing tools act on it
- Version math plus the GitHub layer, stopping at the tag
- The full pipeline including registry publishing

## Decision Outcome

Chosen option: **version math plus the GitHub layer**, stopping at the tag, with promotion to publishing left open.

In scope for v0: the cascade planner, custom publish commands, include/exclude, non-interactive operation, commit generation, zero runtime dependencies, and no bundled GitHub Action. Optional: aggregated GitHub releases. Out: OIDC and staged publishing, prerelease channels, yarn and bun support, and plugin-based changelog formatters.

**No registry publishing at v0, but every adapter carries a `publish` slot**, so promotion is filling in a hole rather than reshaping the design. Stopping at the tag is [ADR-0011](0011-stop-at-the-tag.md); cargo-dist reacts to it.

**One tool across every project.** The alternative — a different tool per ecosystem — was rejected explicitly: "I don't have to remember changeset does it this way, bumpy does this, knope has this limit."

**Package managers, not ecosystems, are the adapter unit.** npm has no `workspace:` protocol at all (verified: `EUNSUPPORTEDPROTOCOL`); it uses plain ranges and symlinking. The protocol is pnpm, yarn, and bun only. Every repository surveyed declares `packageManager: pnpm`, so v0 is **pnpm and cargo**. Swift comes later, and its adapter writes `MARKETING_VERSION` rather than a package manifest.

### Consequences

- Good, because the irreversible operation is not in v0
- Good, because it can be adopted one repository at a time without replacing anything that works
- Bad, because it is not a complete release tool on its own and depends on cargo-dist downstream
- Neutral, because the `publish` slot means the boundary can move later without a redesign

### Confirmation

**The abort condition, agreed up front:** the planner must reproduce linesmith's release history and all eight silent misses, with no false positives, **within two weekends**. If it cannot, stop and upstream the cascade fix to an existing tool instead.

This exists because the failure mode of a project like this is not producing something bad — it is producing something adequate, slowly, that nobody would have chosen to build. Sunk cost is the enemy, so the exit is written down before any of it is spent.

## More Information

**Repository shapes, surveyed 2026-08-18.** These are the integration targets, and they are more varied than they look:

| Repository | Shape | Why it matters |
|---|---|---|
| linesmith | plugin → core → binary; plugin has two consumers | The only repository with a real runtime dependency graph. The eight silent misses happened here. |
| claude-plugins | 3 independent plugins, no dependency edges | Flat and boring, which makes it the safe first cutover |
| tt-packages-demo | pnpm + turbo; two published packages, no runtime edges between them, both devDepending on private `@repo/*` configs | The most common JS monorepo shape, and the [ADR-0008](0008-cascade-only-along-runtime-edges.md) test case |
| tsc-files | single published package, already on changesets v2 with auto-merge working | Auto-merge works there because the signed-commits ruleset is on the **oakoss org**, not the personal account — the signing problem is org-scoped, not universal |
| pewter | Xcode app, 0 tags, never released | Green-field; nothing to migrate |
| finance-tracker | deployed site; lives on main and deploys from main | Wants versions and tags so a bad deploy can be identified and rolled back to, with nothing published to a registry |

**Zero prerelease tags exist across all repositories** (64 tags total, re-counted 2026-08-19: linesmith 19, tsc-files 24, tt-packages-demo 17, claude-plugins 4, pewter and finance-tracker 0), which is why prerelease channels are out of scope rather than deferred.

**Those tags already carry four formats, and three of them are inside linesmith alone** (enumerated 2026-08-19): bare `v0.1.0`, hyphen-prefixed `linesmith-core-v0.1.3` from its release-plz era, slash-prefixed `linesmith-core/v0.4.1` since knope, and changesets' `name@version` in tt-packages-demo — where 12 of the 17 tags take the scoped form `@jbabin91/mui-theme@1.4.3`, whose leading `@`, embedded `/`, and second `@` defeat a naive split on either character and collide with the knope shape on `/`.

**Resolving those tags to commits is worse than the format count suggests.** Two linesmith commits carry four tags each. `7219fa6` answers to `v0.2.0`, `linesmith/v0.2.0`, `linesmith-core/v0.2.0`, and `linesmith-core-v0.2.0` — two packages across three formats on one commit — and `3bbde7f` answers to `linesmith-v0.1.3`, `linesmith-core-v0.1.3`, `linesmith-plugin-v0.1.3`, and `linesmith-plugin/v0.1.3`. So a bare-`v*` reader pointed at linesmith resolves `v0.2.0` and cannot say which package's release line it landed on, because that commit released two of them.

[ADR-0014](0014-tags-are-the-version-source-of-truth.md) derives the current version from exactly these tags, which makes reading them a precondition rather than a preference. **That is a gap in [ADR-0004](0004-derive-facts-configure-preference.md)'s split**, which lists tag formats under *configured* — true of the tag oakum writes, and not true of the tags it must read, where the formats are a fact of the repository's history and one template cannot cover three. Whether the read side derives the shapes, accepts a list, or refuses a history it cannot parse unambiguously is unsettled, and it lands on `migrate` first.

**The private, no-publish path is the most-used one, not an edge case.** finance-tracker and claude-plugins both live there, and so does a manual version-tag-release workflow at the user's workplace. A package that gets bumps, changelogs, tags, and GitHub releases but never touches a registry is therefore the path to test first and best — which is the opposite of how bumpy treats it, and where two of its 2026-08-18 defects live: the false "Published to npm" line recorded in [ADR-0004](0004-derive-facts-configure-preference.md), and private packages skipped by `findUnpublishedPackages` so they never get tagged — `if (pkg.private && !pkg.bumpy?.publishCommand) continue;` in `@varlock/bumpy` 1.18.1 `dist/publish-*.mjs` from the registry tarball, re-read 2026-08-19. Read it from `npm pack` rather than from a `node_modules` copy: claude-plugins carries a local patch rewriting exactly that function, so the defect is absent from the install where it was first hit.

**Migration order**, chosen so failures are cheap early: claude-plugins first, then three consecutive zero-manual-step releases, then shadow mode against linesmith, then cutover. The tool bootstraps via cargo-dist and a hand-cut tag until it can release itself — see [ADR-0007](0007-pin-the-tool-version-in-config.md)'s open question about self-hosting.

**Testing tiers.** Fixtures in this repository for algorithm correctness, using knope's `in/` + `out/` snapshot model: diamond dependencies, two consumers, a transitive chain, cycles that must error, private → public, `version.workspace = true`, and `workspace:*` / `catalog:`. Real repositories for integration, in the migration order above. Every repository is owned by the user, so cases can be constructed — planned: a new private package with a genuine **runtime** edge in tt-packages-demo, since every edge there today is a devDependency.
