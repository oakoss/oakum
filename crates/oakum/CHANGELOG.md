# Changelog

## 0.2.0 (2026-09-10)

### Added

`check` recognizes the npm channel: an exact `@oakoss/oakum` in `package.json`, a versioned `npm i` / `npm install` / `npm add` / `pnpm add` / `pnpm install` / `pnpm dlx` / `npx @oakoss/oakum@x` workflow line, and an `npm:@oakoss/oakum` mise pin count as install pins; a bare `@oakoss/oakum` is `unverified` as unversioned.

`plan::migrate_compare` compares two plan fingerprints, with typed empty discovery and migration resolve and load helpers beside it, so the migration proof can be exercised without going through the CLI. `migrate` keeps the I/O, the rewrites, and the banners, and no longer classifies knope fallout by matching a `Display` string.

`migrate` proves the migration against the source tool's own before-plan whenever one can be run (`bumpy status --json`, a local or `PATH` `changeset status --output` with no silent `npx`, a knope dry run), rather than comparing oakum against oakum with knope's feature remapped. When no source tool can be run it falls back to an oakum simulation for transform safety and exits `unverified` even where the two agree, so a missing binary no longer reads as agreement. An unexpected difference in the fingerprints is still a hard failure.

A bump-file note whose first line is a Keep a Changelog heading (`### Added`, `### Changed`, `### Deprecated`, `### Removed`, `### Fixed`, `### Security`) lands under that section, with the line dropped; the level still picks the section otherwise, and `add --section` writes the line. The changelog template sees `changes` (per note: section, file, and the adding commit's sha, pull request number, and author) and `repo` (the GitHub slug), read from git only when the template mentions them.

The workflow printed by `init` and `migrate` installs oakum with `npm i -g @oakoss/oakum@<tool-version>` in an npm workspace, since `cargo-binstall` is not on `ubuntu-latest` and npm is. `status --from` help names `check`, and the bundled README explains quoting and releaseless files without assuming knope.

### Changed

`check` and `release` exit `unverified` when `.changeset/_config.toml` is absent, naming `oakum init` and `oakum migrate`; `status` keeps defaults and says so. A malformed bump file fails `check`, `status`, `version`, and every other reader by name with its parse error instead of being skipped. `migrate` without a terminal requires `--yes` and otherwise prints the plan and refuses. The workflow printed by `init` and `migrate` runs `oakum check --strict`.

`check` names every `.<name>.oakum-write.*` staging file an unfinished oakum write left in `.changeset/`, at the repository root, beside a package manifest, or beside a declared extra file, and exits `unverified`; `init` and `migrate` name one in `.changeset/` on every run, the already-initialized and already-migrated paths included. Nothing removes it, since a run still in progress could own it.

`check` reports a `CHANGELOG.md` that `version` would refuse to append to (a changesets package-name title, or a UTF-8 BOM) as `unverified`, naming the fix, and `migrate` lists the same file under its remaining steps. Neither rewrites the file.

`add`, `generate`, `version`, and `ci version-pr` exit `unverified` when `.changeset/_config.toml` is absent, naming `oakum init` and `oakum migrate`, instead of writing on defaults.

### Fixed

The bundled `.changeset/README.md` is byte-for-byte what Prettier 3 prints (padded tables), so a repository that formats markdown keeps it recognizable as oakum's own; a real Prettier run over it and over `_schema.json` gates both.

`check` finds the tool-version pin in local composite actions, matrix cells, `with.*` inputs other than `tool`, `with.tool` arrays, and `workflow_call` input defaults. It previously read only workflow `run`, `tool`, and `with.tool` strings, so a pin drifting at any of those sites still verified `ok`. Invalid composite YAML and a non-object `with` fail closed; a step whose command comes only from `${{ matrix.* }}` is documented as unsupported.

A migration bump file identified by its path rather than an id is accepted, and `migrate` builds its after-plan through the same `load_migration_bump_files` seam the before-plan uses, refusing outright when any bump file is malformed.

`migrate` says what it left alone (`config.json` keys, an existing `README.md`), lists publishing and the version-PR branch among the remaining steps, says which tool planned each side of the plan comparison, reports `replaced` when it overwrites a stale `_schema.json`, checks its owned files before the prompt so nothing is rewritten and then refused, and restores missing owned files on a rerun after confirmation. Bump files accept single-quoted scoped package names, so a Prettier `singleQuote` rewrite no longer hides a release. The printed uninstall line names only the files oakum wrote.

Print repo-relative paths with forward slashes on Windows across version, write, inherited, changelog, and intent errors.

`init` and `migrate` say `unchanged` for a `_schema.json` that already holds the bundled schema, where they said `replaced`, and `migrate`'s pending line says it will leave such a file alone. After the prompt `migrate` looks at the owned files again and reports under `changed while waiting:` when a README appeared or stopped being oakum's in the meantime.

`release` fills the GitHub release body with the package's changelog section for that version, read at the tagged commit. When there is none, the body is the title and `release` says so on stderr; a changelog that cannot be read at that commit stops the run before any tag is written. `check`'s tag-drift line says it compared local tags and suggests `git fetch --tags`.

`add` writes the message under a blank line after the frontmatter, with a trailing newline (ADR-0031's envelope), and `generate --dry-run` previews the same shape; `init` emits `_schema.json` with short arrays on one line and pads the bundled README's table, so a repository's Prettier leaves freshly generated files alone. The printed workflow provisions pnpm in every job of an npm workspace and skips `check` on the version PR.

Windows containment treats a loopback admin UNC path as the same volume as the drive letter, so a localhost C$ prefix cannot smuggle a path past the repository check.

## 0.1.4 (2026-09-02)

### Fixed

The bump-file README that oakum init and migrate write documents all add flags, --empty/--none, oakum status for the release plan, and that check is unverified until an install pin exists. oakum check --explain is not a CLI flag. The README compiles into the oakum crate so a published package includes it.

Windows builds compile again: same_identity converts cap-std mtime with into_std before comparing to std.

## 0.1.3 (2026-09-02)

### Fixed

Commands now refuse if the repository root is replaced while they run, instead of reading one tree and writing another.

## 0.1.2 (2026-09-01)

### Fixed

Extract a harden-checkout composite for hand-owned workflows (harden runner and checkout). Mise setup stays in ./.github/actions/setup at each call site.

Release workflow checks out the homebrew tap in a subdirectory so the composite App-token action remains on disk through job cleanup.

## 0.1.1 (2026-09-01)

### Fixed

Self-host release CI uploads cargo-dist artifacts into the GitHub Release oakum already created, instead of calling create again.

Generated by oakum 0.1.4.
