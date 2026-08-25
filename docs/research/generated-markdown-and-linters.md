# Generated markdown and the repository linter

- Date: 2026-08-25
- Author: Jace Babin (agent-assisted)
- Scope: how oakum's generated changelog and bump files should relate to a project's formatter and linter; evidence for [ADR-0031](../decisions/0031-write-generated-markdown-genre-intersection.md)

## Question

A release tool writes markdown it does not lint. The repository lints that markdown with its own config. When the two disagree, the version PR cannot merge, and the failure lands on a branch the tool created. [okm-v2y] asks which of four shapes oakum should take, given [ADR-0003](../decisions/0003-write-only-what-a-command-owns.md), [ADR-0004](../decisions/0004-derive-facts-configure-preference.md), [ADR-0005](../decisions/0005-write-the-changeset-format-intersection.md), [ADR-0006](../decisions/0006-no-command-execution-in-templates.md), and [ADR-0023](../decisions/0023-name-every-verb-and-what-it-owns.md).

The live collision is already recorded: [changelog-lint-collision.md](changelog-lint-collision.md) (2026-08-19). This note does not replace it. It answers the questions that note left open and surveys what peers do.

Four shapes, and a fifth to reject on the record:

1. Write the strict intersection: emit markdown that satisfies the common rules regardless of config. Same strategy [ADR-0005](../decisions/0005-write-the-changeset-format-intersection.md) uses for changeset files.
2. Read the repository's lint config and conform.
3. Run the repository's own formatter over the file oakum wrote.
4. Emit and let a pre-commit hook fix it.
5. Exclude generated files from linting. [ADR-0031](../decisions/0031-write-generated-markdown-genre-intersection.md) exists so this option is not omitted.

## Sources

