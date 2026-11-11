# Writing bump files

A bump file says which packages a change affects, how far to bump each, and what to tell users about it. You write one alongside the change, and oakum consumes it when the release is cut.

This repository uses change files and conventional commits both (`change-files = true`, `conventional-commits = true`): the plan reads only bump files, and a branch whose commits all release nothing needs none ([What covers a change](coverage.md)). oakum's version is its install pin, here the Cargo workspace member `crates/oakum`, so run the tree's own binary with `mise run oakum -- …` ([ADR-0038](../decisions/0038-the-install-pin-is-the-version.md); [ADR-0022](../decisions/0022-zero-major-versioning.md) for zero-major below 1.0.0).

## Creating one

`oakum add` writes one `.changeset/*.md` file. Flags cover the same values as the prompt:

| Flag | Effect |
|---|---|
| `--packages <list>` | Comma-separated `name:level` pairs (`core:minor,utils:patch`). Levels may include `none`. |
| `--message <text>` | Changelog note body. |
| `--name <slug>` | Filename stem, slugified. Defaults to a generated name. |
| `--interactive` | Guided prompts. Exits non-zero when stdin is not a terminal. |
| `--empty` | Empty frontmatter (intentionally releaseless). Cannot combine with `--packages` or `--none`. |
| `--none` | `name: none` coverage entries. Requires `--packages` with `name:none` pairs. |

A flagless `oakum add` exits non-zero:

```text
error: `oakum add` needs `--packages <list>`, `--empty`, `--none`, or `--interactive`
```

`--interactive` without a terminal:

```text
error: `--interactive` needs a terminal; use `--packages <list>` (and optionally `--message` / `--name`) instead
```

Non-interactively:

```bash
oakum add --packages "my-package:minor" --message "What changed and what you do differently."
```

`--empty` and `--none`:

```bash
oakum add --empty --message "docs only"
oakum add --none --packages "my-package:none" --message "covered without a release"
```

Or write the file by hand; hand-written files work the same:

```markdown
---
oakum: minor
---

Bump files can now be written by hand.
```

`oakum add` prints the path it wrote (for example `.changeset/guide-example.md`) and exits 0. Package names are unquoted except a scoped npm name, which must be quoted; single or double quotes both parse, so a Prettier `singleQuote` rewrite is harmless.

## Choosing a level

- **patch** — a fix. Behavior that was wrong is now right.
- **minor** — something new that does not break existing usage.
- **major** — a change that breaks existing usage.

For an application rather than a library, the levels carry no compatibility contract, but they still choose which SemVer component advances, so the resulting version and tag differ. Pick the one that reads correctly.

