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

pnpm's documented rewrites make the protocol forms an explicit declaration rather than a convention: for a dependency at `1.5.0`, `workspace:*` publishes as an exact `1.5.0`, `workspace:~` as `~1.5.0`, and `workspace:^` as `^1.5.0`; a bare `workspace:` is treated as `workspace:*`. Choosing between them *is* the author declaring how far the dependent should follow. ([pnpm workspaces](https://pnpm.io/workspaces), verified 2026-08-18.)

| Declared | Cascades on |
|---|---|
| `workspace:*`, bare `workspace:`, or an exact pin | any bump |
| `~1.5.0` | minor and major |
| `^1.5.0` | major |
| `^0.1.3` | `0.2.0` — caret on `0.x` is stricter than on `1.x` |

That last row is not a corner case. It is the release-plz failure recorded as ADR-0027 in the prior design record: a caret range on a `0.x` version does not span a minor bump, so treating `^` as uniformly permissive ships a workspace that does not compile.

**Compare resolved versions, never manifest strings.** Yarn 4 emits `=1.5.0` where pnpm and bun emit `1.5.0` for the same `workspace:*` declaration. String comparison makes the same dependency look different depending on which package manager wrote the lockfile.

### Consequences

- Good, because no new config key, and the declaration cannot drift from the manifest it lives in
- Good, because it is correct for the `0.x` caret case that broke a real tool
- Bad, because an author who picked a range casually gets cascade behavior they did not think about — mitigated by `--explain`, which states the range that drove each decision
- Neutral, because it only governs libraries; delivery artifacts bypass the gate entirely under [ADR-0009](0009-delivery-artifacts-always-cascade.md)

### Confirmation

changesets implements this rule correctly and is the reference. **bumpy is wrong** — it treats `workspace:*` as a wildcard that always satisfies, so it never cascades where the pin is tightest, which is exactly backwards.

## Pros and Cons of the Options

### A per-bump-file `cascade:` block

- Good, because it is explicit at the moment of the change
- Bad, because it is a hand-maintained restatement of the dependency graph, written by whoever is closest to a deadline. Non-manifest edges belong in `_config.toml` once, not in every file that touches the package.

### A per-package config key

- Good, because it is stated in one place
- Bad, because it duplicates the range and can contradict it, and [ADR-0004](0004-derive-facts-configure-preference.md) rules it out on those grounds

## More Information

- [ADR-0008](0008-cascade-only-along-runtime-edges.md) — which edges reach this gate
- [ADR-0009](0009-delivery-artifacts-always-cascade.md) — what overrides it
- Dependents bump at **patch** by default, configurable `patch | minor | none`. release-please hardcodes patch and agrees; bumpy matches the triggering level, and its own ADR-0032 records matching-severity as a downside it accepted.
