# Bump files

- Status: draft
- Version: 0.1
- Last updated: 2026-08-21
- Driving ADRs: ADR-0005, ADR-0019, ADR-0023, ADR-0028

## Overview

A bump file records an intended version change and the note that goes with it. One file per change, written when the change is made, consumed when the release is cut.

The format is the changesets format, unchanged. That is a compatibility decision rather than an aesthetic one: `@changesets/cli` and knope both read `.changeset/*.md`, and adopting the same directory and grammar means a repository can run oakum alongside its existing tool during a migration, with neither tool confused by the other's *release* files.

Oakum writes the subset all three parsers agree on for `patch` / `minor` / `major` ([ADR-0005](../decisions/0005-write-the-changeset-format-intersection.md)). It also writes empty frontmatter and `none` for releaseless coverage ([ADR-0028](../decisions/0028-releaseless-bump-files-like-bumpy.md)); those two shapes match bumpy and `@changesets/cli` and are unsafe under knope.

## Requirements

### Functional

- A bump file names zero or more packages and the level each should receive (`patch`, `minor`, `major`, or `none`)
- An empty frontmatter block marks an intentionally releaseless change
- The prose body becomes the changelog entry for packages that receive a real bump; `none` entries carry a note for humans and coverage, not a changelog release line
- Files are consumed and deleted when a release is cut
- A `patch` / `minor` / `major` file that oakum writes must be readable, unchanged, by `@changesets/cli` and by knope
- A `none` or empty file that oakum writes must be readable by oakum and by `@changesets/cli`; knope is out of scope for those shapes

### Non-functional

- A malformed file names itself in the error and does not prevent other files from being read
- `add` never blocks on a prompt when input is not a terminal

## Interface / Contract

```markdown
---
oakum: minor
---

Bump files are now validated against all three parsers, and a malformed file
names itself instead of aborting the run.
```

The grammar oakum writes and accepts:

- Line 1 is exactly `---`
- Each following line is `<package-name>: <level>` where level is `patch`, `minor`, `major`, or `none`, **or** the frontmatter contains no package lines (empty)
- Package names are unquoted, except a scoped npm name, which must be quoted
- No blank lines inside the frontmatter, and no repeated package name
- A closing `---`
- Everything after is the note, as Markdown, kept verbatim

**A scoped npm name is the one case the release-level intersection cannot cover.** `@` is a YAML reserved indicator, so `@scope/pkg: minor` is a parse error for `@changesets/cli`; quoting it satisfies YAML but makes knope retain the quotes and skip the file with no output. Oakum quotes scoped names, accepting that such a file is invisible to knope. The two parsers only share a directory in a repository migrating from knope, whose packages are crates, and crate names are never scoped — so the conflict is unreachable in practice. Reject the combination explicitly rather than emitting a file one reader will ignore.

Not permitted: quoted keys except the scoped-name case above, a preamble before the opening `---`, a UTF-8 byte order mark, any key that is not a package in the workspace, and any level other than `patch`, `minor`, `major`, or `none`.

Filenames are arbitrary apart from the `.md` extension, which is the identity used for deletion.

**`oakum add` flags.** Every value the prompt can produce is reachable non-interactively, so an agent or a CI run can write the same file without a terminal:

| Flag | Effect | Status |
|---|---|---|
| `--packages <list>` | comma-separated `name:level` pairs, as `"core:minor,utils:patch"`; levels may include `none` | settled |
| `--message <text>` | the note body | settled |
| `--name <slug>` | filename stem, slugified; defaults to a generated name | settled |
| `--interactive` | runs the guided prompt instead of the silent path; exits non-zero when stdin is not a terminal, naming the equivalent flags | settled |
| `--empty` | writes a bump file with empty frontmatter (intentionally releaseless) | settled ([ADR-0028](../decisions/0028-releaseless-bump-files-like-bumpy.md)) |
| `--none` | names packages at level `none` (no direct bump; cascade still allowed; covers `--strict`) | settled ([ADR-0028](../decisions/0028-releaseless-bump-files-like-bumpy.md)) |

`--packages` is required on the non-interactive path unless `--empty` supplies empty frontmatter. `--none` always requires `--packages` with `name:none` pairs. A flagless `oakum add` has no input and, under the rule below, no prompt either — it exits non-zero naming both `--packages` and `--interactive`, so the guided path stays discoverable without reading this document.

`--empty` is mutually exclusive with `--packages` and `--none`. Non-interactive `--none` still uses the same `--packages` grammar — comma-separated `name:none` pairs, as `oakum add --none --packages "core:none,utils:none" --message "…"`. A bare name list is invalid. Implying "all changed packages" without `--packages` waits on coverage detection and is not part of this contract yet.

**Prompting is opt-in, for the reason [init](init.md) gives.** Detecting a terminal would prompt an agent running through a PTY — the caller least able to answer — so `add` prompts only when asked, and the default path never blocks.

`--packages` and `--message` are what the template ships today; the rest come from bumpy's surface, recorded in [bump-file tool interfaces](../research/bump-file-tool-interfaces.md).

