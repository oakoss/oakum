# generate

- Status: draft
- Version: 0.1
- Last updated: 2026-08-21
- Driving ADRs: ADR-0003, ADR-0019, ADR-0023, ADR-0029

## Overview

`oakum generate` derives one bump file from commits on the current branch and writes it under `.changeset/`. It is the commit→file bridge: humans (and later `status` / `version`) still reason about pending `.md` files, not about a second parallel plan input.

That role is fixed by [ADR-0029](../decisions/0029-plan-from-one-intent-artifact.md) and [ADR-0023](../decisions/0023-name-every-verb-and-what-it-owns.md). When change files are enabled, the plan reads only those files; commits enter the plan only by becoming a file first. When change files are disabled and conventional commits are enabled, the plan reads commit-derived intent directly (no orphan write) — that path shares the same mapper as `generate` but is not this verb.

`generate` therefore requires **both** `change-files` and `conventional-commits` in `.changeset/_config.toml`. With either off it refuses. That keeps a disabled commits switch from leaving a commit parser online, and keeps a commits-only repository from writing files the plan would ignore.

## Requirements

### Functional

- Scan `from..HEAD` (exclusive base) and map commits to package release levels
- Write exactly one bump file per successful run (or print that body under `--dry-run`)
- Aggregate highest-wins per package across the range; include a note body built from commit summaries
- Refuse when either intent mechanism is disabled
- Refuse when the range yields no package bumps
- Never feed the release plan except by writing a bump file the plan already knows how to read

### Non-functional

- Own only the derived `.changeset/*.md` it writes ([ADR-0003](../decisions/0003-write-only-what-a-command-owns.md) / [ADR-0023](../decisions/0023-name-every-verb-and-what-it-owns.md))
- Read-only against git and the workspace except for that write (or no write under `--dry-run`)
- Honor the ADR-0007 `tool-version` gate before doing work

## Interface / Contract

**Writes:** one `.changeset/<stem>.md` bump file (unless `--dry-run`).

**Flags:**

| Flag | Effect | Status |
|---|---|---|
| `--from <ref>` | Exclusive git base for the scan (`from..HEAD`). Default: try `origin/main`, then `main`, then `master`; for the first ref that exists, use `merge-base(ref, HEAD)` when that succeeds, otherwise the ref itself (do not fall through to later candidates). If none exist, refuse and name `--from` | settled |
| `--dry-run` | Print the bump-file body to stdout; write nothing | settled |
| `--name <slug>` | Filename stem, slugified; defaults to a generated name (same rule as `add`) | settled |

**Config gate:** both `change-files = true` and `conventional-commits = true` (missing `_config.toml` defaults both on). Otherwise exit non-zero naming the gate and ADR-0029.

**Never writes:** the plan, tags, manifests, lockfiles, CI, git config, or any file outside `.changeset/`.

## Behavior

### Mapping one commit

Parsing uses `git-conventional` (case-insensitive type compares). Level selection:

| Commit shape | Level |
|---|---|
| Breaking (`!` or `BREAKING CHANGE` footer) | `major` |
| `feat` | `minor` |
| Every other conventional type (`fix`, `docs`, …) | `patch` |
| Not conventional | treated as path fallback at `patch`, using the first line as summary |

**Scoped conventional commit.** Resolve the scope against the workspace package names:

- Exactly one match → attribute that package at the commit's level
- Missing / unknown scope → path fallback at the commit's level
- Ambiguous scope (same name in more than one ecosystem) → hard error

**Path fallback.** When there is no usable scope (or the message is not conventional), attribute the commit's level to every package whose repository-relative directory is a longest prefix of a changed path. Co-located packages that share that prefix (Cargo + npm in one directory) all receive the bump. Merge commits contribute no paths (no parent-union attribution).

### Aggregating the range

Commits are processed oldest-first. Per package, the highest release level wins (`major` > `minor` > `patch`). The note body is one Markdown list line per contribution (`- <package>: <summary>`), in encounter order.

Empty `from..HEAD` or a non-empty range that maps to no packages is an error for `generate` (nothing to write). The commits-only **plan** path treats the same empty aggregate as “nothing to release”; that divergence is intentional.

### Writing

The file uses the same writer as `oakum add`: intersection grammar for release levels, knope-aware quoting rules. Pending files already in `.changeset/` are not consulted — overlap becomes ordinary multi-file highest-wins at plan time, matching bumpy's generate ([intent-mechanism composition](../research/intent-mechanism-composition.md)).

## Edge cases

- **Either mechanism off** — refuse; do not write. Commits-only plan input is a different path ([bump-files](bump-files.md) / ADR-0029).
- **No default base ref** — refuse and require `--from`.
- **Default base exists but has no merge-base with HEAD** — use that ref as the exclusive base; do not try the next candidate (`main` / `master`).
- **Ambiguous conventional scope** — refuse naming the scope; do not guess an ecosystem.
- **Merge commits** — skipped for path attribution so merging `main` into a feature branch does not credit base-only paths.
- **Root package vs nested paths** — longest-prefix wins; the repository-root package does not steal paths under a nested package directory.
- **`--dry-run` with a successful map** — prints the file body; exit zero; `.changeset/` unchanged.
- **Pending bump files already covering a package** — not skipped today; product preference left open (peers append).

## Testing strategy

- Unit tests: conventional level mapping, ambiguous scope errors, path-fallback longest-prefix and co-located packages, aggregation highest-wins (`commits` module).
- Integration tests (`generate_cli`): both-mechanism gate refusals; scoped write; dry-run writes nothing; path fallback for plain / unscoped messages; empty intent errors; multi-commit highest-wins.

## Open questions

- ~~Whether `generate` may run when change files are off.~~ **Answered by ADR-0029:** no — both mechanisms required; commits-only plan does not write.
- Should `generate` warn or skip packages already listed in pending bump files? Peers do not; preference only ([research](../research/intent-mechanism-composition.md)).

## Change log

- 2026-08-21: initial draft from shipped `oakum generate` / `okm-j1r` and ADR-0029 (v0.1)
