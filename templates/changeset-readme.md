# Bump files

> Oakum is pre-release. This describes intended behavior; nothing here works yet.

This directory is used by [oakum](https://github.com/oakoss/oakum) to manage versions and changelogs.

A **bump file** is a small Markdown file recording one change: which packages it affects, how far each should bump, and what to tell users about it. Files accumulate here as work lands, and are consumed when a release is cut.

## How it works

1. You make a change and commit a bump file alongside it, usually one per pull request.
2. Bump files accumulate on the main branch.
3. At release time oakum merges them into a plan, works out which *other* packages need releasing, updates versions and changelogs, and deletes the files it consumed.

## Creating one

Interactively:

```bash
oakum add --interactive
```

Non-interactively, which is also the path to use from a script or an agent:

```bash
oakum add --packages "my-package:minor" --message "What changed and what you do differently."
```

Or write the file yourself. There is nothing special about the ones `oakum add` produces:

```markdown
---
my-package: minor
---

Bump files can be written by hand.
```

Package names are **unquoted**. Other tools read this directory, and at least one of them treats a quoted name as a package that does not exist — it will skip the file without reporting anything.

Scoped npm names are the exception and must be quoted, because `@` starts a reserved token in YAML:

```markdown
---
'@scope/my-package': minor
---
```

## What you do not write

**You never write a bump file for a package that merely depends on what you changed.** Oakum works that out from the dependency graph, including whether the dependent actually needs releasing at all — a library whose published range still covers the new version usually does not, while a binary that bakes its dependencies in at build time always does.

If you find yourself adding a bump file because something downstream also needs releasing, that is a bug in oakum rather than a step you were supposed to take. Run `oakum check --explain`, which states the reasoning for every package it decided *not* to bump, and report what it says.

## Choosing a level

- **patch** — a fix; behavior that was wrong is now right
- **minor** — something new that does not break existing usage
- **major** — a change that breaks existing usage

Only these three. For an application rather than a library there is no compatibility contract behind them, and the choice only affects how the changelog groups the entry.

## Changes that ship no release

A pull request that needs no package covered can omit a bump file entirely when `oakum check` is not in `--strict` mode.

When you need a file — for example to cover packages under a strict coverage gate without releasing them — use the same shapes as bumpy and changesets:

- Empty frontmatter (`---` then `---` with no package lines) for an intentionally releaseless change
- `package: none` for a package that takes no direct bump but still accepts a cascade

`oakum add` will gain `--empty` / `--none` for these shapes; until then write the file by hand. Do not introduce those files while knope is still the repository's release tool: knope treats `none` as a patch and rejects empty frontmatter.

## Keeping them current

A bump file is part of the change, like a test. If a pull request grows from a fix into a feature, update the level and the summary to match before merging — otherwise the release understates what shipped, and the changelog misleads the person deciding whether to upgrade.

Reviewers and agents should read the bump file as part of reviewing the diff.

## Writing the summary

The summary becomes the changelog entry, read by someone deciding whether to upgrade. Say what changed and what they do differently. "Fixed a bug" tells them nothing; "A bump file naming an unknown package now reports the file path instead of aborting the run" tells them whether it affects them.

Markdown works. A sentence or two suits most changes.

## Files in this directory

- `_config.toml` — oakum's configuration
- `_schema.json` — generated; validates the config in your editor
- `*.md` — pending bump files

Everything ending in `.md` here is treated as a bump file, so notes to yourself and scratch files belong somewhere else. Anything without a `.md` extension is ignored.

Four names are skipped: this `README.md`, and `AGENTS.md`, `CLAUDE.md`, and `GEMINI.md`. The last three are matched exactly, so a lowercase `agents.md` is *not* skipped — it is parsed as a bump file, fails, and takes the run with it. If an agent writes notes in this directory, give them any other name.

## Checking your work

```bash
oakum check
```

Validates every bump file, names any that are malformed, and reports what the next release would contain. It writes nothing, so it is safe to run anywhere.

Full documentation: <https://github.com/oakoss/oakum/tree/main/docs>
