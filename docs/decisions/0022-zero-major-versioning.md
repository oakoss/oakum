# Default to zero-major versioning below 1.0.0

- Status: accepted
- Date: 2026-08-19
- Deciders: Jace Babin

## Context and Problem Statement

A package below `1.0.0` has made no compatibility promise. Under strict semver a `major` change file still takes `0.1.3` to `1.0.0`, which declares the API stable as a side effect of describing one breaking change. Every repository that would receive a fresh `init` is pre-1.0 today. What should a breaking change do while a package is below `1.0.0`, who decides, and how does a project stop doing it?

## Decision Drivers

- Semver 2.0.0 §4: *"Major version zero (0.y.z) is for initial development. Anything MAY change at any time. The public API SHOULD NOT be considered stable."* Its FAQ recommends the cadence directly: *"start your initial development release at 0.1.0 and then increment the minor version for each subsequent release."*
- Most of [ADR-0012](0012-scope-v0-to-version-math-and-the-github-layer.md)'s integration targets are below 1.0.0: oakum `0.0.0`, linesmith `0.4.1`, the three claude-plugins at `0.14.0`, `0.1.0` and `0.1.0`, tsc-files `0.8.4`, pewter unreleased, finance-tracker `0.0.0`. **tt-packages-demo is not** — `@jbabin91/mui-theme` is at `1.4.3` and `tt-package-demo-2` at `1.4.1` (npm registry, 2026-08-19). It arrives through `migrate` from changesets, which writes `semver`, so the exception is already handled rather than ignored
- Reaching 1.0.0 is a product decision, not something a change file written on a Tuesday should trigger
- A tool built for these repositories should be correct for them without configuration
- Renumbering an existing project's release line during a migration is a surprise a migration exists to prevent

## Considered Options

- Strict semver only — a breaking change always produces `1.0.0`
- Zero-major as an opt-in key, defaulting to strict semver
- Zero-major as the default, with an explicit override

## Decision Outcome

Chosen option: **zero-major as the default**, expressed as `versioning = "semver" | "zero-major"` and defaulting to `zero-major`.

Below `1.0.0` a `major` change file produces a minor bump: `0.1.3` becomes `0.2.0`. At or above `1.0.0` the setting does nothing. The key is repository-wide by default and evaluated per package against that package's own current version, so a workspace holding both a `1.x` package and a `0.x` one behaves correctly with no override — the setting is inert for the first. A package may still override it; see *Graduating to 1.0.0*.

**The default is opinionated because the opinion is the spec's.** Semver says `0.y.z` carries no stability promise and its FAQ recommends exactly this cadence. A project being initialized has no release history, so it is in that range by definition, and so is every repository here but tt-packages-demo. Requiring a key to get the behavior the spec recommends would be configuration for its own sake.

**A feature does not become a patch.** Mapping `minor` to a patch bump below 1.0.0 is a separate behavior and is not adopted. Zero-major already collapses breaking into minor; collapsing feature into patch as well would leave the version number saying almost nothing in the range where these repositories live. It also suppresses cascades for `^X.Y.Z`, the default range form in both ecosystems — see [version-format constraints](../research/version-format-constraints.md) for the comparison across range shapes. This is a judgment call rather than a proof: a dependent declaring `^0.1.3` genuinely does accept `0.1.4`, so the suppression follows their declaration.

### The override reaches the CLI, not just the file

`oakum init --versioning <value>` writes the key, and `init` writes it explicitly even when it is the default — an invisible default cannot be read, and [specs/init.md](../specs/init.md) requires the repository be left able to explain itself. The `--interactive` wizard may ask the same question, but every answer it can produce is reachable as a flag, so an agent or a CI run can produce byte-identical config without a terminal.

**`init` applies oakum's opinion; `migrate` preserves the repository's.** A new repository gets `zero-major`. A repository migrating from changesets or bumpy gets `semver`, because those tools take `0.1.3` to `1.0.0` and silently renumbering an established release line is not a migration's job. One migrating from knope gets `zero-major`, which is what it was already doing. The flag overrides either.

### Graduating to 1.0.0

Nothing flips automatically, and nothing can: while `zero-major` is in effect and a package is below `1.0.0`, every breaking change yields `0.N+1.0`, so `1.0.0` is unreachable by construction. **Setting `versioning = "semver"` is the graduation.** The next `major` change file then produces `1.0.0`, and the key goes inert.

**A multi-package repository needs to graduate one package at a time, so the key takes a per-package override.** linesmith is the case: `linesmith` and `linesmith-core` at `0.4.1`, `linesmith-plugin` at `0.1.3`, one configuration file. Cutting `linesmith 1.0.0` by flipping the repository-wide key would at the same moment take `linesmith-plugin` off zero-major, so its next breaking change would yield `1.0.0` instead of `0.2.0` — nobody asked for that and nothing would announce it. This ADR introduces that shape. [ADR-0009](0009-delivery-artifacts-always-cascade.md)'s `resolves-dependencies-at` is precedent only for config having per-package granularity at all — it has no repository-wide value to override, and ADR-0009 classifies it as a fact about the ecosystem where `versioning` is preference. A default-plus-override for a preference key is new here.

