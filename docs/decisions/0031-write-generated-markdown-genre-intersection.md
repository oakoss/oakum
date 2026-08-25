# Write generated markdown at the genre intersection

- Status: accepted
- Date: 2026-08-25
- Deciders: Jace Babin

## Context and Problem Statement

`version` writes changelogs. `add` and `generate` write bump files. A repository that lints `**/*.md` will lint both. When the bytes oakum wrote fail that linter, the failure lands on a CI version PR or a feature PR, not on a file a human authored. The live case is recorded in [changelog-lint-collision.md](../research/changelog-lint-collision.md).

[ADR-0023](0023-name-every-verb-and-what-it-owns.md) gives those commands the files. [ADR-0003](0003-write-only-what-a-command-owns.md) permits the write and forbids writing a linter config oakum does not own. [ADR-0006](0006-no-command-execution-in-templates.md) rejects executing a user-named binary from a template; a formatter pass on `version` is not a template, so this record has to say whether that pass is implicit write-path behavior. How does oakum's generated markdown relate to a project's linter?

## Decision Drivers

- A version PR opened by CI has no pre-commit hook
- Two markdown formatters already disagree on one file in this repository
- Keep a Changelog and the changeset bump-file grammar each fail one default markdownlint rule, by design
- `check` is pure

## Considered Options

- Write the genre intersection
- Read the repository's lint config and conform
- Run the repository's formatter over the file oakum wrote
- Emit and let a pre-commit hook fix it
- Exclude generated files from linting (rejected as oakum's answer)

## Decision Outcome

Chosen option: **write the genre intersection**, because that is the changeset-format intersection ([ADR-0005](0005-write-the-changeset-format-intersection.md)) applied to markdown oakum owns, and the research in [generated-markdown-and-linters.md](../research/generated-markdown-and-linters.md) shows the other four options fail a driver they cannot meet.

Oakum emits Keep-a-Changelog-shaped markdown that satisfies every default markdownlint rule those files can satisfy. Date goes in the version heading. Headings and lists have the surrounding blanks. No HTML, no italic-only date line, no extra consecutive blanks, wrap at 80 columns, file ends with one newline. Changelog shape (heading dialect) stays a configured preference ([ADR-0004](0004-derive-facts-configure-preference.md)); the default of that preference is this spacing and date placement, not a read of `.markdownlint*`.

Two rules have no intersection with the genre. Oakum does not try to satisfy them and does not grow a config key that copies the repository's disable:

- **MD024** on changelogs. Keep a Changelog repeats `### Added`. markdownlint's own rule doc names that case.
- **MD041** on bump files. Line 1 is `---`. There is no `title:` key the [ADR-0005](0005-write-the-changeset-format-intersection.md) intersection can add.

If a repository uses a markdown linter, it disables those two rules for those cases (MD024 on changelogs, MD041 on `.changeset/*.md`). MD024's smaller documented setting is `siblings_only: true`. `init` does not write `.markdownlint*` or `.rumdl.toml`; it prints the two lines, the same way it prints the workflow it does not write. `.changeset/README.md` and the guide carry the same text.

`add --message` stays verbatim ([specs/bump-files.md](../specs/bump-files.md)). Mechanical envelope only: trailing newline and a blank after the closing `---`. `generate` wraps the list it authors from commit subjects. `version` does not rewrite existing changelog sections, and `check` does not run a linter.

**Standing with the three ADRs the bead named:**

[ADR-0023](0023-name-every-verb-and-what-it-owns.md) gives `version` the changelog, `add` / `generate` the bump files, and `init` `.changeset/README.md`. This decision is about the bytes those writes contain, not about growing a new owner.

[ADR-0003](0003-write-only-what-a-command-owns.md) is satisfied by writing those owned files and by printing lint-config guidance instead of writing it. Running a formatter on a file oakum just wrote would also be an owned write. That is why option 3 is not an ownership failure.

This record rules option 3 out as implicit write-path behavior. [ADR-0006](0006-no-command-execution-in-templates.md) does not cover a non-template formatter pass; it supplies the two costs this record reuses (a stock runner may lack the binary; a preview that ran the fixer would apply it) and the Confirmation for any later exec surface: named, defaults off, enabled from outside the config file, never during `check`. If a formatter pass is added, it uses that surface on `version` only.

[ADR-0004](0004-derive-facts-configure-preference.md) permits reading a lint file (derive) and forbids a key that restates it. Implementing conformance is a linter product. "The" config is underspecified when two formatters disagree, which this repository already measured.

Option 4 fails where it matters: a CI version PR has no hook. Option 5 (exclude generated files) is a repository choice. It is not oakum's answer. A repository may still exclude `CHANGELOG.md`; the tool still emits bytes that survive a linter the repository kept.

### Consequences

- Good, because a first-release Keep-a-Changelog stub already passes a stock markdownlint run, and the second-release MD024 hit is the documented changelog exception
- Good, because `check` stays pure and a version job does not depend on prettier, rumdl, or dprint being installed
- Bad, because a repository that lints `**/*.md` and keeps default MD024 / MD041 will fail oakum-authored files until it applies the printed config
- Neutral, because additive custom rules (MD043, plugins) still fail after a perfect genre emit; that failure is accepted
- Neutral, because an inherited changelog that already has `<sub>` or missing blanks still fails on the old lines; oakum does not sanitize history

### Confirmation

[okm-luq] pins the satisfiable rules with fixtures the way the changeset-format intersection tests do: heading, blank, section, blank, list; date in the heading; no `<sub>`; no extra blanks. A stock markdownlint run on a new changelog oakum wrote fails only MD024, and only once a second `### Added` exists. A typical bump file oakum wrote fails only MD041. `check` does not shell out to a linter.

Revisit if a surveyed peer ships lint-config conformance that stays hermetic and does not copy the file into `_config.toml`.

## More Information

- [generated-markdown-and-linters.md](../research/generated-markdown-and-linters.md) (2026-08-25) — peer survey and the measurements this record cites
- [changelog-lint-collision.md](../research/changelog-lint-collision.md) (2026-08-19) — the claude-plugins MD022 failure
- Heading dialect (`## 1.2.3 (date)` vs `## [1.2.3] - date`) is changelog shape, already configured. This record does not pick one. cargo-dist's reader accepts both when the version token is in the heading.
- Whether `version` refuses to splice when it cannot find a recognized heading is a later product call. Append-without-spacing is the defect linesmith hit; it is not a reason to run a formatter.
