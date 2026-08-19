# Bump-file tool interfaces: bumpy's CLI surface and propagation model

- Date: 2026-08-18
- Author: Claude Code research agent
- Scope: what bumpy actually exposes and how it propagates versions, as the primary reference for oakum's own interface

## Question

Oakum adopts the changeset file format ([ADR-0005](../decisions/0005-write-the-changeset-format-intersection.md)) but had not settled its own command surface, and several open questions across the specs turned out to be things bumpy already answers. bumpy is the primary reference for interface decisions; changesets is prior art. What does bumpy actually do?

## Sources

- `@varlock/bumpy` 1.18.1 as installed in claude-plugins, plus `dmno-dev/bumpy` `docs/cli.md` and `docs/version-propagation.md`, read 2026-08-18
- `changesets/changesets` `packages/cli/src/cli.ts`, read 2026-08-18

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
| `catalog:` | always satisfied; cannot be resolved for checking |

`^0.x` is handled correctly: `^0.2.3` means `>=0.2.3 <0.3.0`, so a minor bump breaks the range.

### `none` already has semantics

"Acknowledges a change without triggering a direct bump. Unlike a real bump type, `none` doesn't add the package to the release plan on its own. However, cascading bumps from other packages can still bump it normally."

### Per-file `cascade:` is about attribution, not the graph

```yaml
'@myorg/core':
  bump: minor
  cascade:
    '@myorg/plugin-*': patch
```

Glob-capable, and its stated purpose is that "cascaded packages are marked as dependency bumps (not direct changes), which affects how they appear in changelogs and PR comments."

### `version` updates the lockfile

Its documented steps: read bump files and compute the plan, update `package.json` versions, generate changelog entries, delete consumed bump files, **update the lockfile**, optionally commit.

## Conclusions

Most of oakum's undecided interface surface is already answered by the tool it treats as primary, and adopting those answers costs nothing in differentiation — the differentiator is the cascade rules, not the flag names. Someone moving from bumpy to oakum keeping their muscle memory is the explicit goal recorded in [ADR-0012](../decisions/0012-scope-v0-to-version-math-and-the-github-layer.md).

The one deliberate divergence is `workspace:*`. bumpy treats it as always satisfied; pnpm publishes it as an **exact pin**, so oakum cascades on any bump. See [ADR-0010](../decisions/0010-derive-cascade-from-declared-ranges.md).

## Implications / actions

- Define `add` in [bump-files.md](../specs/bump-files.md) as `--packages`/`--message`/`--name`/`--empty`/`--none`; `templates/changeset-readme.md` already ships this and is correct.
- Spec `generate` and `check --hook`; promote or close [idea 0003](../ideas/0003-check-as-a-git-hook.md), whose open questions this answers.
- Rebuild the second job in [github-actions.md](../guide/github-actions.md) around `plan` gating `release`, and adopt the emit-then-post split for fork PRs.
- Adopt bumpy's `none` semantics, closing that open question in bump-files.md and migrate.md.
- `catalog:` needs a position; oakum currently mentions it only in a fixture list.
- Phases B and C are unconsidered scope, not rejected scope.

## Open questions

- Whether oakum's `dependencies` → patch default should follow bumpy's peer-dependency exception, which matches the triggering level for proportionality.
- Whether `catalog:` should be resolved (bumpy resolves catalogs itself) or treated as always satisfied.
- changesets v3 landed August 2026 and fixed forced peer-dep major bumps, unresolved `workspace:` ranges, missing non-interactive mode, and publish ordering. [changeset-file-format.md](changeset-file-format.md) and [registry-publish-semantics.md](registry-publish-semantics.md) were written against earlier behavior and need re-checking against v3.
