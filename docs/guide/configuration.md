<!-- Generated from `config::schema()`. Edit the schema, then run `cargo test -p oakum --test it -- --ignored write_the_configuration_reference`. -->

# Configuration

`.changeset/_config.toml` configures oakum. Every top-level and per-package key is optional, and an omitted key takes its default. `oakum init` writes the file, and `init` and `upgrade` keep `_schema.json` beside it, which editors read for the same descriptions.

## Top-level keys

Refused together: `change-files = false` with `conventional-commits = false`.

### `change-files`

When true, the plan reads `.changeset/*.md`.

- Type: boolean
- Default: `true`

### `commit-message`

Commit message for the version commit. Receives the `oakum status --json` document: `awaiting_tag`, `changed`, `coverage`, `hand_set`, `manages_nothing`, `named_unmanaged`, `packages`, `schema_version`, `selection_empty`, `target`, `uncovered`, `unmanaged`, `unresolved_scopes`. A string is inline; `{ file = "path" }` loads a repository-relative file. Templates render; they do not execute (ADR-0006).

- Type: a string, or `{ file = "path" }` to read one from a file

### `conventional-commits`

When true, commits can feed `generate`, and the plan if change-files is off (ADR-0029).

- Type: boolean
- Default: `true`

### `exclude`

Exact package names to leave alone after include filtering (ADR-0027).

- Type: array of strings
- Default: `[]`

### `include`

Exact package names to manage. Empty means all packages; then exclude removes (ADR-0027).

- Type: array of strings
- Default: `[]`

### `packages`

Per-package overrides, keyed by the name the manifest declares.

- Type: table

### `pr-status`

Pull-request presentation. `none` silences comment and summary; the exit-code gate is not configurable (ADR-0015).

- Type: one of `"comment"`, `"summary"`, `"both"`, `"none"`
- Default: `"both"`

### `private-packages`

Opt-in to version and/or tag packages that are not registry-publishable. Axes are independent (ADR-0027).

- Type: table
- Default: `{ tag = false, version = false }`
- Keys:
  - `tag`: boolean, default `false`. When true, unpublishable packages still receive git tags.
  - `version`: boolean, default `false`. When true, unpublishable packages still receive version bumps and changelogs.

### `tag-format`

Tag oakum writes. Receives `package` and `version`. A string is inline; `{ file = "path" }` loads a repository-relative file. Existing tags are derived, not configured (ADR-0004).

- Type: a string, or `{ file = "path" }` to read one from a file

### `template`

Changelog template, rendered once per package section. Receives `bump`, `changes`, `date`, `ecosystem`, `notes`, `package`, `repo`, `source`, `target`, `tool_version`, `trigger`, `version`. A string is inline; `{ file = "path" }` loads a repository-relative file. Templates render; they do not execute (ADR-0006).

- Type: a string, or `{ file = "path" }` to read one from a file

### `title`

Title for the version pull request. Defaults to the version commit's built-in message; a configured `commit-message` does not change it. Where a repository takes its squash subject from the title, that default keeps the subject conventional. Receives the `oakum status --json` document: `awaiting_tag`, `changed`, `coverage`, `hand_set`, `manages_nothing`, `named_unmanaged`, `packages`, `schema_version`, `selection_empty`, `target`, `uncovered`, `unmanaged`, `unresolved_scopes`. A string is inline; `{ file = "path" }` loads a repository-relative file. One template per surface, with conditionals in the body (ADR-0015).

- Type: a string, or `{ file = "path" }` to read one from a file

### `tool-version`

Deprecated: oakum no longer reads this key, and `oakum upgrade` removes it. The install pin is the version (ADR-0038).

- Type: any

### `versioning`

How a breaking change file moves a version. `zero-major` (default) maps a major file to a minor bump while the package is below 1.0.0, matching SemVer 2.0 §4. `semver` is how a project releases 1.0.0: the next major file produces 1.0.0, and the key is then inert for that package. Override per package under `[packages.<name>]` so one crate can graduate without taking the rest of the workspace with it.

- Type: one of `"zero-major"`, `"semver"`
- Default: `"zero-major"`

## Per-package keys

Under `[packages.<name>]`, keyed by the name the manifest declares.

### `extra-files`

Declared version writes outside the package manifest (ADR-0033).

- Type: array of tables
- Keys:
  - `format`: `"json"`, required. Document format. v1 ships json only.
  - `key`: string, required. Dotted key path. `{field=value}` selects a unique array element (ADR-0033).
  - `path`: string, required. Leading `/` is repository-root relative; otherwise relative to the package manifest directory.

### `publish-command`

Stored publish command for a post-v0 registry slot. Never executed in v0 (ADR-0011 / ADR-0012).

- Type: string

### `resolves-dependencies-at`

Declare that this library bundles dependencies into the published artifact. Binaries are derived; `install` is not configurable (ADR-0009).

- Type: `"build"`

### `versioning`

How a breaking change file moves a version. `zero-major` (default) maps a major file to a minor bump while the package is below 1.0.0, matching SemVer 2.0 §4. `semver` is how a project releases 1.0.0: the next major file produces 1.0.0, and the key is then inert for that package. Override per package under `[packages.<name>]` so one crate can graduate without taking the rest of the workspace with it.

- Type: one of `"zero-major"`, `"semver"`
