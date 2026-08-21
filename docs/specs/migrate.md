# migrate

- Status: draft
- Version: 0.1
- Last updated: 2026-08-19
- Driving ADRs: ADR-0003, ADR-0005, ADR-0007, ADR-0011, ADR-0022, ADR-0023

## Overview

`oakum migrate` adopts a repository that already uses another release tool. It is separate from [`init`](init.md) because adoption carries risks initialization does not: existing bump files may be in a dialect oakum does not write, and another tool is still reading the same directory.

ADR-0003 restricts a command to the files it owns. A command named `migrate` owns the migration — the objection it encodes is to work performed as an unrequested side effect of something else, not to a command doing the job it is named for. The line drawn here is **transform data, report tooling**.

## Requirements

### Functional

- Convert existing intent files and configuration into the form oakum writes
- Leave the repository initialized, as if `init` had run — with one deliberate exception, `versioning`, which is taken from the source tool rather than from oakum's default
- Prove the migration did not change what would be released, with one documented exception: a pre-1.0 knope repository with a pending feature, where [ADR-0022](../decisions/0022-zero-major-versioning.md) deliberately plans a minor where knope planned a patch
- Name every remaining step it does not perform

### Non-functional

- Idempotent — running it twice changes nothing the second time
- Runnable non-interactively
- Shows what it will do before doing it

## Interface / Contract

**Transforms:**

| Change | Why |
|---|---|
| Adopts `.changeset/` in place | The directory name is already correct. bumpy renames `.changeset/` → `.bumpy/` with a plain `fs.rename`; that is pure risk here. |
| Rewrites quoted package keys to unquoted, except scoped npm names | `@changesets/cli` writes every key quoted, and knope silently skips those files with exit 0 and no output. A scoped name keeps its quotes: `@` is a YAML reserved indicator, so unquoting it makes the file unparseable by the tool being migrated away from. |
| Converts `.changeset/config.json` → `_config.toml` | Carrying over only keys that still mean something, and naming every key it drops. A silently discarded key is the failure `docs/research/tool-version-pinning.md` records: a stale `prettier` key survived a changesets upgrade with no error and no warning, and formatting changed underneath the user. |
| Leaves `none`-level entries unchanged | Oakum represents `none` in ordinary bump files ([ADR-0028](../decisions/0028-releaseless-bump-files-like-bumpy.md)); coercing to `patch` would invent a release. |
| Writes `_schema.json` and `README.md` | Same as `init`; [ADR-0023](../decisions/0023-name-every-verb-and-what-it-owns.md) assigns `migrate` those files plus the `.changeset/*.md` it transforms. |
| Sets `versioning` from the source tool | From changesets or bumpy, `semver` — those take `0.1.3` to `1.0.0`, and silently renumbering an established release line is not a migration's job. From knope, `zero-major`, which matches knope on breaking changes below 1.0.0 but **not** on features: knope also maps a feature to a patch, and [ADR-0022](../decisions/0022-zero-major-versioning.md) declines that. The plan-equality check below excludes that case rather than failing on it. `--versioning` overrides. This is the one place `migrate` deliberately differs from `init`, which applies oakum's default instead. |

**Flags:**

| Flag | Effect |
|---|---|
| `--versioning <semver\|zero-major>` | Overrides what would be inferred from the source tool ([ADR-0022](../decisions/0022-zero-major-versioning.md)) |

**Reports, does not perform:**

- Removing the old tool's dependency. bumpy shells out to `pnpm remove @changesets/cli`, touching `package.json`, the lockfile, and `node_modules` — three mutations that can fail in ways oakum cannot repair, against one command the user can run and verify.
- Editing or deleting the old tool's workflow. Not oakum's file, by the same decision that makes `check` read-only.
- Deleting the old tool's config.
- Adding oakum to a workflow. It prints the same YAML [`init`](init.md) does, with the exact `tool-version` [ADR-0007](../decisions/0007-pin-the-tool-version-in-config.md) requires already substituted. ADR-0003 forbids writing the file; printing nothing at all would be worse, because the end state would be a repository holding oakum config with no oakum invocation anywhere — which `check` reports as not found, and that is a failure.

## Behavior

### Breaking the old tool is the transition, not collateral damage

Writing `.changeset/README.md` breaks knope: it treats every `.md` there as a bump file and aborts on the first parse failure. `migrate` writes it anyway, and says so — knope will fail until `knope.toml` and its workflow are removed.

