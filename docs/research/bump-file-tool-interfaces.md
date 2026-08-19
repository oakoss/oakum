# Bump-file tool interfaces: bumpy's CLI surface and propagation model

- Date: 2026-08-18, revised 2026-08-19
- Author: Jace Babin
- Scope: what bumpy actually exposes and how it propagates versions, as the primary reference for oakum's own interface

## Question

Oakum adopts the changeset file format ([ADR-0005](../decisions/0005-write-the-changeset-format-intersection.md)) but had not settled its own command surface, and several open questions across the specs turned out to be things bumpy already answers. bumpy is the primary reference for interface decisions; changesets is prior art. What does bumpy actually do?

## Sources

- `@varlock/bumpy` 1.18.1 as installed in claude-plugins, plus the full `dmno-dev/bumpy` `docs/` set — `bump-files`, `changelog-formatters`, `cli`, `comparisons`, `configuration`, `differences-from-changesets`, `github-actions`, `prereleases`, `snapshots`, `version-propagation` — read 2026-08-18
- `changesets/changesets` `packages/cli/src/cli.ts`, read 2026-08-18
- `knope-dev/changesets` `src/versioning.rs`, read 2026-08-19, for how an unrecognized bump level is handled

## Findings

### `add` is fully non-interactive

```bash
bumpy add --packages "core:minor,utils:patch" --message "Added features"
bumpy add --empty --name "docs-only-pr"
bumpy add --none
```

| Flag | Meaning |
|---|---|
| `--packages <list>` | comma-separated `name:level` pairs |
| `--message <text>` | changelog description |
| `--name <name>` | bump file filename, auto-slugified |
| `--empty` | a bump file marking a PR as intentionally releaseless |
| `--none` | set all changed packages to `none` |

The top-level `bumpy --help` lists only `--empty` and `--none` for `add`; the full set is in `docs/cli.md`. `bumpy add --help` launches the interactive TUI rather than printing help — do not copy that.

changesets reached the same capability later and chose a different shape: `--major <pkg>`, `--minor <pkg>`, `--patch <pkg>` with the level as the flag name, accepting both comma-separated and repeated forms. Its v3 release (August 2026) is what added a non-interactive mode at all.

### `generate` derives bump files from commits

`--from <ref>` (default: branch point from `baseBranch`), `--dry-run`, `--name <name>`.

Conventional commits map type to level — `feat:` minor; `fix|perf|refactor|docs|style|test|build|ci|chore:` patch; `feat!:` or `BREAKING CHANGE:` major — with the scope resolving the package name. Every other commit style maps to packages by changed file path and defaults to `patch`.

### `check` distinguishes hook contexts by what counts

Default behavior fails only when **no** bump files exist at all. `--strict` requires every changed package to be covered.

| Flag | Meaning |
|---|---|
| `--hook pre-commit` | counts staged **and** committed bump files |
| `--hook pre-push` | counts committed bump files only |
| `--strict` | every changed package must be covered |
| `--no-fail` | warn only, for advisory hooks |
| `--base <branch>` | branch to compare against |

The staged-versus-committed distinction is the substantive difference between the two hook points, not verbosity or exit code.

### CI is three commands, split by privilege

- `ci check` — computes the plan from the PR's bump files and posts a comment.
- `ci plan` — "detects what should happen next (`version-pr`, `publish`, or nothing) **without needing write permissions or publish credentials**. Used to gate downstream jobs in split-job workflows."
- `ci release` — opens or updates the version PR, or publishes and tags when it merges.

Two fork-safety mechanisms come with it. `ci check --emit-comment <dir>` renders the comment in the untrusted job so a trusted downstream job can post it. The global `--cwd` exists "to point bumpy at an untrusted checkout (e.g. a fork PR) while bumpy itself is fetched from a trusted directory."

### Propagation is three phases

**Phase A — out-of-range, mandatory, cannot be skipped.** For each dependent whose declared range would no longer admit the new version:

| Dependency type | Dependent gets | bumpy's reason |
|---|---|---|
| `peerDependencies` | matches the triggering bump | proportional; matters for `0.x` where `^` breaks often |
| `dependencies` | `patch` | internal detail, consumers do not see it |
| `optionalDependencies` | `patch` | same |
| `devDependencies` | skipped | does not affect published consumers |

The bundled-devDependency exception is handled by `releaseTriggeringDevDeps` or `cascadeFrom`, not by cascading all dev edges.

