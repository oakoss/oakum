# Read the declared range as the cascade preference

- Status: accepted
- Date: 2026-08-18
- Deciders: Jace Babin

## Context and Problem Statement

Given a runtime edge to a library ([ADR-0008](0008-cascade-only-along-runtime-edges.md)), when does releasing the dependency actually require releasing the dependent? A configuration key could answer it, but the manifest already carries an answer: the author chose a range, and that choice states how tightly the dependent tracks its dependency. Is a separate key needed?

## Decision Drivers

- [ADR-0004](0004-derive-facts-configure-preference.md) forbids a key that restates the repository
- Range syntax already differs deliberately between packages, so the intent is present
- Getting this wrong under-releases silently, which is the failure mode this project exists to fix

## Considered Options

- A per-package config key stating cascade strictness
- A per-bump-file `cascade:` block, as bumpy does
- Derive from the declared range

## Decision Outcome

Chosen option: **derive from the declared range**. One rule covers both workspace protocols and plain ranges:

> **Cascade when the dependent's published range would no longer resolve to the new version.**

"Published" is doing real work in that sentence. It means the range as of the last tag reachable from `HEAD`, not as of the working tree ([ADR-0014](0014-tags-are-the-version-source-of-truth.md)) — and those differ inside an open version pull request.

pnpm's documented rewrites make the protocol forms an explicit declaration rather than a convention: for a dependency at `1.5.0`, `workspace:*` publishes as an exact `1.5.0`, `workspace:~` as `~1.5.0`, and `workspace:^` as `^1.5.0`; a bare `workspace:` is treated as `workspace:*`. Choosing between them *is* the author declaring how far the dependent should follow. ([pnpm workspaces](https://pnpm.io/workspaces), verified 2026-08-18.)

| Declared | Cascades on |
|---|---|
| `workspace:*`, bare `workspace:`, or an exact pin | any bump |
| `~1.5.0` | minor and major |
| `^1.5.0` | major |
| `^0.1.3` | `0.2.0` — caret on `0.x` is stricter than on `1.x` |

That last row is not a corner case. It is the release-plz failure recorded in linesmith's ADR-0027 (`oakoss/linesmith`, `docs/adrs/0027-knope-for-release-automation.md`, accepted 2026-05-23): a caret range on a `0.x` version does not span a minor bump, so `linesmith-core ^0.1.3` excluded `0.2.0` and the release pull request shipped a workspace that did not compile. Treating `^` as uniformly permissive reproduces it.

**Compare resolved versions, never manifest strings.** Yarn 4 emits `=1.5.0` where pnpm and bun emit `1.5.0` for the same `workspace:*` declaration. String comparison makes the same dependency look different depending on which package manager wrote the lockfile.

### Consequences

- Good, because no new config key, and the declaration cannot drift from the manifest it lives in
- Good, because it is correct for the `0.x` caret case that broke a real tool
- Bad, because an author who picked a range casually gets cascade behavior they did not think about — mitigated by `--explain`, which states the range that drove each decision
- Neutral, because it only governs libraries; delivery artifacts bypass the gate entirely under [ADR-0009](0009-delivery-artifacts-always-cascade.md)

### Confirmation

changesets implements this rule correctly and is the reference. **bumpy is wrong in two places, both the same mistake.** It treats `workspace:*` as always satisfied, so it never cascades where the pin is tightest — exactly backwards, since pnpm publishes that form as an exact version. And it treats `catalog:` as always satisfied too — its docs say the catalog cannot be resolved for checking, but the shipped code says otherwise: `satisfies()` short-circuits with `if (range.startsWith("catalog:")) return true;` while the same package ships `loadCatalogs` and `resolveCatalogDep`, reading `pnpm-workspace.yaml`, `.yarnrc.yml`, and `package.json`. The resolver exists and the range check declines to use it. Both cases resolve an unread range as "no cascade," which is the silent under-release this project exists to catch. oakum resolves `catalog:` rather than waiving it; where a range genuinely cannot be resolved, the answer is an error naming the file, not a pass. (`@varlock/bumpy` 1.18.1 `dist/release-plan-*.mjs` and `dist/package-manager-*.mjs` from the registry tarball, read 2026-08-18 and re-checked 2026-08-19.)

