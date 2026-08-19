# Changeset file format: what the JS and knope parsers each tolerate

- Date: 2026-08-18
- Author: Jace Babin
- Scope: Which parts of the `.changeset/*.md` format are safe to write when both `@changesets/cli` and knope may read the same directory.

## Question

Oakum adopts the changesets file format so migration costs nothing. Two other tools read those files in repositories we care about — `@changesets/cli` (tsc-files, tt-packages-demo) and knope (linesmith). What can oakum write that both parsers read identically, and which format extensions are unavailable?

## Sources

- `@changesets/parse@1.0.0` and `@0.4.3`, `@changesets/read@1.0.0` and `@0.6.7`, `@changesets/write` — read from installed packages, exercised through `@changesets/cli@3.0.0` and `@2.31.1`
- `changesets` Rust crate `0.4.0` (github.com/knope-dev/changesets) — `src/change.rs`, `src/versioning.rs`, `src/changeset.rs`. Crate source confirmed byte-identical to what `knope 0.22.4` links.
- Both exercised against crafted files in scratch repositories.

## Findings

### The two parsers are not the same implementation

`@changesets/parse` finds frontmatter with an unanchored regex and hands group 1 to a real YAML parser. The knope crate is a hand-rolled line parser with zero runtime dependencies: it requires `---` on line 1 exactly, splits each subsequent line on the first `:`, and stops at the next `---`.

That difference produces divergent behavior on nearly every edge case.

### Case matrix

| Input | `@changesets/cli` 3.x | knope 0.22.4 |
|---|---|---|
| `pkg: minor` (unquoted key) | accepted | accepted |
| `"pkg": minor` (quoted key) | accepted — **this is what it writes** | **silent no-op**, exit 0, no output |
| `none` as bump type | valid; consumes file, bumps nothing | `Custom("none")` → **patch bump, summary discarded** |
| `major` / `minor` / `patch` | accepted | accepted |
| `Major` (wrong case) | error | `Custom("Major")` → patch bump, summary discarded |
| unrecognized value (`bogus`) | error naming valid types | patch bump, summary discarded |
| unknown key (`$meta: minor`) | **fatal** — "not in the workspace" | silently ignored; file still deleted |
| object value (`pkg: {bump: minor}`) | **fatal** — expected string | parsed as a `Custom` string; shape decides outcome |
| empty frontmatter (`---\n---`) | accepted, zero releases | **fatal** — "Versioning needs at least one item" |
| blank line inside frontmatter | accepted (YAML) | **fatal** |
| duplicate key | fatal (YAML duplicate) | **fatal** — off-by-N on the closing delimiter |
| preamble before opening `---` | silently discarded | **fatal** — "missing front matter" |
| CRLF line endings | accepted | accepted |
| UTF-8 BOM | accepted | **fatal** |
| `README.md` in `.changeset/` | skipped by name | **fatal — kills the entire run** |
| subdirectory | `pre/` read specially; others skipped (v3) or fatal (v2) | not discovered, not recursed |
| non-`.md` file | skipped | skipped (extension check is case-sensitive) |
| unparseable file | **hard-errors the whole run**, does not name the file | **hard-errors the whole run**, does not name the file |

### Consequences that are not obvious from the table

**`@changesets/cli` writes quoted keys.** `writeChangeset` emits `"${name}": ${type}`. knope's `parts.0.trim()` does not strip quotes, so the package name retains them, matches nothing, and the file is a complete no-op with exit 0 and no diagnostic. Every file the JS tool writes is invisible to knope.

**`.changeset/README.md` breaks knope.** `@changesets/cli init` creates one. knope's discovery filters on extension only, with no skip list, and the first parse failure aborts everything: `× missing front matter`, exit 1.

**Neither parser names the offending file.** `parseChangesetFile` receives no filename; knope's `LoadingError` carries no path. knope's `read_dir` order is OS-dependent, so with several bad files, *which* one aborts is not deterministic either.

**Extra frontmatter keys are not a metadata channel in either direction.** Fatal in JS; in knope, silently dropped *and* the file is deleted by whichever real package consumes it.

**Scoped npm names have no form both parsers accept.** `@` is a YAML reserved indicator, so an unquoted `@scope/pkg` key is a parse error — verified against `yaml` 2.x, the parser `@changesets/parse` uses: `Plain value cannot start with reserved character @`. Quoting fixes YAML and breaks knope, which retains the quote characters in the package name and skips the file silently. The intersection is therefore empty for scoped packages, and no wording can close it.

### The documented differences are incomplete

The knope `changesets` README lists three differences: Rust vs JavaScript, custom change types instead of `none`, and "change" vs "changeset" naming. Everything else in the table above is undocumented.

## Conclusions

The safe intersection is narrow and worth writing down as a rule:

> First line exactly `---`. One `name: patch|minor|major` per line, unquoted key, no blank lines, no duplicate keys. Closing `---`. No preamble, no BOM. LF or CRLF.

Files written that way are read identically by both parsers. Nothing outside it is safe.

With one exception that cannot be written around: a **scoped npm package** needs quoting to be valid YAML and needs no quoting to be visible to knope. The intersection covers unscoped names only. That is survivable because the two parsers only meet in a repository migrating from knope, and knope's packages are crates, whose names are never scoped — but it is a real limit rather than an oversight, and it belongs in the spec.

Two format extensions considered and rejected on this evidence: `none` as a bump type (silently becomes a patch release with a deleted summary under knope) and empty bump files (fatal under knope).

## Implications / actions

- Oakum writes only the intersection above. See [ADR-0005](../decisions/0005-write-the-changeset-format-intersection.md).
- Tool configuration goes in a non-`.md` file inside `.changeset/`. Both parsers skip anything without a `.md` extension, which makes `_config.toml` invisible to them.
- A "this change ships no release" marker also has to live in a non-`.md` file, since neither `none` nor an empty file survives.
- Migration precondition: detect `.changeset/README.md` and warn. In a knope repository it is already breaking releases.
- Naming the offending file on a parse error, and continuing past it, is a strict improvement over both tools. It costs nothing and it is the difference between a fix and a manual bisect.

## Open questions

- Whether knope's duplicate-key failure is a deliberate constraint or an off-by-N defect worth reporting upstream. The closing-delimiter check advances by the *deduplicated* entry count, which lands short of the delimiter whenever a key repeats.