Per-package settings live in a table keyed by the package name exactly as its manifest declares it, which is also where `resolves-dependencies-at` belongs:

```toml
versioning = "zero-major"

[packages."linesmith"]
versioning = "semver"
```

That shape is what the generated `_schema.json` describes, so an editor validates it. There is no flag for the per-package value: `--versioning` sets the repository-wide default at `init` time, and graduating a single package later is a config edit in the pull request that cuts its `1.0.0` — the same reviewable act the repository-wide graduation is.

A change file cannot express this instead — [specs/bump-files.md](../specs/bump-files.md) permits no key that is not a package in the workspace, so there is no per-release version override under [ADR-0005](0005-write-the-changeset-format-intersection.md)'s parser intersection. The config edit is the only mechanism, which is what makes it the right one: it lands in the pull request that cuts `1.0.0`, next to the release it caused.

Three obligations come with the default, and they are its cost rather than polish:

- **The generated `_schema.json` description states the whole lifecycle**, including that setting `semver` is how a project releases `1.0.0`. That description is what an editor shows to someone editing the config, which is where the question gets asked.
- **`--explain` covers the setting rather than `check` reporting on it.** An earlier draft had `check` announce when the setting was inert; that would fire on every correct `1.x` package on every run, which is the wallpaper failure [ADR-0009](0009-delivery-artifacts-always-cascade.md) names. `check` holds no prior state and cannot see a transition, so there is nothing for it to report that is drift.
- **`--explain` names the setting** whenever it produced a version the change file would not otherwise have produced.

### Consequences

- Good, because a project can describe a breaking change honestly without declaring itself stable, and gets that without configuring anything
- Good, because the version string costs nothing: it is ordinary semver, both registries accept it, and tags are unaffected
- Good, because it agrees with knope below 1.0.0, which is what linesmith runs — so [ADR-0012](0012-scope-v0-to-version-math-and-the-github-layer.md)'s shadow comparison does not diverge on every breaking change. That is a consequence of the decision, not a reason for it
- Bad, because it **does** change the cascade for some dependents. A **library** dependent whose range is bounded only at `1.0.0` — `^0`, `0.x`, Cargo's bare `"0"`, `>=0.1.3 <1.0.0` — admits `0.2.0` and rejects `1.0.0`, so it stops being released. Delivery artifacts are unaffected: [ADR-0009](0009-delivery-artifacts-always-cascade.md) cascades them regardless of range. It follows the author's declaration, but it is a behavior change and `--explain` has to surface it
- Bad, because someone who writes `major` expecting `1.0.0` gets `0.2.0`. `--explain` has to name the setting as the reason, or this becomes the silent surprise the project exists to avoid
- Bad, because rejecting feature-to-patch means oakum still diverges from knope on every *feature* in a pre-1.0 repository, where knope bumps the patch and oakum bumps the minor. The shadow comparison has to expect that difference rather than read it as a defect
- Neutral, because a package already at or above 1.0.0 ignores the setting, so a workspace that is already mixed behaves correctly without one

### Confirmation

Revisit if a repository wants features to bump patch below 1.0.0 as well. That is the behavior knope and release-please both offer and this decision declines; wanting it would mean the information loss was judged acceptable, which is a different decision rather than an extension of this one.

## Pros and Cons of the Options

### Strict semver only

- Good, because it is the ecosystem default — four of five surveyed tools do it, and someone arriving from changesets or bumpy expects `1.0.0`. That is a real argument, answered by `migrate` writing `semver`, which puts the expectation where it actually applies and leaves `init` free to be opinionated about projects with no release history
- Good, because it matches changesets and bumpy, so a migration from either produces identical numbers
- Bad, because it forces a project to choose between describing a change accurately and not claiming stability it does not have
- Bad, because it is wrong for a project being initialized, which has no release history and is therefore pre-1.0 by definition

### Zero-major as an opt-in key

- Good, because the default surprises nobody arriving from changesets or bumpy
- Bad, because it makes every pre-1.0 repository configure its way to the behavior the spec recommends
- Bad, because the migration case it protects is better handled by `migrate` preserving the source repository's behavior, which is narrower and does not tax new repositories

## More Information

- [version-format constraints](../research/version-format-constraints.md) — what each surveyed tool does, and the cascade comparison across range shapes. Prior art is context there, not justification here
- [ADR-0010](0010-derive-cascade-from-declared-ranges.md) — the cascade rule this was checked against
- [ADR-0004](0004-derive-facts-configure-preference.md) — why a version policy is preference rather than a derivable fact
- [idea 0008](../ideas/0008-custom-version-formats.md) — why the key is an enum: epoch semver would be another value, where a boolean has nowhere to go
