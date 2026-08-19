# Bump files

- Status: draft
- Version: 0.1
- Last updated: 2026-08-19
- Driving ADRs: ADR-0005, ADR-0019, ADR-0023

## Overview

A bump file records an intended version change and the note that goes with it. One file per change, written when the change is made, consumed when the release is cut.

The format is the changesets format, unchanged. That is a compatibility decision rather than an aesthetic one: `@changesets/cli` and knope both read `.changeset/*.md`, and adopting the same directory and grammar means a repository can run oakum alongside its existing tool during a migration, with neither tool confused by the other's files.

Because three parsers read these files, oakum writes only the subset all three agree on. See [ADR-0005](../decisions/0005-write-the-changeset-format-intersection.md) for what that excludes and why.

## Requirements

### Functional

- A bump file names one or more packages and the bump level each should receive
- The prose body becomes the changelog entry for those packages
- Files are consumed and deleted when a release is cut
- A file that oakum writes must be readable, unchanged, by `@changesets/cli` and by knope

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
- Each following line is `<package-name>: <level>` where level is `patch`, `minor`, or `major`
- Package names are unquoted, except a scoped npm name, which must be quoted
- No blank lines inside the frontmatter, and no repeated package name
- A closing `---`
- Everything after is the note, as Markdown, kept verbatim

**A scoped npm name is the one case the intersection cannot cover.** `@` is a YAML reserved indicator, so `@scope/pkg: minor` is a parse error for `@changesets/cli`; quoting it satisfies YAML but makes knope retain the quotes and skip the file with no output. Oakum quotes scoped names, accepting that such a file is invisible to knope. The two parsers only share a directory in a repository migrating from knope, whose packages are crates, and crate names are never scoped — so the conflict is unreachable in practice. Reject the combination explicitly rather than emitting a file one reader will ignore.

Not permitted, because at least one other parser rejects or silently mishandles it: quoted keys except the scoped-name case above, `none` as a level, an empty frontmatter block, a preamble before the opening `---`, a UTF-8 byte order mark, and any key that is not a package in the workspace.

Filenames are arbitrary apart from the `.md` extension, which is the identity used for deletion.

**`oakum add` flags.** Every value the prompt can produce is reachable non-interactively, so an agent or a CI run can write the same file without a terminal:

| Flag | Effect | Status |
|---|---|---|
| `--packages <list>` | comma-separated `name:level` pairs, as `"core:minor,utils:patch"` | settled |
| `--message <text>` | the note body | settled |
| `--name <slug>` | filename stem, slugified; defaults to a generated name | settled |
| `--interactive` | runs the guided prompt instead of the silent path; exits non-zero when stdin is not a terminal, naming the equivalent flags | settled |
| `--empty` | marks the change as intentionally releaseless | **blocked** |
| `--none` | names packages that take no direct bump but still accept a cascade | **blocked** |

`--packages` is required on the non-interactive path. A flagless `oakum add` has no input and, under the rule below, no prompt either — it exits non-zero naming both `--packages` and `--interactive`, so the guided path stays discoverable without reading this document.

**Prompting is opt-in, for the reason [init](init.md) gives.** Detecting a terminal would prompt an agent running through a PTY — the caller least able to answer — so `add` prompts only when asked, and the default path never blocks.

**The last two flags cannot be implemented yet, and they are blocked on different things.** Neither `none` nor an empty frontmatter block survives all three parsers ([ADR-0005](../decisions/0005-write-the-changeset-format-intersection.md)), so both need the non-`.md` marker the open questions below still owe. `--empty` needs a marker that merely exists; `--none` needs one carrying a package list and a note, which is a larger shape. Both also write a file that is not a `.md`, so settling the marker means amending [ADR-0023](../decisions/0023-name-every-verb-and-what-it-owns.md)'s `add` row, which today grants only "one `.changeset/*.md` per invocation". Until then `templates/changeset-readme.md` is right to tell users to write nothing at all.

`--packages` and `--message` are what the template ships today; the rest come from bumpy's surface, recorded in [bump-file tool interfaces](../research/bump-file-tool-interfaces.md).

