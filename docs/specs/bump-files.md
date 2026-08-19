# Bump files

- Status: draft
- Version: 0.1
- Last updated: 2026-08-18
- Driving ADRs: ADR-0005

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

## Behavior

**Writing.** `oakum add` creates one file per invocation. Multiple bump files for the same package accumulate; the highest level among them wins, and every note appears in the changelog.

**Reading.** Every `.md` file directly inside `.changeset/` is a bump file. Subdirectories are not read. Anything without a `.md` extension is ignored, which is what makes `_config.toml` and no-op markers safe to store alongside.

**Consuming.** `oakum version` applies the plan and deletes the files it consumed. A file naming only packages that saw no release is left in place.

## Edge cases

- **A package name that is not in the workspace** is an error naming the file and the unknown name. `@changesets/cli` treats this as fatal, and knope ignores it silently; erroring is the safer of the two.
- **A malformed file** is reported by path and skipped, and the run continues. Both other parsers abort the entire run without naming the file, which turns a typo into a manual bisect.
- **`.changeset/README.md`**, created by `changeset init`, is skipped by oakum, but it aborts every knope run. Migration warns about it.
- **No bump files at all** is not an error. It reports that there is nothing to release and exits zero.

## Testing strategy

ADR-0005's Confirmation requires pinning the intersection with tests against **both** foreign parsers, not just oakum's own. The constraints only bind during migration, which is exactly when nobody is looking for them, so a suite that only proves oakum can read what oakum writes proves nothing about the property the ADR is protecting.

- Unit tests: frontmatter parsing and rejection of each item in the not-permitted list, including a scoped name with and without quotes.
- Integration tests: every file oakum writes is fed to `@changesets/cli` and to knope's `changesets` crate as fixtures, asserting each reader either accepts it or fails loudly. A file that one reader skips with exit 0 and no output is the failure this catches, and it is invisible to any assertion made against oakum alone.

## Open questions

- How to express "this change ships no release". Neither `none` nor an empty file survives all three parsers, so it needs a non-`.md` marker file, and the shape of that is undecided.
- Whether a repository can opt out of bump files entirely in favor of conventional commits, and how the two compose when both are enabled.
- **What `oakum add`'s non-interactive interface is.** Three documents currently give three answers: `templates/changeset-readme.md` ships `oakum add --packages "pkg:minor" --message "…"` into user repositories as the scripted path, this spec defines no flags at all, and `docs/guide/bump-files.md` says to skip the prompts by writing the file yourself. The template is the one that escapes into other people's repositories, so it is the copy that has to be right — but the flags it names were never specified here, and [`init`](init.md) is the only command carrying a runnable-non-interactively requirement.

## Change log

- 2026-08-18: initial draft (v0.1)
