# Writing bump files

A bump file says which packages a change affects, how far to bump each, and what to tell users about it. You write one alongside the change, and oakum consumes it when the release is cut.

This repository uses change files (`change-files = true`) and not conventional commits. Commits never cover a package. Pin `tool-version` in `.changeset/_config.toml` to the oakum binary you run ([ADR-0022](../decisions/0022-zero-major-versioning.md) for zero-major below 1.0.0).

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

**You do not write bump files for packages that merely depend on what changed.** Oakum derives those from the dependency graph. They show up in `oakum status` when they will release. If you find yourself writing one because a dependent needs releasing too, that is a bug in the derivation: paste the `oakum status` table and report it.

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

The level picks the changelog section: `patch` under `### Fixed`, `minor` under `### Added`, `major` under `### Changed`. A patch is not always a fix, so when the level would mislabel the entry, name the section yourself: a summary whose first line is one of Keep a Changelog's headings (`### Added`, `### Changed`, `### Deprecated`, `### Removed`, `### Fixed`, `### Security`) goes under that heading, and the line is dropped from the entry. `oakum add --section changed --message "..."` writes it. Use it in files only oakum will render: changesets' default changelog emits the summary as a list item, so the heading line becomes a nested `###` there.

## Rendering the entry with a template

The builtin entry is `## <version> (<date>)` followed by the sections above. A `template` in `_config.toml` (inline text or `{ file = "..." }`) renders one version section instead, with these values:

| Value | Meaning |
| --- | --- |
| `version`, `date` | the new version and the run date, `YYYY-MM-DD` |
| `package`, `ecosystem`, `bump` | package name, `cargo` or `npm`, the effective level |
| `source`, `trigger` | `intent` or `cascade`; for a cascade, the package that triggered it |
| `notes` | each note body, in bump-file order, with any opening section heading removed |
| `changes` | one entry per note: `note`, `section`, `level`, `file` (the bump file name), and, when the file is committed, `commit` (`sha`, `short`, `url`), `pr` (`number`, `url`, from a `(#N)` squash-merge subject), and `author` (`name`, `email`); empty when `--notes-file` supplies the body |
| `repo` | `owner`, `name`, `url` when the `origin` remote or `GITHUB_REPOSITORY` names a GitHub repository |
| `tool_version`, `target` | the running oakum and `changelog` |

`changes` and `repo` are read from git only when the template names them at the top level, one `git log` per bump file. Off GitHub, `commit` and `pr` still carry the hash and number; their `url` values are absent, so guard on them. A template that wants changesets-style attribution, with the loop's own newlines trimmed so the entry keeps single blank lines:

```jinja
## {{ version }} ({{ date }})
{% for c in changes %}
- {{ c.note }}{% if c.pr and c.pr.url %} ([#{{ c.pr.number }}]({{ c.pr.url }})){% endif %}{% if c.author %} by {{ c.author.name }}{% endif %}
{%- endfor %}
```

## What not to put in `.changeset/`

Every `.md` file directly inside `.changeset/` is treated as a bump file, except four instruction names: `README.md` matched case-insensitively, and `AGENTS.md`, `CLAUDE.md`, and `GEMINI.md` matched exactly. A lowercase `agents.md` is parsed as a bump file. Notes to yourself, templates, and scratch files belong elsewhere: a file that names something other than a package in your workspace is an error that names the file and the unknown name.

Files without a `.md` extension are ignored, which is why oakum's own `_config.toml` lives there safely.

Leave any `.changeset/README.md` from `@changesets/cli` where it is. `oakum init` and `oakum migrate` write their own only when none is present, so deleting yours just makes oakum write one back.

Knope treats every `.md` in that directory as a bump file and aborts its whole run on the first parse failure, so oakum's README breaks it by design. `migrate` writes it anyway and says so. The fix is removing `knope.toml` and its workflow, not the README.

## Checking before you push

```bash
oakum check
```

Reports tag drift (a manifest above a reachable tag) and, when change files are on, packages that changed with no covering bump file. It also reports a `CHANGELOG.md` that `version` would refuse to append to, such as one still titled with the package name from changesets: change the first line to `# Changelog`. It writes nothing.

A `.<name>.oakum-write.*` file is a staging file from an oakum write that did not finish. `check` names one in `.changeset/`, at the repository root, beside a package manifest, or beside a declared extra file; `init` and `migrate` name one in `.changeset/`. None of them removes it, since a run still in progress could own it. Once no oakum run is in progress, remove it yourself.

Until an install pin exists in `.github/workflows`, `.github/actions`, `package.json`, `.mise.toml`, `mise.toml`, or a Cargo workspace member named `oakum`, it reports `unverified` instead.

On a pinned repository whose tags match the manifests, whose bump files parse, and whose changed packages are covered, it prints nothing and exits 0.

A malformed bump file fails `check` (and `status` and `version`), named with its parse error, so a formatter that rewrites one cannot turn a release into a silent no-op. A body that is not frontmatter prints:

```text
error: `broken.md` is not a bump file: bump file must start with --- on line 1
```

`--strict` also fails when a changed package has no covering intent, with a hint to add a bump file (or `none` / empty frontmatter). The workflow printed by `init` and `migrate` runs `check --strict`.

A bump file naming a package oakum cannot version is refused whether or not `--strict` is set, because the file can never be honoured: the release it asks for would not happen. That covers an unpublishable package with `private-packages.version` unset, and one `include`/`exclude` leaves out. `status` and `version` refuse it in the same words. A package that merely *changed* without a bump file is different — if oakum does not version it, nothing is reported, because not versioning your private packages is a choice rather than an omission; `oakum status` lists those under "Changed but not version-managed" if you want to see them.

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