That is deliberate, at a moment the user chose, with the fix named. It is the opposite of `init` silently breaking a tool still in use, which is why the README conditional moved here rather than being handled with a permanently degraded filename.

### Verify the plan did not change

Compute the release plan before transforming and after, and assert they are identical apart from the knope feature case above. If adopting oakum would produce a different next version than the current tool, that must surface during migration rather than at the next release — and the one difference oakum creates on purpose is named, so it can be recognized rather than investigated.

No surveyed tool does this. It is cheap, falsifiable, and it is the same postcondition discipline the rest of the design rests on.

A difference is reported, not silently accepted, and never auto-resolved — the two tools disagreeing about a version is exactly the kind of thing a human should look at.

### Order

1. Detect the source tool and refuse if none is found
2. Compute the current release plan
3. Show every change to be made, and stop unless confirmed or run non-interactively
4. Transform
5. Recompute the plan and compare
6. Report remaining manual steps, including that the old tool will now fail

Nothing is written if any step before 4 fails — [ADR-0011](../decisions/0011-stop-at-the-tag.md)'s replacement for rollback, applied to a transformation: preflight the whole set so most failures abort with nothing to recover.

## Edge cases

- **Nothing to migrate** — reports it and names `oakum init`.
- **Already migrated** — the [ADR-0007](../decisions/0007-pin-the-tool-version-in-config.md) version gate runs first, since it exempts only `upgrade`. If the existing `_config.toml` pins a `tool-version` this binary does not match, `migrate` refuses in either direction and names `oakum upgrade`. Matching, it reports that it has already run and exits zero.
- **`none`-level bump file** — kept as `none` when the source already uses that level (changesets / bumpy). Oakum understands it ([ADR-0028](../decisions/0028-releaseless-bump-files-like-bumpy.md)). Do not rewrite it to `patch`. A knope source should not produce `none` files; if one appears, report it and refuse rather than let knope's custom-type reading become oakum's release plan.
- **An agent instruction file already in `.changeset/`** — warned about, never blocking. `AGENTS.md`, `CLAUDE.md`, and `GEMINI.md` match exactly; `README.md` matches in any case. All four abort a knope run, and `migrate` is the only command that runs in a knope repository, so it is the only place the warning can fire. A lowercase `agents.md` is skipped by neither reader and is the worst of them. `README.md` warns but never blocks: it is the expected state migrating from changesets, `migrate` leaves an existing one in place, and blocking would break idempotency on a second run.
- **Bump files naming packages not in the workspace** — reported by path, not dropped. The old tool may have been ignoring them silently.
- **Subdirectories in `.changeset/`** — reported. Fatal under `@changesets/cli` v2 and invisible to knope, so they were already doing nothing useful.
- **Plans differ before and after** — reported in full, exits non-zero, transformation is kept. Reverting would leave the repository in a third state nobody asked for.
- **A knope repository with a pending feature below 1.0.0** — the plans differ by construction, because knope maps a feature to a patch there and oakum maps it to a minor ([ADR-0022](../decisions/0022-zero-major-versioning.md)). Report it as an expected divergence naming the packages and both versions, and exit zero. Every other difference is still a failure and still exits non-zero.
- **A scoped npm package alongside `knope.toml`** — refuse, per ADR-0005. Quoting the scoped name satisfies `@changesets/cli` and makes knope skip the file silently; unquoting it inverts which reader breaks. `migrate` is the only command that runs in a knope repository, so this is the one place the rule can fire.

## Open questions

- ~~What a `none`-level entry becomes.~~ **Answered 2026-08-21 by [ADR-0028](../decisions/0028-releaseless-bump-files-like-bumpy.md)**: preserve `none`; never coerce to `patch`.
- Whether plan comparison is possible when the source tool cannot be run — knope requires its own config, which may be mid-removal.
- Whether `migrate` should support the reverse direction. Being able to leave is a reasonable thing to promise, and no surveyed tool offers it.

## Change log

- 2026-08-18: initial draft (v0.1)
- 2026-08-19: `versioning` preserved from the source tool per ADR-0022 (v0.1)
- 2026-08-19: ADR-0007, ADR-0011, and ADR-0023 added to the driving list — the workflow pin, the never-roll-back preflight, and this command's file ownership were each relied on without being declared. ADR-0020 was cited first and is the wrong driver: it settles that `check` and `release` share one precondition *implementation*, which `migrate` does not touch (v0.1)
- 2026-08-21: `none`-level handling closed by ADR-0028 (v0.1)