For oakum itself the line between the levels is written down, with the contract surface it rests on: [contributing/task-tracking.md](../contributing/task-tracking.md#version-labels).

**A package that merely depends on what changed releases without being named.** Oakum derives that from the dependency graph and shows it in `oakum status`: a library whose published range still covers the new version does not release, and a delivery artifact such as a binary always does, at patch. Name a dependent only to give it more than the cascade: a binary whose users see a library's new feature takes `minor` beside the library.

`oakum check --explain` says why, after the looks: each planned bump, and each dependency onto a bumped package, whether it cascades and why, with the range its manifest declares when a range decided it. A version the zero-major setting changed names the setting. The trace prints whatever `check` decides; when the plan cannot be built, it traces nothing and says so. It is text only; `--json` carries no explanation.

```text
explain:
  bump cli (cargo): 0.1.0 → 0.1.1 (patch); cascade from core (cargo)
  bump core (cargo): 0.1.0 → 0.1.1 (patch); intent
  edge core (cargo) → cli (cargo) (normal as core): always → cascade patch
  edge core (cargo) → lib (cargo) (normal as core): if-range-unsatisfied; declared ^0.1.0 → skip (range still satisfied)
```

A skipped edge says which of these held: the range still admits the new version, the dependency is a development one, or the dependent is not version-managed (left out by `include`/`exclude`, or private with `private-packages.version` unset). Nothing cascades through a package that is not version-managed, so a managed dependent releases through a cascade only when a managed package's edge onto it fires. If a release you expected is missing from `oakum status` and the explanation does not account for it, paste both and report it.

## One change, several packages

List them all:

```markdown
---
core: minor
cli: patch
---

Plans now record which parser rejected a bump file.
```

## Several files for one package

Accumulating files is expected. Write one per change rather than editing an existing file. At release time the highest *release* level wins and every note appears in the changelog, so three patches and one minor produce one minor release with four entries.

## Writing the summary

The summary is the changelog entry. It is read by someone deciding whether to upgrade, so write it for them rather than for the reviewer of your pull request.

Say what changed and what the reader does differently. "Fixed a bug" tells them nothing; "Bump files that name an unknown package now report the file path instead of aborting the run" tells them whether it affects them.

Markdown works. Keep it short: a sentence or two for most changes, a paragraph when the upgrade needs explaining.

The level picks the changelog section: `patch` under `### Fixed`, `minor` under `### Added`, `major` under `### Changed`. A patch is not always a fix, so when the level would mislabel the entry, name the section yourself: one of Keep a Changelog's headings (`### Added`, `### Changed`, `### Deprecated`, `### Removed`, `### Fixed`, `### Security`) puts what follows it under that heading, and the heading line is dropped from the entry. A summary may declare more than one, so a single bump file can carry an `### Added` note and a `### Changed` note; text above the first heading falls under the level's own section. A heading is one only at the start of a line, indented no more than three spaces, and outside a fenced block — anything else, including `### Notes`, is body text. `oakum add --section changed --message "..."` writes it. Use it in files only oakum will render: changesets' default changelog emits the summary as a list item, so the heading line becomes a nested `###` there. A heading with nothing under it reaches no changelog, so `oakum add` refuses such a note and `oakum check` reports it (refusing under `--strict`), naming the empty heading even when another one in the same note renders.

## Rendering the entry with a template

The builtin entry is `## <version> (<date>)` followed by the sections above. A `template` in `_config.toml` (inline text or `{ file = "..." }`) renders one version section instead, with these values:

| Value | Meaning |
| --- | --- |
| `version`, `date` | the new version and the run date, `YYYY-MM-DD` |
| `package`, `ecosystem`, `bump` | package name, `cargo` or `npm`, the effective level |
| `source`, `trigger` | `intent` or `cascade`; for a cascade, the package that triggered it |
| `notes` | each note body, in bump-file order, with section headings removed — a note declaring two sections contributes two |
| `changes` | one entry per section of each note, so a bump file declaring two sections is two entries sharing one file's provenance: `note`, `section`, `level`, `file` (the bump file name), and, when the file is committed, `commit` (`sha`, `short`, `url`), `pr` (`number`, `url`, from a `(#N)` squash-merge subject), and `author` (`name`, `email`); empty when `--notes-file` supplies the body |
| `repo` | `owner`, `name`, `url` when the `origin` remote or `GITHUB_REPOSITORY` names a GitHub repository |
| `tool_version`, `target` | the running oakum and `changelog` |

The section must open with a heading oakum can read back, because `check` and `release` find a version by it: `## {{ version }}`, `## [{{ version }}]`, `## v{{ version }}` or `## {{ package }}@{{ version }}`, with anything after it separated by a space or `(`, as in `## {{ version }} ({{ date }})` or `## [{{ version }}] - {{ date }}`. A `<name>@` heading counts only for the package it names. When more than one package oakum versions or tags shares a `CHANGELOG.md`, only `## {{ package }}@{{ version }}` reads back, and the builtin entry uses that heading. A heading inside a code fence, an HTML comment or a `<details>` block is not a section, and a changelog that leaves a comment or `<details>` block open is refused until it is closed. Every bumped version needs its section, a cascade with no notes included. `version` refuses, before it writes anything, a template that renders another heading or renders nothing, an entry that leaves a code fence, comment, `<details>`, `<pre>`, `<script>`, `<style>` or `<textarea>` block open (naming the bump file whose note opened it), and a changelog whose own open block would hide the new heading. `check` renders the pending plan's entries through the same template and notes, and exits 1 on any entry `version` would refuse, so a template that fails only for a cascade, or a note left open, fails the pull request rather than the version job; with nothing pending it renders nothing. Two inputs differ on a branch. The date is today's. A bump file the base does not hold yet has the branch's commit, pull request and author, and the merge replaces them: an entry that fails only because `changes` has no commit or `pr` yet, as for an uncommitted bump file or a subject without the squash merge's `(#N)`, is rendered again with stand-ins for them and passes when that render does, with a `changelogs:` line naming the entry. `version` then renders it only if the merge's subject carries the pull request, as a squash merge's does and a merge commit's does not. A bump file the base already holds keeps its own provenance, so the same failure there is an error, and when the stand-in render fails too, `check` names its error, the one the merge leaves. When the version job passes `--notes-file`, pass the same file to `check`.

`changes` and `repo` are read from git only when the template names them at the top level, one `git log` per bump file. Off GitHub, `commit` and `pr` still carry the hash and number; their `url` values are absent, so guard on them. A template that wants changesets-style attribution, with the loop's own newlines trimmed so the entry keeps single blank lines:

```jinja
## {{ version }} ({{ date }})
{% for c in changes %}
- {{ c.note }}{% if c.pr and c.pr.url %} ([#{{ c.pr.number }}]({{ c.pr.url }})){% endif %}{% if c.author %} by {{ c.author.name }}{% endif %}
{%- endfor %}
```

## The tag `release` cuts

`release` tags each version it releases. With `tag-format` unset, the tag is `v{{ version }}` when one package is tagged and `{{ package }}/v{{ version }}` when several are: `v1.2.0`, or `@demo/a/v1.2.0` in a pnpm workspace. A package is tagged when `include`/`exclude` keep it and it is publishable or `private-packages.tag` opts it in, so an unpublished helper crate does not turn a single package's tags into prefixed ones.

`tag-format` replaces the default with a template that receives `package` and `version`: a string, or `{ file = "path" }` for a repository-relative file. `check`, `version` and `release` read tags back to learn which versions are released, so the template has to render a shape they read: `{{ package }}@{{ version }}`, `{{ package }}/v{{ version }}`, `{{ package }}-v{{ version }}`, or `v{{ version }}` when one package is tagged. `release` refuses a template that renders anything else, such as `rel-{{ package }}-{{ version }}`, before it creates any tag, and `check` and `version` name no tag in their advice for an untagged version, because `check` would report a tag of that shape as leftover.

## Retiring a package

To stop releasing a package but keep its code, list it in `exclude`. To remove it, delete its directory, then the lines that still name it: its `[packages.<name>]` table, any `include`/`exclude` entry, and its lines in pending bump files. Nothing else is needed. Its tags stay as history, and its published releases with them; do not delete them.

`check` and every command that reads tags look at the commit a removed package's newest tag points at. When a package oakum tags now lives where that commit declared the removed one, under another name and with no tags of its own, the package was renamed, and its history would read as never released: the command exits 2 and names the tag to add, such as `git tag new@0.1.2 <commit>`. Otherwise its tags are history and count for nothing, except a bare `v<version>` tag on the same commit as one of them: either package could have released it, so the command exits 2 until you delete it, first tagging that commit under its package's name when the bare tag released a package that remains. A package moved to a new directory and renamed in the same change reads as removed, so its new name starts untagged.

The text output never mentions a retired package. `status --json` lists each one under `retired`, as `{ "name", "ecosystem", "version", "tag" }` for its newest release, and `retired_look` says whether the tags were read: `ran`; `failed` when they could not be read or a rename stopped the read; or `not-asked` where nothing read them, as in the `commit-message` and `title` templates `ci version-pr` renders. Only `ran` means an empty `retired` is none. `check --json` carries the same list on the tags look (`looks[]` where `look` is `tags`) as `retired`, absent when that look could not answer.

A package added back under a retired name continues from its old tags, as npm and crates.io require, since neither lets a published version be published again. Deprecating or yanking what is already on a registry (`npm deprecate`, `cargo yank`) is a step oakum does not take for you.

## What not to put in `.changeset/`

Every `.md` file directly inside `.changeset/` is treated as a bump file, except four instruction names: `README.md` matched case-insensitively, and `AGENTS.md`, `CLAUDE.md`, and `GEMINI.md` matched exactly. A lowercase `agents.md` is parsed as a bump file. Notes to yourself, templates, and scratch files belong elsewhere: a file that names something other than a package in your workspace is an error that names the file and the unknown name.

Files without a `.md` extension are ignored, which is why oakum's own `_config.toml` lives there safely.

Leave any `.changeset/README.md` from `@changesets/cli` where it is. `oakum init` and `oakum migrate` write their own only when none is present, so deleting yours just makes oakum write one back.

Knope treats every `.md` in that directory as a bump file and aborts its whole run on the first parse failure, so oakum's README breaks it by design. `migrate` writes it anyway and says so. The fix is removing `knope.toml` and its workflow, not the README.

## Checking before you push

```bash
oakum check
```

Reports tag drift (a manifest above a reachable tag) and, when change files are on, packages that changed with no covering bump file. It also reports a `CHANGELOG.md` that `version` would refuse to append to, such as one still titled with the package name from changesets: change the first line to `# Changelog`. It renders the entries the pending bump files would add and fails on any `version` would refuse to write; `--notes-file <path>` renders them with that body in place of the notes, as `version --notes-file` does. It writes nothing.

In a pnpm workspace, the `lockfiles` look compares the specifiers `pnpm-lock.yaml` records with what the workspace declares, and only those: each workspace project's importer (the projects `pnpm list` reports, private and unversioned ones included, not every importer the lockfile kept) against its `package.json`, and each catalog entry the lockfile records against `pnpm-workspace.yaml`. It exits 1 on a specifier that differs, a dependency one side has and the other lacks (including one moved between `dependencies`, `devDependencies` and `optionalDependencies`, and a project with dependencies but no importer), and a recorded catalog entry `pnpm-workspace.yaml` gives another value or none. Specifiers compare as written, so `workspace:*`, `catalog:`, `catalog:<name>`, `link:`, `file:` and `npm:<name>@<range>` must match exactly. A dependency declared in two sections counts where pnpm files it (`optionalDependencies`, then `dependencies`, then `devDependencies`), and a peer counts under `dependencies` only when the lockfile's `autoInstallPeers` is on and no other section declares it. Some entries are skipped rather than compared, and the look prints a `lockfiles:` line counting them: a dependency whose recorded specifier is the value of an override the lockfile's own `overrides` lists for it, with a `link:` or `file:` value read from the dependent's directory, or which a `-` override removes when it names no parent or names this project's package (pnpm records an applied override in place of the declared range; one whose parent or range rules this dependent out is listed but not applied, so the dependency still compares; pnpm 12.16.1 applies `pnpm-workspace.yaml`'s `overrides` and ignores a root `package.json`'s `pnpm.overrides`), an optional dependency the lockfile's `ignoredOptionalDependencies` matches (as pnpm reads it: `*` matches any characters, `/` included, a `!` pattern excludes, the last matching pattern decides, and a list opening with `!` starts from every name), and, when the lockfile records `packageExtensionsChecksum`, an entry the manifest does not declare, since a package extension adds it. What the manifest declares is still compared. With `excludeLinksFromLockfile: true`, pnpm leaves `link:` dependencies out of the lockfile, which the look accepts; it also leaves out `file:` and `catalog:` ones that resolve to a workspace package, and pnpm 12.16.1 then refuses its own lockfile, so those exit 2. A lockfile recording a pnpmfile exits 2 too, since its `readPackage` hook can rewrite any specifier, as do a `lockfileVersion` other than `'9.0'` and a manifest or entry the look cannot read. Settings, `dependenciesMeta`, `publishConfig.directory` and patched dependencies are not compared; `pnpm install --frozen-lockfile` catches those. Each finding names the lockfile entry and both values; the fix is `pnpm install`, which `check` names and never runs. A clean comparison with nothing skipped says nothing. Without a pnpm workspace or a `pnpm-lock.yaml` the look says nothing.

A `.<name>.oakum-write.*` file is a staging file from an oakum write that did not finish. `check` names one in `.changeset/`, at the repository root, or beside any file `oakum version` writes (manifests, changelogs, lockfiles, the pnpm catalog, declared extra files) or the file a symlink there points to; `init` and `migrate` name one in `.changeset/`. None of them removes it, since a run still in progress could own it. Once no oakum run is in progress, remove it yourself.

A `.<name>.oakum-consume.*` file in `.changeset/` is left by a `version` that did not finish. It is either a bump file that run set aside or `.version.oakum-consume.*`, the marker the run held while it wrote and consumed. Don't remove these on sight: `version` refuses to run while they are there, and its refusal says what happened and how to recover.

Until an install pin exists in `.github/workflows`, `.github/actions`, `package.json`, `.mise.toml`, `mise.toml`, or a Cargo workspace member named `oakum`, it reports `unverified` instead.

On a pinned repository whose tags match the manifests, whose bump files parse, and whose changed packages are covered, it exits 0 and prints only what it looked at and how each changed package is covered. [What covers a change](coverage.md) explains those lines, the `--json` fields that carry them, and what `--strict` asks of an uncovered package.

A malformed bump file fails `check` (and `status` and `version`), named with its parse error, so a formatter that rewrites one cannot turn a release into a silent no-op. A body that is not frontmatter prints:

```text
error: `broken.md` is not a bump file: bump file must start with --- on line 1
```

`--strict` also fails when a changed package has no covering intent, naming the commits that ask to release it and the `oakum add` commands that cover it; see [What covers a change](coverage.md). The workflow printed by `init` and `migrate` runs `check --strict`.

A bump file naming a package oakum cannot version is refused whether or not `--strict` is set, because the file can never be honoured: the release it asks for would not happen. That covers an unpublishable package with `private-packages.version` unset, and one `include`/`exclude` leaves out. `status` and `version` refuse it in the same words, and `oakum add` refuses to write one; a `name:none` entry releases nothing and is accepted. A package that merely *changed* without a bump file is different: its change is the change of the versioned packages that ship it, as [What covers a change](coverage.md#what-counts-as-changed) says, and a `fix` commit to it releases them. Otherwise nothing is reported, because not versioning your private packages is a choice rather than an omission. With commits-only intent, a commit whose scope names such a package and that no versioned package ships releases nothing; `status` lists it under "Named by a commit but not version-managed", and `check`, `version` and `ci version-pr` name it without failing. An excluded package is silent in both lists. A `feat`, `fix` or breaking commit whose scope names no package (`fix(gamma)`, or `fix(alpha,beta)`, since a scope names one package) and whose files are under no package releases nothing too; `status` lists it under "Released by a commit but reaching no package", and the same commands name it.

The next-release table is `oakum status`, not `check`:

```bash
oakum status
```

```text
## Release plan

| Package | From | To | Bump | Source |
| --- | --- | --- | --- | --- |
| demo (`cargo`) | 0.1.0 | 0.2.0 | minor | intent |
```
