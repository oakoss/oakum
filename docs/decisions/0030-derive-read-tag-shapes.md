# Derive read-side tag shapes; refuse leftover ambiguity

- Status: accepted
- Date: 2026-08-21
- Deciders: Jace Babin

## Context and Problem Statement

[ADR-0014](0014-tags-are-the-version-source-of-truth.md) reads the current version from tags reachable from `HEAD`. [ADR-0012](0012-scope-v0-to-version-math-and-the-github-layer.md) enumerates four tag shapes across the integration targets, three of them inside linesmith, plus two commits that each carry four tags. [ADR-0004](0004-derive-facts-configure-preference.md) already notes that the *write* template is preference and the *read* shapes are history. This record is the read rule.

How does oakum parse those tags into `(package, version)` without a config list of patterns?

## Decision Drivers

- Tag shapes in an existing repo are a fact of history, not a preference (ADR-0004 amendment of 2026-08-19)
- Cascade math is wrong if the last shipped version is the wrong package's
- A tag that cannot be attributed must not be treated as "no release" (three-outcome verification)
- `migrate` is the first command that runs against another tool's tags

## Considered Options

- **Derive known shapes, key by package, refuse leftover ambiguity**
- Accept a list of read patterns, separate from the write template
- Refuse any history that is not a single shape

## Decision Outcome

Chosen option: **derive known shapes, key by package, refuse leftover ambiguity.**

ADR-0012's four formats, parsed most-specific first. Scoped and unscoped `name@version` are two productions of one format:

1. `@scope/name@version` (changesets scoped)
2. `name@version` (changesets unscoped)
3. `name/v{semver}` (knope)
4. `name-v{semver}` (release-plz)
5. `v{semver}` (bare) — only when there is exactly one package candidate (the workspace has one package). In a multi-package workspace, a bare tag is ignored if package-prefixed tags on that commit already name the packages for that version; if it is the only evidence and more than one package could own it, the tag is leftover ambiguity, not a default assignment.

A tag that matches none of these and does not look like a version (`v1`, `nightly`) is ignored. A tag that looks like a version but matches no production (a fifth shape) is leftover ambiguity, not a missing history. Leftover ambiguity is **unverified**: name the tags, do not pick a winner, do not compute `0.1.0`.

Two names for the same `(package, version)` (linesmith's hyphen and slash pair for `linesmith-core` 0.2.0) are one release. Dedup after parse, not before.

The write template stays a single configured format. It does not have to be able to parse history.

### Consequences

- Good, because no config key restates the history (ADR-0004)
- Good, because linesmith's duplicate names collapse to one version per package
- Good, because `7219fa6`'s bare `v0.2.0` is ignored once the package-prefixed tags on that commit already name `linesmith` and `linesmith-core`
- Bad, because a genuine new shape that still looks like a version is unverified until a parser is added — the run fails instead of inventing `0.1.0`
- Neutral, because a package whose name is literally `v1.0.0` is unreadable as a scoped tag; that repo would have to be refused or migrated

### Confirmation

The rule is correct if, on the ADR-0012 enumeration **re-listed from GitHub 2026-08-21**:

- oakoss/linesmith: 19 tags, three formats. `7219fa6` carries `v0.2.0`, `linesmith/v0.2.0`, `linesmith-core/v0.2.0`, `linesmith-core-v0.2.0` — `linesmith-core` 0.2.0 once, not twice; bare `v0.2.0` does not assign a third package. `3bbde7f` carries `linesmith-v0.1.3`, `linesmith-core-v0.1.3`, `linesmith-plugin-v0.1.3`, `linesmith-plugin/v0.1.3` — three packages, not one. `v0.1.0` and `v0.1.1` are leftover ambiguity (sole tags on their commits; three package candidates); name them, do not assign, do not treat as never released.
- jbabin91/tt-packages-demo: 17 tags, 12 scoped `@jbabin91/mui-theme@…`, 5 `tt-package-demo-2@…` — not a knope split on `/`.
- oakoss/claude-plugins: 4 tags, all `name@version` (`pr-kit@0.1.0`, …).
- jbabin91/tsc-files: 24 tags, all bare `v*` (one package).
- pewter and finance-tracker: no tags.

And if a shallow clone with no tags still fails as "did not look," not as "never released" (ADR-0014).

Revisit if a fifth shape appears in a target repo: it must fail unverified, not look like never released.

## Pros and Cons of the Options

### Derive known shapes, key by package, refuse leftover ambiguity

- Good, because it matches the 2026-08-19 ADR-0004 amendment
- Good, because the four shapes are already enumerated, not discovered by clustering
- Bad, because a fifth shape is a code change, not a config edit

### A list of read patterns

- Good, because an unusual history can be taught without a release of oakum
- Bad, because the key restates history and will rot when a tag format is added by a migration
- Bad, because it is the option ADR-0004 exists to reject

### Refuse any mixed-shape history

- Good, because it is simple
- Bad, because every integration target except a green field would fail `migrate` on day one
- Bad, because linesmith's hyphen/slash pair is *the same* release, not a conflict

## More Information

- [ADR-0014](0014-tags-are-the-version-source-of-truth.md) — reachable tags; this fills its open "what format" question
- [ADR-0012](0012-scope-v0-to-version-math-and-the-github-layer.md) — the 64-tag / four-format / two four-tag-commit enumeration
- [ADR-0004](0004-derive-facts-configure-preference.md) — write template vs read shapes
- `okm-141` / `okm-0li` — this decision; then the parser
- knope's `PackageVersions::from_tags` is the write-shape reader, not a mixed-history reader
