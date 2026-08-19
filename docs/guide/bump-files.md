# Writing bump files

> Oakum is pre-release. This describes intended behavior; nothing here works yet.

A bump file says which packages a change affects, how far to bump each, and what to tell users about it. You write one alongside the change, and oakum consumes it when the release is cut.

## Creating one

```bash
oakum add --interactive
```

This asks which packages changed, at what level, and for a summary, then writes a file into `.changeset/`. It is an ordinary file — commit it with your change.

Without `--interactive` it does not prompt at all, which is what makes it usable from a script or an agent:

```bash
oakum add --packages "my-package:minor" --message "What changed and what you do differently."
```

Or write the file yourself. There is nothing magic about the ones `oakum add` produces:

```markdown
---
oakum: minor
---

Bump files can now be written by hand.
```

## Choosing a level

- **patch** — a fix. Behavior that was wrong is now right.
- **minor** — something new that does not break existing usage.
- **major** — a change that breaks existing usage.

For an application rather than a library, the distinction carries no compatibility contract, and only the changelog grouping differs. Pick the one that reads correctly and do not agonize.

**You do not write bump files for packages that merely depend on what changed.** Oakum derives those from the dependency graph. If you find yourself writing one because a dependent needs releasing too, that is a bug in the derivation — run `oakum check --explain` and report what it says.

## One change, several packages

List them all:

```markdown
---
oakum: minor
oakum-schema: patch
---

Plans now record which parser rejected a bump file.
```

## Several files for one package

Accumulating files is normal and correct. Write one per change rather than editing an existing file. At release time the highest level wins and every note appears in the changelog, so three patches and one minor produce one minor release with four entries.

## Writing the summary

The summary is the changelog entry. It is read by someone deciding whether to upgrade, so write it for them rather than for the reviewer of your pull request.

Say what changed and what the reader does differently. "Fixed a bug" tells them nothing; "Bump files that name an unknown package now report the file path instead of aborting the run" tells them whether it affects them.

Markdown works. Keep it short — a sentence or two for most changes, a paragraph when the upgrade needs explaining.

## What not to put in `.changeset/`

Every `.md` file directly inside `.changeset/` is treated as a bump file. Notes to yourself, templates, and scratch files belong elsewhere: oakum tries to parse them, and a file that names something other than a package in your workspace is an error that names the file and the unknown name. It is not silently discarded, and it is not released.

Files without a `.md` extension are ignored, which is why oakum's own `_config.toml` lives there safely.

Leave any `.changeset/README.md` from `@changesets/cli` where it is. `oakum init` and `oakum migrate` write their own only when none is present, so deleting yours means oakum writes one back rather than that you get a better one. Oakum skips it by name, along with `AGENTS.md`, `CLAUDE.md`, and `GEMINI.md`.

Those three are matched exactly where `README.md` is matched case-insensitively, so a lowercase `agents.md` is parsed as a bump file rather than skipped. Agent notes in this directory need a different name, not a different case.

Knope is the exception, and not one you fix by deleting the README: knope treats every `.md` in that directory as a bump file and aborts its whole run on the first parse failure, so oakum's README breaks it by design. `migrate` writes it anyway and says so. The fix is removing `knope.toml` and its workflow, not the README.

## Checking before you push

```bash
oakum check
```

This validates every bump file, names any that are malformed, and reports what the next release would contain. It writes nothing.