## Behavior

**Absence versus an explicit file.** No bump file at all is a valid releaseless answer when nothing needs covering: default `check` treats an empty set as “nothing to release.” A pull-request comment that suggests adding a changeset is presentation ([ADR-0015](../decisions/0015-layer-the-pr-status-channels.md)) — the gate must not depend on it, so ignoring the comment is fine. Empty frontmatter and `none` exist for when something must answer out loud: a strict coverage gate, or a human who wants the bot to stop asking without shipping a bump.

**Writing.** `oakum add` creates one file per invocation. Multiple bump files for the same package accumulate; the highest *release* level among them wins (`major` > `minor` > `patch`), `none` never raises a release level, and every note from a releasing contribution appears in the changelog.

**Reading.** Every `.md` file directly inside `.changeset/` is a bump file, except the four names `@changesets/read` skips as of `@changesets/cli` v3: `README.md` matched case-insensitively, and `AGENTS.md`, `CLAUDE.md`, and `GEMINI.md` matched exactly. Subdirectories are not read. Anything without a `.md` extension is not parsed as a bump file, which is what makes `_config.toml` safe to store alongside. Empty and `none` files are read: they are coverage and intent, not ignored config.

**Consuming.** `oakum version` applies the plan and deletes the files it consumed. A file whose packages all stayed at `none` (no direct bump and no cascade applied) is still consumed when the release run that considered it finishes — same `.md` delete rule as any other bump file. A file naming only packages that saw no release under an older rule is left in place only until this consumption rule is implemented; do not invent a second extension.

## Edge cases

- **A package name that is not in the workspace** is an error naming the file and the unknown name. `@changesets/cli` treats this as fatal, and knope ignores it silently; erroring is the safer of the two.
- **A malformed file** is reported by path and skipped, and the run continues. Both other parsers abort the entire run without naming the file, which turns a typo into a manual bisect.
- **An agent instruction file in `.changeset/`** — `README.md`, `AGENTS.md`, `CLAUDE.md`, or `GEMINI.md` — is skipped by oakum and by `@changesets/cli` v3, and aborts every knope run. The case handling is asymmetric: `readme.md` is skipped, `agents.md` is not, so a lowercase variant of the latter three is parsed as a bump file and is fatal to both readers. Migration warns about all four.
- **No bump files at all** is not an error. It reports that there is nothing to release and exits zero.
- **`none` or empty under knope** — if knope is still the release tool for the repository, do not introduce these files until cutover. `migrate` must not silently leave a `none` file for knope to treat as a patch.

## Testing strategy

ADR-0005's Confirmation requires pinning the *release-level* intersection with tests against **both** foreign parsers, not just oakum's own. ADR-0028 adds oakum (and JS) coverage for empty and `none`; those fixtures are not knope Confirmation inputs.

- Unit tests: frontmatter parsing and rejection of each item in the not-permitted list, including a scoped name with and without quotes; accept empty and `none`.
- Integration tests: every *release-level* file oakum writes is fed to `@changesets/parse`
  (format gate behind `@changesets/cli`; workspace membership out of scope) and
  to knope's `changesets` crate. Unscoped intersection bodies must be accepted
  by both with the intended package names. A silent skip (exit 0 with retained
  quotes or unmatched names) is the failure mode. Scoped keys oakum quotes have
  no intersection: JS accepts the real name; knope retains the quotes. The suite
  asserts that retention; it is not a Confirmation failure. Empty and `none`
  files are asserted against oakum and `@changesets/parse` only.

## Open questions

- ~~How to express "this change ships no release".~~ **Answered 2026-08-21 by [ADR-0028](../decisions/0028-releaseless-bump-files-like-bumpy.md)**: empty frontmatter and `name: none` in ordinary `.changeset/*.md`, matching bumpy.
- ~~Whether a repository can opt out of bump files entirely in favor of conventional commits.~~ **Answered 2026-08-18 by [ADR-0019](../decisions/0019-both-change-files-and-commits-each-disableable.md)**: it can, and it can opt out of commit parsing too — either mechanism alone is a complete configuration. What remains open is how the two *compose* when both are enabled, and whether a commit mapping to a package a bump file already covers is ignored, merged, or reported as a conflict.
- ~~What `oakum add`'s non-interactive interface is.~~ **Answered 2026-08-18, written into the Interface section 2026-08-19.** It adopts bumpy's surface; the template had shipped `--packages` and `--message` only. ADR-0028 settles the last two flags' wire format.

## Change log

- 2026-08-18: initial draft (v0.1)
- 2026-08-18: opting out of bump files closed by ADR-0019; the composition question stays open (v0.1)
- 2026-08-19: `add`'s flags written into the Interface section; the skip list corrected to four names with its case asymmetry; ADR-0023 added to the driving list (v0.1)
- 2026-08-21: ADR-0028 settles empty / `none` in normal `.md`; flags unblocked; knope Confirmation scoped to release levels (v0.1)
