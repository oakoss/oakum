# Derive what a change reaches, and what covers it

- Status: accepted
- Date: 2026-10-31
- Deciders: Jace Babin
- Amends: [ADR-0028](0028-releaseless-bump-files-like-bumpy.md)

## Context and Problem Statement

A tm pull request (oakoss/tm#101) refactored two internal crates with no behavior change. It needed a hand-written CI step and an `--empty` bump file, for two reasons:

- `check` never asks about an internal crate a released package ships. Coverage drops a change to an unmanaged package into a separate report (`changed_by_standing` in `crates/oakum/src/cli/coverage.rs`), so `tm-store` changing says nothing about `tm-cli`, which ships it.
- With change files and conventional commits both on, a branch whose commits all say `refactor:` or `test:` still needs a file to say it releases nothing. The commits already say so.

The first gap is not only a coverage gap. A `fix(store): …` commit whose paths touch only `tm-store` reaches no package: `generate` refuses with nothing to write, and in a commits-only repository the fix is never released. A gate that asks `tm-cli` for coverage would ask for a file `generate` cannot write.

What does a change reach, and what covers a package it reaches?

## Decision Drivers

- Which packages ship an internal crate is a fact in the dependency graph, not a preference ([ADR-0004](0004-derive-facts-configure-preference.md)). No config keys.
- One answer to "what does this change reach" for every reader: coverage, `generate`, the commits-only plan. Two answers mean the gate asks for something the writer cannot produce.
- A look that could not run is `unverified` ([ADR-0034](0034-exit-two-for-unverified.md)), never covered.
- A refusal names the commit that caused it and the command that fixes it.

## Considered Options

- **A config list of the internal crates each package ships**: restates the graph, and will rot.
- **Reach for coverage only**: closes the gate gap and leaves `generate` and the commits-only plan unable to attribute an internal fix.
- **One derived reach rule for every reader of changes**, with releaseless commits as coverage.

For releaseless commits, two scopes were weighed: the whole branch (no bump file of its own and every commit releases nothing) or each package (every commit reaching it releases nothing). Branch scope leaves `b` uncovered on a branch with `fix(a)` and `refactor(b)` even after `a` gets its file, and differs from how the commits-only plan already reads a commit that releases nothing.

## Decision Outcome

Chosen option: **one derived reach rule for every reader of changes, and per-package releaseless coverage**, because it is the only option where the gate never asks for intent that `generate` cannot write, and it adds no configuration.

### What a change reaches

A changed path reaches the package whose directory is its longest prefix, as today. When that package is not managed (unmanaged, or excluded by `include`/`exclude`), the change also reaches every managed package that depends on it along a runtime edge: the edges a release cascades along under [ADR-0008](0008-cascade-only-along-runtime-edges.md) (npm `dependencies`, `peerDependencies`, `optionalDependencies`; Cargo `dependencies` and `build-dependencies`). The walk continues through other non-managed packages and stops at the first managed package on each path; past it, the cascade rules of ADR-0008 to [ADR-0010](0010-derive-cascade-from-declared-ranges.md) decide.

Excluded packages are followed like unmanaged ones. ADR-0027 makes exclusion a statement about a package's own version, changelog and tag, not about what ships it.

A reach through other packages records its path, written `tm-cli (via tm-store)`; a longer chain names each package in order. When several paths reach the same package, the shortest is named, ties broken by name.

The rule serves every reader of changes:

- coverage in `check`, `status` and `ci pr-status`;
- `generate`'s path fallback, and a scope that names a non-managed package;
- the commits-only plan;
- the report of releasing commits that reach no package.

A `fix(store): …` commit therefore releases `tm-cli` at `patch`, and `generate` writes that line.

A changed non-managed package that a managed package ships is reported as that package's change. Only one that no managed package ships stays in the unmanaged report.

### What covers a package a change reaches

A managed package a change reaches is covered by any of:

1. a bump file on the branch that names it, at any level including `none`;
2. an empty bump file on the branch, which covers every changed package (ADR-0028);
3. with `conventional-commits` on, the branch's commits, when every commit that reaches the package releases nothing.

Otherwise it is uncovered.

A commit reaches the packages `generate` attributes it to: its scope when that names a package, otherwise its paths, both through the reach rule above. A commit releases nothing when it is conventional, not breaking, and neither `feat` nor `fix` ([generate spec](../specs/generate.md)). A message that is not conventional releases, at `patch`, as `generate` reads it.

| Intent configured | What covers a package |
|---|---|
| change files only | a bump file (unchanged) |
| change files and conventional commits | a bump file, or commits that all release nothing |
| conventional commits only | commits, read as the plan reads them (unchanged) |

A `fix` commit covered by a `none` file stays covered: the file is the author saying it releases nothing, which is what ADR-0028 made it for.

**This amends ADR-0028.** Under both intent sources, a branch whose commits release nothing needs no file to cover the packages they reach.

**Merge commits are skipped.** They carry no paths of their own, as in `generate`, so merging the base into a branch neither covers nor uncovers anything.

**A revert of a commit on the branch cancels both.** Git's `Revert "…"` commit names its target in its body (`This reverts commit <sha>.`). When the target is in the range being read, neither commit counts, for coverage or for the plan. A revert of a commit outside the range is read like any other non-conventional message, and releases, because undoing a released change changes what ships.

### When the look cannot run

A shallow clone is `unverified` (exit 2), naming `fetch-depth: 0`, as today: the same range supplies paths and messages. A dependency graph or commit messages that cannot be read leave coverage `unverified` too. Neither is ever read as covered.

### What each surface says

`check`'s text, the pull-request comment and `--json` ([ADR-0036](0036-report-the-whole-check-run-as-data.md)) report the same things for each changed managed package:

- the path it was reached by, empty for a direct change;
- how it is covered: the bump files that name it, an empty bump file, or the releaseless commits with each one's short hash and type;
- when uncovered, each commit that releases, with its short hash, subject, and why it releases (`feat`, `fix`, breaking, or not conventional).

Releaseless coverage is shown, not only counted. The comment reads "releases nothing: refactor, test" so a reviewer sees the claim the commits make. Coverage takes the commit type on trust, as conventional commits always have.

A refusal names the fix:

- `oakum generate` for a releasing commit when `generate` is allowed, otherwise `oakum add`;
- `oakum add --none <package>` when the change should release nothing;
- for a message that is not conventional, that oakum reads it as a `patch`.

It never suggests rewording or rebasing commits.

### Consequences

- Good, because tm's refactor is covered by its own commits, with no CI step and no file.
- Good, because a fix in an internal crate releases the package that ships it, in both intent modes, where it was silently dropped before.
- Good, because the gate and `generate` read changes the same way, so the gate never asks for a file that cannot be generated.
- Bad, because a pull request that changes only an internal crate passed `check --strict` and is now refused until something covers it. That is a refusal on an input that worked, breaking under [ADR-0022](0022-zero-major-versioning.md). A repository without conventional commits covers it with a `none` file.
- Bad, because coverage trusts the commit type: a behavior change typed `refactor:` passes the gate. The comment showing the claim is the defense.
- Bad, because an npm package that bundles an internal package listed under `devDependencies` is not reached. The release cascade has the same gap under ADR-0008, so the two rules stay one rule.
- Neutral, because oakum's own workspace is unaffected: `crates/oakum` does not depend on `crates/plan-no-std`.

### Confirmation

tm removes its inline CI step once this ships. Revisit if repositories routinely cover behavior changes with releaseless types, or if bundled `devDependencies` reach a decision for the cascade.

## More Information

- Driving bead: `okm-e9p8.1`, epic `okm-e9p8`. Implemented by `okm-e9p8.3` (releaseless commits), `okm-e9p8.4` (the reach rule, including `generate` and the commits-only plan) and `okm-e9p8.6` (the report). The exact `--json` field names are settled with `okm-e9p8.6`.
- The [generate spec](../specs/generate.md) is updated with the implementation.
- Deferred: a pull-request-title check for squash-merged commits-only repositories; `check --explain` (`okm-bluv`).
- Open: a conventional `revert:` commit releases nothing under `generate`'s table, unlike git's default revert subject.