- [Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/), fetched 2026-08-25 — official sample: `# Changelog`, `## [1.1.2] - 2024-09-27`, repeated `### Added` / `### Fixed`, blank line after every heading, no HTML
- [changelog-lint-collision.md](changelog-lint-collision.md) and `crates/oakum/tests/fixtures/changelog-lint/`, captured 2026-08-19 — bumpy 1.18.1 vs `markdownlint-cli2` 0.23.2 (`markdownlint` 0.41.1) on `oakoss/claude-plugins` PR #26, CI run `32224322374`
- DavidAnson/markdownlint, fetched 2026-08-25:
  - [README](https://github.com/DavidAnson/markdownlint) (`main`): all rules enabled when no config is passed
  - [MD022](https://raw.githubusercontent.com/DavidAnson/markdownlint/main/doc/md022.md) — blanks around headings; default 1 above and 1 below; fixable
  - [MD012](https://raw.githubusercontent.com/DavidAnson/markdownlint/main/doc/md012.md) — consecutive blanks; default maximum 1; fixable
  - [MD024](https://raw.githubusercontent.com/DavidAnson/markdownlint/main/doc/md024.md) — duplicate headings; default `siblings_only: false`; docs name changelogs as the `siblings_only: true` case
  - [MD013](https://raw.githubusercontent.com/DavidAnson/markdownlint/main/doc/md013.md) — line length; default 80
- [MD033](https://raw.githubusercontent.com/DavidAnson/markdownlint/main/doc/md033.md) — inline HTML; default `allowed_elements: []`; no fix
- [MD036](https://raw.githubusercontent.com/DavidAnson/markdownlint/main/doc/md036.md) — emphasis used as a heading; single-line italic/bold paragraphs
- [MD041](https://raw.githubusercontent.com/DavidAnson/markdownlint/main/doc/md041.md) — first line is a top-level heading; YAML front matter counts as a title only if it matches `front_matter_title` (default `^\s*title\s*[:=]`)
- claude-plugins fixture `.markdownlint-cli2.yaml` (captured 2026-08-19): `default: true`, twelve rules off including MD012, MD013, MD024, MD033; MD022 left on
- this repository's `.rumdl.toml` (read 2026-08-25): same subtractive shape; MD022 left on; comment records that rumdl and markdownlint `--fix` disagree
- knope, fetched 2026-08-25:
  - [Changelog](https://knope.tech/reference/concepts/changelog/) — version heading `## 1.2.3 (2023-02-01)`
  - [Release notes](https://knope.tech/reference/config-file/release-notes/) — default templates `["### $summary\n\n$details", "- $summary"]`
  - knope's own [`CHANGELOG.md`](https://raw.githubusercontent.com/knope-dev/knope/main/CHANGELOG.md) (`main`) — blank line after every heading
  - oakoss/linesmith [PR #16](https://github.com/oakoss/linesmith/pull/16) — knope insert fallback tripped MD022/MD032 when headings were Keep-a-Changelog bracketed form
- changesets, fetched 2026-08-25:
  - [`docs/config-file-options.md`](https://raw.githubusercontent.com/changesets/changesets/main/docs/config-file-options.md) — `format`: `"auto"` | `"prettier"` | `"oxfmt"` | `"deno"` | `"dprint"` | `false`
  - [`@changesets/cli@3.0.0`](https://github.com/changesets/changesets/releases/tag/@changesets/cli@3.0.0) — `prettier` removed in favor of `format`
  - [PR #1994](https://github.com/changesets/changesets/pull/1994) (merged 2026-05-13) — `prettier` replaced by `format` through [`@changesets/format`](https://github.com/changesets/format); user-installed formatter. PR #1639 was the formatly attempt; it closed unmerged
  - [`packages/cli/CHANGELOG.md`](https://raw.githubusercontent.com/changesets/changesets/main/packages/cli/CHANGELOG.md) — Keep-a-Changelog sections, blank lines around headings, no date
- release-please, fetched 2026-08-25:
  - [`src/updaters/changelog.ts`](https://github.com/googleapis/release-please/blob/main/src/updaters/changelog.ts) — inserts an entry; new file starts `# Changelog\n`
  - [`test/updaters/changelog.ts`](https://raw.githubusercontent.com/googleapis/release-please/main/test/updaters/changelog.ts) — fixture entries are `## 2.0.0\n\n* added…` (blank line under the heading)
  - [`src/changelog-notes/default.ts`](https://raw.githubusercontent.com/googleapis/release-please/main/src/changelog-notes/default.ts) — `conventional-changelog-writer` + conventionalcommits preset; section headings `Features`, `Bug Fixes`, …; subjects HTML-escaped
  - [issue #2085](https://github.com/googleapis/release-please/issues/2085) (closed 2024-09-11) — generated file failed MD012; maintainer: no post-processing binaries; users may clean up the release PR. Same stance on [issue #1802](https://github.com/googleapis/release-please/issues/1802) (Prettier). Follow-on PRs in other repos exclude `CHANGELOG.md` from lint.
- cargo-dist, fetched 2026-08-25:
  - [Simple application guide](https://axodotdev.github.io/cargo-dist/book/workspaces/simple-guide.html) — reads `CHANGELOG.md` / `RELEASES.md` with `parse-changelog`; does not generate the file
  - [package.changelog](https://axodotdev.github.io/cargo-dist/book/reference/config.html) — path override only
- git-cliff, fetched 2026-08-25:
  - [`examples/keepachangelog.toml`](https://raw.githubusercontent.com/orhun/git-cliff/main/examples/keepachangelog.toml) — `## [version] - YYYY-MM-DD`, `### Added` / `### Fixed`, date in the heading
  - `git-cliff` 2.13.1 render of that template (`--unreleased --tag 1.2.3` against this repository, 2026-08-25): blank line between `## [1.2.3] - 2026-08-25` and `### Added`
  - [templating-prior-art.md](templating-prior-art.md) (2026-08-18/19) — `replace_command` on commit preprocessors and postprocessors
- bumpy, fetched 2026-08-25:
  - [`docs/changelog-formatters.md`](https://raw.githubusercontent.com/dmno-dev/bumpy/main/docs/changelog-formatters.md) — default example is `## 1.2.0` then `_2026-04-19_` (italic), then bullets
  - [`docs/differences-from-changesets.md`](https://github.com/dmno-dev/bumpy/blob/main/docs/differences-from-changesets.md) — "includes the release date in every changelog heading by default"
  - bumpy's own release [eb0f9da](https://github.com/dmno-dev/bumpy/commit/eb0f9da6c6e090929f03bfa5df91a08b906ee0db) (2026-06-03, `@varlock/bumpy` 1.13.0) — `## 1.13.0` then `<sub>2026-06-03</sub>`
  - [bump-file-tool-interfaces.md](bump-file-tool-interfaces.md) (2026-08-18/19) — formatter context includes `target`: `changelog` | `github-release`
- [ADR-0003](../decisions/0003-write-only-what-a-command-owns.md), [ADR-0004](../decisions/0004-derive-facts-configure-preference.md), [ADR-0005](../decisions/0005-write-the-changeset-format-intersection.md), [ADR-0006](../decisions/0006-no-command-execution-in-templates.md), [ADR-0023](../decisions/0023-name-every-verb-and-what-it-owns.md)
- [specs/bump-files.md](../specs/bump-files.md) (2026-08-21) — line 1 is exactly `---`; note after the closing delimiter is Markdown, kept verbatim
- `crates/oakum/src/changeset/format.rs` `write` (read 2026-08-25) — `---\n` plus the note with no inserted blank
- [specs/generate.md](../specs/generate.md) (2026-08-21) — note body is one list line per commit (`- <package>: <summary>`)
- markdownlint-cli2 0.23.2 / markdownlint 0.41.1, no config, 2026-08-25 — a Keep-a-Changelog stub produced zero hits; typical bump files (`---` / `oakum: minor` / `---` / prose) failed only MD041; a long one-line note also failed MD013; `<sub>` also failed MD033

Optional peers, fetched 2026-08-25 and not in the required set: [GoReleaser changelog](https://goreleaser.com/customization/publish/changelog/) is an SCM release body, not a repo `CHANGELOG.md`. [release-plz](https://release-plz.dev/docs/changelog) generates with git-cliff; [tips](https://release-plz.dev/docs/changelog/tips-and-tricks) document HTML comments in group names stripped by `striptags`. [`@semantic-release/changelog`](https://github.com/semantic-release/changelog) writes `CHANGELOG.md` with no documented formatter; [issue #93](https://github.com/semantic-release/changelog/issues/93) is users adding `@semantic-release/exec` + prettier — not shipped docs.

Still unchecked: whether markdownlint custom-rule plugins appear often enough on changelogs to matter. cargo-dist is in the required list as a reader, not a generator.

## Findings

### The rules a generated changelog trips

markdownlint enables every rule when no config is passed ([README](https://github.com/DavidAnson/markdownlint), 2026-08-25). Repository configs in this survey are subtractive from that default: claude-plugins and oakum both turn a dozen rules off and leave MD022 on.

| Rule | Default | Why a changelog hits it | Fixable? |
|---|---|---|---|
| **MD022** | 1 blank above and below every heading | Date, HTML, or a list on the next line. The claude-plugins failure. | Yes — insert blanks |
| **MD012** | at most one consecutive blank | bumpy's four blank lines after the preamble (collision note). release-please [issue #2085](https://github.com/googleapis/release-please/issues/2085): extra blank before `###`. Hidden when the repo disables it. | Yes — collapse |
| **MD024** | `siblings_only: false` — any duplicate heading in the file | Official [Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/) repeats `### Added` / `### Fixed` under every version. markdownlint's own docs give this example and say set `siblings_only: true` "as is common in changelogs". | No useful fix — renaming `### Added` abandons the genre |
| **MD033** | no HTML; `allowed_elements: []` | `<sub>date</sub>`, `<!-- generated by … -->`. No fix defined. | No |
| **MD036** | single-line italic/bold paragraph is a fake heading | bumpy's current-docs `_2026-04-19_` on its own line | Use a real heading, or put the date in one |
| **MD013** | 80 columns | A one-line bullet with a PR link and a sentence. Long URLs without spaces are exempt. Official Keep a Changelog 1.1.0 bullets stay under 80; wrapping continuation lines satisfies the rule. | Yes — wrap. Repos often disable it (claude-plugins, oakum rumdl) |
| **MD032** | blanks around lists | List immediately under a heading — the same miss as MD022. linesmith PR #16. | Yes |
| **MD041** | first line is a top-level heading | File starts with an HTML comment or a blank | Start with `# Changelog` |
| **MD047** / **MD009** | trailing newline; no trailing spaces | Forgotten trailing newline or trailing spaces | Yes |

MD022 is the collision that blocked a release. Official Keep a Changelog 1.1.0 already satisfies it: blank line after every heading, including between `## [1.1.2] - 2024-09-27` and `### Added`. MD024 is the one that makes "pass every default rule" and "emit Keep a Changelog" have no intersection. The collision note's cheapest-form claim (default-rule-clean, because configs are subtractive) is true for MD022 and false for MD024. Repos that lint changelogs turn MD024 off (or set `siblings_only`). A generator that repeated `### Added` would fail a stock `markdownlint` run and pass claude-plugins.

MD013 is default-on and fails a long unwrapped bullet. Keep a Changelog does not require long lines: the official 1.1.0 sample stays under 80, and wrapping a PR-link bullet satisfies the rule. Surveyed repos disable it anyway. Long URLs with no spaces are already exempt; wrapping inside `[text](url)` is worse than leaving that line long.

### Bump files are a second lint surface

`.changeset/*.md` bump files are markdown. Feature PRs lint them under a `**/*.md` glob; the version PR deletes them. The note becomes the changelog entry ([specs/bump-files.md](../specs/bump-files.md)). `add --message` is user-authored and the spec keeps it verbatim, so oakum does not wrap it or strip HTML. Mechanical envelope only: trailing newline, and the blank after the closing `---` that the spec example already shows. `generate` writes notes from commit subjects ([specs/generate.md](../specs/generate.md)); that body is oakum-authored, so wrapping those list lines is `generate`'s job.

The file cannot pass default MD041. Line 1 must be exactly `---` ([ADR-0005](../decisions/0005-write-the-changeset-format-intersection.md)). After front matter, the first content line is almost always a paragraph. MD041 applies unless front matter has a `title:` key ([MD041](https://raw.githubusercontent.com/DavidAnson/markdownlint/main/doc/md041.md), 2026-08-25). An extra frontmatter key is fatal in `@changesets/parse` and wrong in knope. A `#` heading in the note would satisfy MD041 and then appear in the changelog.

Measured 2026-08-25 with markdownlint-cli2 0.23.2 and no config: a typical bump file failed only MD041. Adding a blank after the closing `---` did not help. `write()` today concatenates the note immediately after `---\n` (`format.rs`); the spec example already shows a blank there. That blank is envelope; it does not rewrite the note.

`.changeset/README.md` from `init` is ordinary markdown and can start with `#`. GitHub release bodies and version-PR comments are not repo files; the repository linter never sees them.

A linter reads the whole `CHANGELOG.md`. Prepending a clean section onto a file that already has `<sub>` or missing blanks still fails on the old lines. Oakum does not rewrite existing sections.

### What the surveyed tools do

Nobody in the required set reads a markdownlint or rumdl config. One tool runs the project's formatter. The rest emit a shape and leave linting to the repository.

**bumpy (the live collision).** Built-in `"default"` and `"github"` formatters, or a user JS/TS module ([changelog-formatters.md](https://raw.githubusercontent.com/dmno-dev/bumpy/main/docs/changelog-formatters.md), 2026-08-25). No lint pass. The date is a line under the heading, not text in it.

The `<sub>` form is deliberate styling. bumpy's own 1.13.0 entry (commit `eb0f9da`, 2026-06-03) is:

```markdown
## 1.13.0

<sub>2026-06-03</sub>
```

The claude-plugins 1.18.1 fixture is the same date-as-a-line choice, but not the same spacing. Its newest heading is `## 0.15.0` then `<sub>2026-08-19</sub>` with no blank: that is the MD022 failure (`nl -ba` on the fixture, 2026-08-25). bumpy's own 1.13.0 entry, quoted above, already inserts the blank; current docs show `_2026-04-19_` (italic) with a blank. Those two pass MD022 and fail MD033 (`<sub>`, no fix) or MD036 (italic-only line). The date line is a rendering choice. The live CI block was the missing blank under a newly prepended heading. Putting the date in the heading (knope `## 1.2.3 (2023-02-01)`, git-cliff `## [1.2.3] - 2023-02-01`) avoids all three rules.

The extra blank lines are a second, independent defect. MD012 is off in claude-plugins, so CI never saw them.

**changesets — option 3, formatter auto-detect.** `@changesets/cli@3.0.0` replaced `prettier` with `format`: `"auto"` | named formatter | `false` ([release notes](https://github.com/changesets/changesets/releases/tag/@changesets/cli@3.0.0), 2026-08-25; [PR #1994](https://github.com/changesets/changesets/pull/1994)). `"auto"` detects the project's formatter and applies it through [`@changesets/format`](https://github.com/changesets/format); the formatter must be installed. The changelog shape is still changesets' own: `## 3.0.0` / `### Major Changes`, no date, blank lines around headings (their `CHANGELOG.md`, 2026-08-25). Formatting is a post-write pass, not linting. MD033 has no fixer. MD024 has no useful one. Two formatters still disagree: the collision note measured rumdl vs markdownlint `--fix` on one file.

That is option 3 in production. The cost is formatter detection that still does not make a version PR lint-clean.

**knope — emit a spaced shape; fail when the existing file is a shape it cannot parse.** Version heading is `## 1.2.3 (2023-02-01)` ([docs](https://knope.tech/reference/concepts/changelog/), 2026-08-25). Date in the heading. knope's own `CHANGELOG.md` has a blank line after every heading. Default change templates insert `\n\n` between a complex-change heading and its body. No formatter pass. Repeated `### Features` / `### Fixes` — MD024 under defaults.

oakoss/linesmith [PR #16](https://github.com/oakoss/linesmith/pull/16) is a second live collision, knope rather than bumpy. Keep-a-Changelog headers (`## [X.Y.Z] - YYYY-MM-DD`) did not match knope's parser, so insertion fell through to a no-leading-newline append and tripped MD022/MD032 on every release PR. The generator emits clean spacing when it finds the seam, and does not when it cannot. Oakum inherits that class of bug if it splices into an existing file by heading regex.

**release-please — emit; maintainers refuse a formatter.** `Changelog.updateContent` prepends an entry; a new file is `# Changelog\n` plus the entry ([source](https://github.com/googleapis/release-please/blob/main/src/updaters/changelog.ts), 2026-08-25). Tests use `## 2.0.0\n\n* added…` — they get MD022 right in that fixture. Notes come from `conventional-changelog-writer` with section headings `### Features`, `### Bug Fixes`; `<` / `>` in subjects become entities. Same MD024 exposure as knope.

When a generated file failed MD012, the maintainer closed it as not their job ([#2085](https://github.com/googleapis/release-please/issues/2085), 2024-09-11; same text on [#1802](https://github.com/googleapis/release-please/issues/1802) for Prettier): *"Formatting is a personal choice and we cannot feasibly configure formatting for every file type, nor do we plan to run any post-processing binaries. Users are free to run post-processing on the release-please PRs to 'clean up' formatting."* That is options 3, 4, and 5 pushed onto the consumer. Follow-on PRs in other repos excluded `CHANGELOG.md` from lint — option 5 as a workaround, not a product default.

**cargo-dist — reads; does not write.** `parse-changelog` looks for a heading whose version matches the package ([guide](https://axodotdev.github.io/cargo-dist/book/workspaces/simple-guide.html), 2026-08-25). Unreleased headings can be rewritten on prerelease. If a repo uses both oakum and cargo-dist, oakum's version heading has to remain parseable: version in the heading text, not only in a `<sub>` or italic line underneath. That is a reader constraint, not a lint one. It lands on the same date-in-the-heading shape.

**git-cliff — the user owns the bytes.** The keepachangelog template puts the date in the heading (`## [version] - YYYY-MM-DD`) ([`examples/keepachangelog.toml`](https://raw.githubusercontent.com/orhun/git-cliff/main/examples/keepachangelog.toml), 2026-08-25). A `git-cliff` 2.13.1 render of that template (`--unreleased --tag 1.2.3`, 2026-08-25) produced a blank line between `## [1.2.3] - 2026-08-25` and `### Added`, the official Keep a Changelog 1.1.0 spacing. The tool does not lint. Docs show an HTML footer (`<!-- generated by git-cliff -->`) — MD033. `replace_command` on preprocessors and postprocessors is exec ([templating-prior-art.md](templating-prior-art.md)); [ADR-0006](../decisions/0006-no-command-execution-in-templates.md) already used git-cliff's pandoc example as the hermeticity failure. A user who wants prettier can put it in `replace_command` or in their workflow. That is option 3 as a user surface, not as the tool's write path.

### Option 2 and ADR-0004

Lint config is a preference. The repository already states it, in a file oakum can read. Reading `.markdownlint-cli2.yaml` / `.rumdl.toml` / `dprint.json` each run is **derive**, not configure. [ADR-0004](../decisions/0004-derive-facts-configure-preference.md) permits that. A key in `.changeset/_config.toml` that copies those rules would restate a file and rot, which ADR-0004 forbids.

The remaining problems are:

- Conforming means implementing (or shelling out to) markdownlint, rumdl, dprint, prettier, markdownlint-cli2, and whatever the next repo adds. Shelling out is option 3.
- "The" config is underspecified when two tools disagree. This repository runs rumdl and captured a markdownlint `--fix` result that rumdl will not emit. The collision note already called that the more general finding.
- Additive custom rules (MD043 required headings, a 100-column MD013, a plugin rule) still fail after a perfect read of the default-rule subset.

Changelog shape is already on ADR-0004's configured list: headings, date placement, Keep-a-Changelog vs knope. That is output preference. Lint rules are a different preference, already on disk. Oakum should not grow a second copy of them.

### Option 3 and ADR-0003 / ADR-0006

[ADR-0023](../decisions/0023-name-every-verb-and-what-it-owns.md) gives `version` the changelog. Running a formatter on the file oakum wrote is still writing an owned file. [ADR-0003](../decisions/0003-write-only-what-a-command-owns.md) is satisfied.

[ADR-0006](../decisions/0006-no-command-execution-in-templates.md) rejects `{ command = "..." }` in *templates*, because rendering for `check` or `--dry-run` would execute the command, and because a missing binary aborts the release. Running `prettier --write CHANGELOG.md` during `version` is not a template. Those reasons still apply to a `version` formatter pass:

- Hermeticity. A version job on a stock runner might not have prettier, rumdl, or dprint. changesets spent years on this (bundled prettier → user-installed → `@changesets/format` auto-detect) and still requires the formatter to be present.
- `check` is pure. A preview that ran the fixer would apply it. ADR-0006's confirmation already says: if exec is added, it is a **separate named surface**, defaults to off, is enabled only from outside the config file, and **never runs during `check`**.
- Formatters disagree, and a formatter is not a linter. MD033 has no fix. MD024 has no useful one. The collision note's one-line `--fix` is MD022 only.
- release-please will not run post-processing binaries ([#2085](https://github.com/googleapis/release-please/issues/2085)). Same hermeticity argument ADR-0006 already made.

Option 3 can be a later named surface on `version`. It cannot be implicit write-path behavior. The escape hatch already exists: the user's workflow runs the formatter, or notes arrive on stdin / `--notes-file` ([ADR-0006](../decisions/0006-no-command-execution-in-templates.md)). changesets is the peer that built that escape hatch in; release-please left it on the user.

### Options 4 and 5

Option 4 fails on a CI version PR: there is no pre-commit hook. The collision was a CI job named `Versioned release`.

Option 5 (exclude `CHANGELOG.md` from linting) resolves the collision for that repository and ends the question for every tool. It is a repository choice. release-please's issue trail shows consumers doing exactly that after the maintainers refused to format. It is not oakum's answer. [ADR-0031](../decisions/0031-write-generated-markdown-genre-intersection.md) rejects it on the record rather than skipping it.

## Conclusions

**Recommend option 1, refined: write the genre intersection, not the raw default-rule set.**

Emit Keep-a-Changelog-shaped markdown that satisfies every default markdownlint rule a changelog can satisfy. Treat **MD024** (changelogs) and **MD041** (bump files) as genre exceptions. That is the same class as [ADR-0005](../decisions/0005-write-the-changeset-format-intersection.md)'s scoped-name case, where no intersection exists.

Mechanically satisfiable (oakum must emit these):

- Blank line above and below every heading (MD022)
- At most one consecutive blank (MD012)
- Blank lines around lists (MD032)
- No inline HTML (MD033)
- No italic- or bold-only date line (MD036)
- First line `# Changelog` (MD041)
- File ends with a single newline; no trailing spaces (MD047, MD009)
- Date **in the version heading**, not under it: `## 1.2.3 (2026-08-25)` or `## [1.2.3] - 2026-08-25`. Official Keep a Changelog 1.1.0 uses the second form and already has the blank lines. cargo-dist's reader wants the version in that heading. knope does this. A `git-cliff` 2.13.1 keepachangelog render did too, including the blank before `###`. bumpy's `<sub>` / italic line is the shape not to copy.
- Wrap bullets that would exceed 80 columns (MD013). Official Keep a Changelog 1.1.0 already does. A one-line PR-link bullet is a wrapping miss, not a genre exception.

Genre exceptions (do not try to satisfy; do not add oakum config that copies the repo's disable):

- **MD024** on changelogs. Keep a Changelog repeats `### Added`. markdownlint's own rule doc names this. A stock default run fails; every changelog-linting repo in this survey turns the rule off or sets `siblings_only`. A first release with one `### Added` can pass; the second version PR is when the duplicate appears.
- **MD041** on bump files. Line 1 is `---`. There is no `title:` key oakum can add. Same class as MD024: a default rule the genre cannot meet.

If a repository uses a markdown linter, it should disable those two rules for those cases (MD024 on changelogs, MD041 on `.changeset/*.md`). MD024's own docs give the smaller changelog setting: `siblings_only: true`. MD041 has no equivalent that leaves bump files valid under ADR-0005. Oakum keeps emitting the files. `init` does not write `.markdownlint*` or `.rumdl.toml`; it can print the two lines, the same way it prints the workflow it does not write. `.changeset/README.md` and the guide carry the same text.

That answers the collision note's first open question: default-rule-clean is not enough, because MD024 is on by default and Keep a Changelog cannot meet it. MD013 is on by default too, but wrapping satisfies it. The useful default (the rules repositories keep) is MD022 and friends. Emit those. Do not implement a linter.

**Do not read lint config (option 2).** ADR-0004 permits the read and forbids a key that restates the file. Implementing conformance is a linter product, and "the" config is underspecified when two formatters disagree.

**Do not run the repository formatter inside oakum (option 3).** ADR-0003 permits the write. Hermeticity and `check` purity do not permit making it implicit. changesets is the peer that chose this and still does not lint. release-please is the peer that refused. If exec is added, it is ADR-0006's named surface on `version` only.

**Do not rely on hooks (option 4).** CI version PRs have none.

**Do not exclude generated files as oakum's answer (option 5).** A repository may; the tool must still emit something that survives a linter the repository kept.

## Implications / actions

- **[ADR-0031](../decisions/0031-write-generated-markdown-genre-intersection.md)** (2026-08-25) adopts the genre intersection, including the two exceptions and the printed lint guidance. Changelog shape stays a configured preference ([ADR-0004](../decisions/0004-derive-facts-configure-preference.md)); the default of that preference is the heading-and-spacing rules above, not a read of `.markdownlint*`.
- **[okm-luq]** (implement changelogs) should pin the satisfiable rules with fixtures, the way the changeset-format intersection tests do: heading then blank then section then blank then list; no `<sub>`; no extra blanks; date in the heading. Do not run markdownlint as a subprocess in `check`. Do not rewrite existing changelog sections.
- **`add`** keeps `--message` verbatim ([specs/bump-files.md](../specs/bump-files.md)). Mechanical envelope only: trailing newline and a blank after the closing `---`. **`generate`** wraps the oakum-authored list it writes from commit subjects. Frontmatter stays the [ADR-0005](../decisions/0005-write-the-changeset-format-intersection.md) intersection.
- Insertion into an existing file needs a tested seam. knope's linesmith failure is the warning: a heading form oakum does not recognize must not fall through to "glue the new section on with no leading newline."
- cargo-dist compatibility is free if the version is in the heading.

## Open questions

- Heading dialect inside the genre intersection (knope `## 1.2.3 (date)` vs Keep a Changelog `## [1.2.3] - date`) is changelog shape, already configured. This note does not pick one. cargo-dist's `parse-changelog` accepts both so long as the version token is in the heading.
- Whether `version` should refuse to splice when it cannot find a recognized heading, rather than append. Product preference; linesmith says append-without-spacing is the defect.
- Additive custom rules (MD043, plugin rules) — unchecked frequency. Option 1 still fails those; that failure is accepted.
- GoReleaser writes a release body, not a repo file. release-plz is git-cliff plus HTML comments stripped in the template. semantic-release's official changelog plugin has no formatter; users invent `exec` + prettier. None change the recommendation.

## Raw data

| Tool | Writes CHANGELOG? | Date placement | Blank lines around headings | Runs project formatter? | Reads lint config? |
|---|---|---|---|---|---|
| bumpy 1.18.1 | yes | line under heading (`<sub>` in 1.13.0–1.18.1; docs now show italic) | newest fixture entry **no** (MD022); own 1.13.0 and current docs **yes** | no | no |
| changesets 3.0 | yes | none | yes (own file) | **yes** — `format: "auto"` | no |
| knope | yes | in heading `## 1.2.3 (date)` | yes when the seam parses; no on fallback | no | no |
| release-please | yes | in heading (conventional-changelog) | yes in updater tests; MD012 in #2085 | **no** — maintainers refuse | no |
| cargo-dist | no (reads) | n/a | n/a | n/a | n/a |
| git-cliff | yes (template) | in heading in keepachangelog.toml | yes in a 2.13.1 keepachangelog render (2026-08-25) | only if the user puts it in `replace_command` | no |

| Option | ADR-0003 | ADR-0004 | ADR-0006 | Verdict |
|---|---|---|---|---|
| 1. Genre intersection | `version` owns the file | shape is configured preference; no lint-config key | no exec | **Recommend** |
| 2. Read lint config | n/a (read) | read is derive; a copying key would rot; implementing it is a linter | exec if it shells out | Reject for v0 |
| 3. Run project formatter | write is owned | n/a | hermeticity; never during `check`; named surface only | Reject as implicit; defer to ADR-0006 confirmation |
| 4. Pre-commit hook | n/a | n/a | n/a | Reject — no hook in CI version PRs |
| 5. Exclude generated files | n/a | n/a | n/a | Reject as oakum's answer; repo may still do it |