**Phase B** — fixed and linked groups. **Phase C** — proactive propagation via `updateInternalDependencies`, valued `out-of-range` (default), `patch`, or `minor`, releasing dependents whose ranges are still satisfied.

### Protocol resolution, and where oakum diverges

| Declared | bumpy resolves to |
|---|---|
| `workspace:^` | `^<currentVersion>` |
| `workspace:~` | `~<currentVersion>` |
| `workspace:*` | **always satisfied — never triggers propagation** |
| `catalog:` | always satisfied — `satisfies()` short-circuits on the protocol, despite bumpy shipping a working catalog resolver |

`^0.x` is handled correctly: `^0.2.3` means `>=0.2.3 <0.3.0`, so a minor bump breaks the range.

### `none` already has semantics

"Acknowledges a change without triggering a direct bump — useful for covering packages in `--strict` mode. Cascading bumps from other packages can still apply." (`dmno-dev/bumpy` `docs/bump-files.md`, read 2026-08-19.)

The `--strict`-mode clause is the point: `none` exists so a package can satisfy the coverage gate without producing a release.

### Per-file `cascade:` is about attribution, not the graph

```yaml
'@myorg/core':
  bump: minor
  cascade:
    '@myorg/plugin-*': patch
```

Glob-capable, and its stated purpose is that "cascaded packages are marked as dependency bumps (not direct changes), which affects how they appear in changelogs and PR comments."

### Non-manifest edges are declared once, in per-package config

`cascadeTo` ("when I am bumped, cascade to these") and `cascadeFrom` ("when these are bumped, cascade to me") live in a package's own config, take globs, take `{ trigger, bumpAs }`, and apply regardless of `updateInternalDependencies`. `cascadeFrom` is the documented answer for a **bundled** devDependency — code that tsup or Vite inlines into the published artifact, where the dev edge really does reach consumers — alongside `releaseTriggeringDevDeps`.

This is the mechanism [ADR-0010](../decisions/0010-derive-cascade-from-declared-ranges.md) prefers over the per-file `cascade:` block, and bumpy ships both. The disagreement is which one is the default, not whether declared-once edges should exist.

### Cascade is configured on two axes

Each dependency type carries a `trigger` (how large a bump sets off a cascade) and a `bumpAs` (how large a bump the dependent gets). Defaults: `dependencies` `patch`/`patch`, `peerDependencies` `major`/`match`, `optionalDependencies` `minor`/`patch`, `devDependencies` disabled. Both are overridable globally and per package.

oakum derives the trigger from the declared range instead of configuring it, so only `bumpAs` is a live question for it.

### The config surface, in full

Keys not covered elsewhere in this document, several of which oakum has no position on:

| Key | Default | What it decides |
|---|---|---|
| `changedFilePatterns` | `["**"]` | which file changes make a package "changed" for coverage |
| `ignoredPackageJsonFields` | `["devDependencies"]` | manifest fields whose sole change needs no bump file |
| `fixed` / `linked` | `[]` | groups versioned together, or sharing the highest level |
| `ignore` / `include` | `[]` | glob-based package selection; `include` overrides the rest |
| `privatePackages` | `{ version: false, tag: false }` | whether `private: true` packages get versions and tags |
| `allowCustomCommands` | `false` | whether per-package publish commands in a manifest are honored |
| `managed` (per package) | — | opt a single package in or out of version management |
| `skipNpmPublish`, `checkPublished` (per package) | — | tag without publishing; supply a command reporting the live version |
| `protocolResolution` | `"pack"` | resolve `workspace:`/`catalog:` by packing, or rewrite in place |
| `versionCommitMessage`, `versionPr.{title,branch,preamble}`, `gitUser` | — | the customization surface, as string or module path |

`privatePackages`, `skipNpmPublish`, and `checkPublished` are the private-and-unpublished path — which [ADR-0012](../decisions/0012-scope-v0-to-version-math-and-the-github-layer.md) makes oakum's most-exercised path, not an edge case. `allowCustomCommands` defaulting to `false` is a security posture worth copying: a command read out of a manifest is code from the repository, and opt-in is the right default for it.

### Changelog formatters get a context with a target discriminator

A formatter receives `release`, `bumpFiles`, `date` (ISO `YYYY-MM-DD`), and `target` — either `changelog` or `github-release` — so one formatter can drop the date from a release body that already shows one. Built-ins are `default` and `github`; the latter shells out to `gh` for pull-request links and contributor thanks, with `internalAuthors` excluded.

