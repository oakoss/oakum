# Plan from one intent artifact at a time

- Status: accepted
- Date: 2026-08-21
- Deciders: Jace Babin
- Amends: [ADR-0019](0019-both-change-files-and-commits-each-disableable.md), [ADR-0023](0023-name-every-verb-and-what-it-owns.md)

## Context and Problem Statement

[ADR-0019](0019-both-change-files-and-commits-each-disableable.md) accepts both change files and conventional commits, each fully disableable. It left open how they compose when both are enabled: whether a commit that maps to a package already covered by a change file is ignored, merged, or reported as a conflict. How does oakum feed the plan?

## Decision Drivers

- Pending bump files batch across many changes into one release; the plan already aggregates many `.changeset/*.md` files ([specs/bump-files.md](../specs/bump-files.md))
- [ADR-0019](0019-both-change-files-and-commits-each-disableable.md) and [ADR-0023](0023-name-every-verb-and-what-it-owns.md) already give `generate` the role of writing change files from commits, not feeding the plan directly
- bumpy is the primary interface peer and keeps the plan files-only, with commits entering only through `generate` ([intent-mechanism composition](../research/intent-mechanism-composition.md))
- The coverage gate (`okm-22h`) needs a single, obvious definition of "covered"

## Considered Options

- **Single artifact (Policy A)** — when change files are enabled, the plan reads only `.changeset/*.md`; commits feed `generate` only. When change files are disabled and commits are enabled, the plan reads commits
- **Parallel merge (Policy B)** — both feed the plan; bump level is highest-wins (knope)
- **Conflict (Policy C)** — disagreeing levels for the same package fail `check` / `status`

## Decision Outcome

Chosen option: **single artifact (Policy A)**, because the repository's release batching is already "many pending files, one cut," ADR-0019/`generate` already converge intent onto change files, and that is what bumpy does — so plan-time overlap between commits and files does not arise when files are on.

| Config | What the plan reads |
|---|---|
| Change files enabled (commits on or off) | `.changeset/*.md` only |
| Change files disabled, commits enabled | Commit-derived intent mapped directly into the plan (same bump/package rules `generate` would use, without writing a file) |
| Both disabled | Invalid — `check` must say so (ADR-0019) |

`generate` requires **both** change files and commit-derived intent enabled: it is the commit→bump-file bridge, writing `.changeset/*.md` a human can edit, and those files are what the plan reads. It does not feed the plan by any other path. If either mechanism is disabled, `generate` is unavailable (or refuses) — otherwise disabling commits would leave a commit parser online, and running it with change files off would write files the commits-only plan ignores. (`add` may still write bump files when change files are the enabled mechanism; when change files are disabled those files are not plan input until that mechanism is turned back on.) Multiple pending files still accumulate until `version` consumes them; one file is not one release.

**This amends ADR-0019** (composition) **and ADR-0023** (`generate` availability). Symmetry of disable switches is unchanged.

Coverage (`okm-22h`) follows the same table: when change files are enabled, coverage is against pending bump files (including empty / `none` where used); commits never satisfy the gate. When only commits are enabled, coverage is defined against commit-derived intent for changed packages.

### Consequences

- Good, because authors reason about one pending pool of bump files, including files produced by `generate`
- Good, because a forgotten `feat:` cannot silently raise a carefully authored `patch` file (knope's surprise)
- Good, because the coverage gate has one primary definition when files are on
- Bad, because repositories that want knope-style auto-merge without running `generate` must change workflow or disable change files
- Neutral, because whether `generate` skips packages already listed in pending files is product preference, not required by this decision

### Confirmation

Revisit if a commits-only repository cannot get a correct plan without a throwaway change-file write, or if teams that keep both mechanisms enabled routinely need knope-style dual plan input and refuse the `generate` step.

## More Information

- [intent-mechanism composition](../research/intent-mechanism-composition.md) — peer survey (bumpy serialize vs knope merge; no conflict peer)
- [ADR-0019](0019-both-change-files-and-commits-each-disableable.md) — both mechanisms, each disableable
- [ADR-0023](0023-name-every-verb-and-what-it-owns.md) — `generate` owns derived `.changeset/*.md` when both mechanisms are enabled
- [specs/bump-files.md](../specs/bump-files.md) — pending files accumulate; `version` consumes them
- Follow-up: `okm-j1r` (`generate`), `okm-22h` (coverage gate against the table above)