## Behavior

**Writing.** `oakum add` creates one file per invocation. Multiple bump files for the same package accumulate; the highest level among them wins, and every note appears in the changelog.

**Reading.** Every `.md` file directly inside `.changeset/` is a bump file, except the four names `@changesets/read` skips as of `@changesets/cli` v3: `README.md` matched case-insensitively, and `AGENTS.md`, `CLAUDE.md`, and `GEMINI.md` matched exactly. Subdirectories are not read. Anything without a `.md` extension is not parsed as a bump file, which is what makes `_config.toml` safe to store alongside. A no-op marker is a different case: it is read, so "ignored" would be wrong for it.

**Consuming.** `oakum version` applies the plan and deletes the files it consumed. A file naming only packages that saw no release is left in place.

## Edge cases

- **A package name that is not in the workspace** is an error naming the file and the unknown name. `@changesets/cli` treats this as fatal, and knope ignores it silently; erroring is the safer of the two.
- **A malformed file** is reported by path and skipped, and the run continues. Both other parsers abort the entire run without naming the file, which turns a typo into a manual bisect.
- **An agent instruction file in `.changeset/`** — `README.md`, `AGENTS.md`, `CLAUDE.md`, or `GEMINI.md` — is skipped by oakum and by `@changesets/cli` v3, and aborts every knope run. The case handling is asymmetric: `readme.md` is skipped, `agents.md` is not, so a lowercase variant of the latter three is parsed as a bump file and is fatal to both readers. Migration warns about all four.
- **No bump files at all** is not an error. It reports that there is nothing to release and exits zero.

## Testing strategy

ADR-0005's Confirmation requires pinning the intersection with tests against **both** foreign parsers, not just oakum's own. The constraints only bind during migration, which is exactly when nobody is looking for them, so a suite that only proves oakum can read what oakum writes proves nothing about the property the ADR is protecting.

- Unit tests: frontmatter parsing and rejection of each item in the not-permitted list, including a scoped name with and without quotes.
- Integration tests: every file oakum writes is fed to `@changesets/cli` and to knope's `changesets` crate as fixtures, asserting each reader either accepts it or fails loudly. A file that one reader skips with exit 0 and no output is the failure this catches, and it is invisible to any assertion made against oakum alone.

## Open questions

- How to express "this change ships no release". Neither `none` nor an empty file survives all three parsers, so it needs a non-`.md` marker, and the shape is undecided. Two shapes are needed, not one: `--empty` wants a marker that only has to exist, while `--none` has to name packages and carry a note, since bumpy's semantics are that a `none` package takes no direct bump and still accepts a cascade. Settling this also amends [ADR-0023](../decisions/0023-name-every-verb-and-what-it-owns.md), whose `add` row grants only a `.md` write. And it owes a lifecycle as well as a shape: the `.md` extension is the identity `version` uses to delete a consumed file, so a marker outside it has no consumption rule and would otherwise survive every release.
- ~~Whether a repository can opt out of bump files entirely in favor of conventional commits.~~ **Answered 2026-08-18 by [ADR-0019](../decisions/0019-both-change-files-and-commits-each-disableable.md)**: it can, and it can opt out of commit parsing too — either mechanism alone is a complete configuration. What remains open is how the two *compose* when both are enabled, and whether a commit mapping to a package a bump file already covers is ignored, merged, or reported as a conflict.
- ~~What `oakum add`'s non-interactive interface is.~~ **Answered 2026-08-18, written into the Interface section 2026-08-19.** It adopts bumpy's surface; the template had shipped `--packages` and `--message` only, and this spec's silence about the rest let `add` go missing from [ADR-0023](../decisions/0023-name-every-verb-and-what-it-owns.md) until a review caught it. Two of the six flags remain blocked on the marker question above.

## Change log

- 2026-08-18: initial draft (v0.1)
- 2026-08-18: opting out of bump files closed by ADR-0019; the composition question stays open (v0.1)
- 2026-08-19: `add`'s flags written into the Interface section; the skip list corrected to four names with its case asymmetry; ADR-0023 added to the driving list (v0.1)
