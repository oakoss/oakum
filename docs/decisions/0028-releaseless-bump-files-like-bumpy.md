# Express releaseless changes in normal bump files

- Status: accepted
- Date: 2026-08-21
- Deciders: Jace Babin
- Amends: [ADR-0005](0005-write-the-changeset-format-intersection.md)

## Context and Problem Statement

`--empty` and `--none` on `oakum add` need a wire format. [ADR-0005](0005-write-the-changeset-format-intersection.md) rejected putting `none` or an empty frontmatter block in `.changeset/*.md` because knope misreads them (silent patch, or fatal). That forced an undecided non-`.md` marker (`okm-sqo`). Should oakum keep that constraint, or write the same shapes bumpy and `@changesets/cli` already use?

## Decision Drivers

- bumpy is the primary reference for oakum's `add` surface ([bump-file tool interfaces](../research/bump-file-tool-interfaces.md))
- A separate marker invents a second lifecycle for `version` to consume and delete
- Dual-tool safety during migration is a shadow-period concern, not a permanent dialect tax on every oakum feature
- Leaving oakum for another tool is not a design obligation

## Considered Options

- Non-`.md` oakum-owned marker (TOML / custom extension), keeping the ADR-0005 exclusion
- Normal `.changeset/*.md` with empty frontmatter (`--empty`) and `name: none` (`--none`), matching bumpy / changesets
- Keep "write nothing" forever and drop the two flags

## Decision Outcome

Chosen option: **normal `.changeset/*.md` like bumpy**, because the flags already exist to match that tool, the `.md` extension already owns consumption and deletion, and the knope failure modes are accepted for these shapes rather than forever forbidding them.

| Flag | File shape |
|---|---|
| `--empty` | frontmatter with no package lines (`---` / `---`), optional note body |
| `--none` | one or more `name: none` lines, plus a note |

`none` means: cover the named package for a strict coverage gate, take no direct bump, still accept a cascade from another package's release. That is bumpy's documented semantics, not a new invention.

**This amends ADR-0005.** The intersection rule still governs `patch` / `minor` / `major` files that must round-trip through knope during a shadow period. Oakum may additionally write and read `none` and empty frontmatter. Those files are valid for oakum and for `@changesets/cli`; they are **not** safe under knope and are out of scope for the knope leg of the foreign-parser Confirmation suite. A repository still releasing with knope should not introduce them until cutover.

ADR-0023's `add` row stays "one `.changeset/*.md` per invocation" — no ownership amendment.

**Absence remains valid.** A pull request with no bump file still means nothing to release under a non-strict check. Empty and `none` are for when coverage or review needs an explicit “I meant this” — including answering a pull-request comment that asks for a changeset without turning that comment into the gate ([ADR-0015](0015-layer-the-pr-status-channels.md)).

### Consequences

- Good, because `--empty` / `--none` unblock without a second file type or delete rule
- Good, because agents and humans already know the bumpy / changesets shapes
- Bad, because a knope shadow checkout that picks up such a file can mis-bump (`none` → patch) or abort (empty) — migrate and docs must say so
- Neutral, because "write nothing" remains valid for docs-only PRs when no coverage gate requires a file

### Confirmation

Foreign-parser Confirmation continues to pin `patch`/`minor`/`major` intersection bodies against both parsers. Oakum's own tests cover empty and `none`. Revisit only if a real dual-tool workflow needs releaseless coverage before knope is removed.

## More Information

**Amended 2026-09-05 (`okm-ctd`):** this ADR settles the wire format oakum writes. Migration of files another tool already wrote is separate: `oakum migrate` preserves `none` and empty frontmatter from changesets / bumpy, and refuses those shapes when `knope.toml` is present — never coerce to `patch`. See [specs/migrate.md](../specs/migrate.md).

- [changeset-file-format.md](../research/changeset-file-format.md) — still true that knope mishandles these shapes; the research implication that oakum must therefore use a non-`.md` marker is superseded here
- [specs/bump-files.md](../specs/bump-files.md) — contract for the flags and grammar
- [specs/migrate.md](../specs/migrate.md) — preserve `none` / empty on adopt; refuse under knope
- [ADR-0023](0023-name-every-verb-and-what-it-owns.md) — `add` still writes only `.md`
- Implementation: `okm-64b.4`. Coverage-gate behavior that distinguishes advisory comments from `--strict`: `okm-22h`. Migrate policy: `okm-ctd`.