The `target` discriminator generalizes: it is the same problem [ADR-0015](../decisions/0015-layer-the-pr-status-channels.md) has rendering one plan to both a comment and a job summary.

### Snapshots are a separate concept from channels

A snapshot is a throwaway publish of one commit — `1.4.0-pr-123-a1b2c3d` under `versionStrategy: "sha"`, which is idempotent per commit, or a timestamp form that never is. `publish --snapshot <name>` and `ci release --snapshot <name>`; the name serves as both preid and dist-tag unless `--tag` decouples them.

bumpy's own docs route public-package previews to pkg.pr.new and keep snapshots for private registries. That is three clean seams — channels, snapshots, previews — and oakum's non-goals should name which of the three they exclude rather than saying "prereleases."

### Maintenance branches are unimplemented there too

bumpy's comparison docs list "maintenance/release branch workflows: hotfix support for older versions" as planned and not built, alongside root-workspace change tracking and non-JS ecosystem support. That corroborates the survey in [idea 0007](../ideas/0007-maintenance-release-branches.md): no surveyed tool serves the release-train workflow.

### `version` updates the lockfile

Its documented steps: read bump files and compute the plan, update `package.json` versions, generate changelog entries, delete consumed bump files, **update the lockfile**, optionally commit.

## Conclusions

Most of oakum's undecided interface surface is already answered by the tool it treats as primary, and adopting those answers costs nothing in differentiation — the differentiator is the cascade rules, not the flag names. Someone moving from bumpy to oakum keeping their muscle memory is the explicit goal recorded in [ADR-0012](../decisions/0012-scope-v0-to-version-math-and-the-github-layer.md).

The one deliberate divergence is `workspace:*`. bumpy treats it as always satisfied; pnpm publishes it as an **exact pin**, so oakum cascades on any bump. See [ADR-0010](../decisions/0010-derive-cascade-from-declared-ranges.md).

## Implications / actions

- Define `add` in [bump-files.md](../specs/bump-files.md) as `--packages`/`--message`/`--name`/`--empty`/`--none`; `templates/changeset-readme.md` already ships this and is correct.
- Spec `generate` and `check --hook`; promote or close [idea 0003](../ideas/0003-check-as-a-git-hook.md), whose open questions this answers.
- Rebuild the second job in [github-actions.md](../guide/github-actions.md) around `plan` gating `release`, and adopt the emit-then-post split for fork PRs.
- Adopt bumpy's `none` **semantics** — acknowledge a change, take no direct bump, still accept a cascade — but not the literal level. [ADR-0005](../decisions/0005-write-the-changeset-format-intersection.md) rules that out: the parser knope uses maps an unrecognized level to `Custom` rather than rejecting it — `knope-dev/changesets` `src/versioning.rs:140`, `impl From<&str> for ChangeType`, whose final arm is `other => ChangeType::Custom(other.to_string())` — so `none` is silently reinterpreted (read 2026-08-19). This does **not** close the open question in [bump-files.md](../specs/bump-files.md), which asks for the shape of the non-`.md` marker that would carry those semantics.
- Take a position on `changedFilePatterns` and `ignoredPackageJsonFields`: they decide what "changed" means, and the coverage gate is built on that word.
- Phase B is unconsidered scope. Phase C now has a stated non-position in [ADR-0010](../decisions/0010-derive-cascade-from-declared-ranges.md).

## Open questions

- Whether oakum's `dependencies` → patch default should follow bumpy's peer-dependency exception, which matches the triggering level for proportionality.
- How `catalog:` gets resolved. [ADR-0010](../decisions/0010-derive-cascade-from-declared-ranges.md) rules out treating it as always satisfied; bumpy resolves pnpm, Bun, and Yarn catalogs itself, and whether oakum reads the catalog file directly or asks the package manager is undecided.
- Whether `version` shelling out to `pnpm install --lockfile-only` is the sanctioned exception to the read-only-discovery rule, or whether the lockfile is edited directly. `AGENTS.md` says `version` owns the lockfile entries its bumps invalidate but not by what means.
- changesets v3 landed August 2026 and fixed forced peer-dep major bumps, unresolved `workspace:` ranges, missing non-interactive mode, and publish ordering. [changeset-file-format.md](changeset-file-format.md) and [registry-publish-semantics.md](registry-publish-semantics.md) were written against earlier behavior and need re-checking against v3.