**This rule covers out-of-range cascade only.** bumpy calls that Phase A and adds a Phase C — `updateInternalDependencies`, valued `out-of-range` (default), `patch`, or `minor` — that releases dependents whose ranges are still satisfied. oakum takes no position on Phase C. It is unconsidered scope rather than rejected scope, and [ADR-0009](0009-delivery-artifacts-always-cascade.md) is effectively a narrow hardcoded instance of it.

## Pros and Cons of the Options

### A per-bump-file `cascade:` block

- Good, because it is explicit at the moment of the change, and bumpy's version is glob-capable and exists mainly for **attribution** — cascaded packages are marked as dependency bumps rather than direct changes, which changes how they read in changelogs and PR comments
- Good, because it also bypasses the trigger threshold entirely — a per-file cascade always applies, which is the escape hatch for a relationship the ranges cannot express
- Bad, because as a *routine* way to express edges it is a hand-maintained restatement of the dependency graph, written by whoever is closest to a deadline. Non-manifest edges belong in `_config.toml` once, not in every file that touches the package.
- Rejected as the primary mechanism, not as a one-off override. The attribution problem it solves is real; [ADR-0032](0032-synthesize-cascade-changelog-line.md) is the answer.

**bumpy already ships the alternative this ADR prefers; it is not an oakum invention.** Its per-package config carries `cascadeTo` ("when I am bumped, cascade to these") and `cascadeFrom` ("when these are bumped, cascade to me"), both glob-capable, both taking `{ trigger, bumpAs }`, and both applying regardless of `updateInternalDependencies`. That is declared-once configuration for a non-manifest edge — the same shape oakum wants. The disagreement is only about which mechanism is the default, not about whether the mechanism should exist.

### A per-package config key

- Good, because it is stated in one place
- Bad, because it duplicates the range and can contradict it, and [ADR-0004](0004-derive-facts-configure-preference.md) rules it out on those grounds

## More Information

- [ADR-0008](0008-cascade-only-along-runtime-edges.md) — which edges reach this gate
- [ADR-0009](0009-delivery-artifacts-always-cascade.md) — what overrides it
- [ADR-0014](0014-tags-are-the-version-source-of-truth.md) — what "published" resolves to
- Dependents bump at **patch**. A later `bumpAs` key must be set; Patch remains the default ([ADR-0032](0032-synthesize-cascade-changelog-line.md)). release-please hardcodes patch and agrees.

**bumpy models this on two axes where oakum currently has one.** Each dependency type carries a `trigger` (how large a bump on the dependency sets off a cascade) and a `bumpAs` (how large a bump the dependent then gets), both configurable globally and per package:

| Type | `trigger` | `bumpAs` |
|---|---|---|
| `dependencies` | `patch` | `patch` |
| `peerDependencies` | `major` | `match` |
| `optionalDependencies` | `minor` | `patch` |
| `devDependencies` | disabled | disabled |

oakum derives the trigger from the declared range rather than configuring it, which is the whole point of this ADR — so only the `bumpAs` column was left open. bumpy's leftover is the `peerDependencies` exception: matching the triggering level rather than patching, on the stated grounds that a peer bump is proportional and that `^` on `0.x` breaks often enough for the distinction to matter. See [bump-file tool interfaces](../research/bump-file-tool-interfaces.md). [ADR-0032](0032-synthesize-cascade-changelog-line.md) parks that column: Patch stays the default; a later key must be set.

**Amended 2026-08-25 by [ADR-0032](0032-synthesize-cascade-changelog-line.md).** Attribution is a synthesized Changed line. Dependents stay `CascadeAs::Patch`. A later `bumpAs` key is config that must be set; Patch remains the default.
