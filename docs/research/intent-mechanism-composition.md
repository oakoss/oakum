# How change files and conventional commits compose

- Date: 2026-08-21
- Author: Jace Babin (agent-assisted)
- Scope: how peers compose bump/change files with commit-derived intent; evidence for [ADR-0029](../decisions/0029-plan-from-one-intent-artifact.md) (closed `okm-1w2`)

## Question

When a tool supports both change/bump files and conventional commits (or commit-derived intent), how do they compose — especially when a commit maps to a package a change file already covers? What should oakum do, given [ADR-0019](../decisions/0019-both-change-files-and-commits-each-disableable.md) left composition open?

*(Answered 2026-08-21 by [ADR-0029](../decisions/0029-plan-from-one-intent-artifact.md): single artifact.)*

## Sources

- `@varlock/bumpy` 1.18.1 published tarball (`npm pack @varlock/bumpy@1.18.1`), `package/dist/generate-ClTJ7X7I.mjs`, read 2026-08-21
- `dmno-dev/bumpy` docs: `docs/cli.md`, `docs/bump-files.md`, `docs/differences-from-changesets.md` (raw GitHub, 2026-08-21)
- `knope.tech` docs: [Changes](https://knope.tech/reference/config-file/changes/), [ChangeSet](https://knope.tech/reference/concepts/changeset/), [PrepareRelease](https://knope.tech/reference/config-file/steps/prepare-release/) (fetched 2026-08-21)
- `knope-dev/knope` source (main, 2026-08-21): `crates/knope/src/step/releases/package.rs` (`get_changes` / `ignore_conventional_commits`), `crates/knope-versioning/src/package.rs` (`get_changes`), `crates/knope-versioning/src/semver/rule.rs` (`Stable::from` → `.max()`)
- `changesets/changesets` issue [#862](https://github.com/changesets/changesets/issues/862) — state `open`, 21 comments (queried via `gh api`, 2026-08-21); intro docs still describe a files-only plan loop
- `changesets/changesets` PR [#1720](https://github.com/changesets/changesets/pull/1720) ("Add auto mode based on semver commits") — state `open` / unmerged (queried via `gh api`, 2026-08-21); would be a generate-style bridge, not dual plan inputs
- [ADR-0019](../decisions/0019-both-change-files-and-commits-each-disableable.md), [ADR-0023](../decisions/0023-name-every-verb-and-what-it-owns.md), [ADR-0029](../decisions/0029-plan-from-one-intent-artifact.md), [specs/bump-files.md](../specs/bump-files.md) — oakum framing and the decision this research informed

## Findings

### Two architectures exist in the wild; oakum already named both

**Files-as-the-only-plan-input, with commits as a bridge.** bumpy's `status` / `version` / `ci *` read pending bump files. Commits enter the system only through `bumpy generate`, which writes a bump file a human can edit. Docs call this "not a replacement for explicit bump files — a bridge" (`differences-from-changesets.md`). `bumpy check` compares branch file changes to bump-file coverage; it does not treat conventional commits as coverage.

**Parallel plan inputs, highest-wins.** knope's `PrepareRelease` parses conventional commits since the last tag *and* reads `.changeset/`, then "combines" them for version and changelog ([ChangeSet](https://knope.tech/reference/concepts/changeset/), [PrepareRelease](https://knope.tech/reference/config-file/steps/prepare-release/)). Default is both on. `[changes] ignore_conventional_commits = true` drops the commit leg; there is no symmetric config to ignore change files when the directory exists (the asymmetry ADR-0019 already recorded).

No surveyed tool implements a third shape: **detect disagreement and fail** when a commit and a change file cover the same package at different levels.

### bumpy: generate never feeds the plan; generate does not consult existing files

From `docs/cli.md` (2026-08-21): `bumpy version` step 1 is "Reads bump files and computes the release plan"; `bumpy generate` is a separate command that "Auto-create[s] bump files from commits."

From the published `generateCommand` in `@varlock/bumpy` 1.18.1:

1. Scans branch commits (or `--from`).
2. Builds an in-memory map, merging commits for the same package with **highest bump level wins** (`mergeRelease` / `bumpPriority`).
3. Writes **one new** `.bumpy/<name>.md`.

It does **not** read existing bump files, skip packages already covered, or refuse to run when pending files exist. Overlap with hand-written files is therefore a *files-vs-files* problem after generate — the same highest-wins aggregation every bump-file tool already does across multiple `.md` files — not a commits-vs-files problem at plan time.

### knope: commits and change files are one `Change` list; bump level is `.max()`

`Package::get_changes` chains commit-derived changes with changeset-derived changes (`knope-versioning` `package.rs`). `Stable::from(changes)` maps each `Change` to Major/Minor/Patch and takes `.max()` (`semver/rule.rs`). Changelog content keeps entries from both sources; only the *version component* collapses.

That is merge-by-default, not "files win" or "commits win." A `feat:` commit plus a `patch` change file for the same package yields a minor (or higher if anything else is major). There is no conflict diagnostic.

Opting out of commits is explicit; opting out of files is "don't have a `.changeset/` directory" — which is why ADR-0019 insisted oakum's disables be symmetric.

### changesets / release-please / semantic-release: single mechanism each

- **changesets** — plan from `.changeset/*.md` only. Conventional-commit integration remains [issue #862](https://github.com/changesets/changesets/issues/862) (`open`, 21 comments as of 2026-08-21). The unfinished PR [#1720](https://github.com/changesets/changesets/pull/1720) (`add` auto mode from semver commits, still `open`) is a generate-style bridge, not parallel plan inputs. bumpy cites #862 as the motivation for `generate`.
- **release-please** / **semantic-release** — treated as commit-driven peers in [ADR-0019](../decisions/0019-both-change-files-and-commits-each-disableable.md); this pass did not re-read their primary docs or source. Marked **unchecked** for dual-input composition (same caution as cargo-release / release-plz below).

cargo-release / release-plz were not deep-read for this note. Marked **unchecked** rather than asserted as dual-input peers.

### Oakum framing before the decision

ADR-0019 consequences already pointed at the bridge: "`generate` has an honest role: it derives change files from commits, so the two mechanisms converge on one artifact rather than running as parallel code paths." ADR-0023 assigns `generate` ownership of `.changeset/*.md` derived from commits — "writes a file a human can edit, **never the plan directly**." That matched bumpy; whether the plan could *also* read commits when files were enabled was still open until [ADR-0029](../decisions/0029-plan-from-one-intent-artifact.md).

## Conclusions

What the peer evidence supported, and what oakum chose:

1. **Peers that keep both mechanisms either serialize them (bumpy) or merge them silently (knope).** Nobody conflicts. A conflict policy would be original to oakum, not prior art.
2. **bumpy is the closest interface peer** (already primary for `add`/`check`/`generate` flags). Its answer to dual intent is: plan = files only; commits = generate only.
3. **knope's highest-wins merge** remains the documented alternative oakum rejected for plan input: a forgotten `feat:` can raise a carefully authored `patch` file, and coverage semantics get muddy.
4. **`generate` idempotence / double-coverage** is still open as product preference: bumpy always appends a new file; oakum may copy that or later skip packages already listed in pending files.

**Accepted 2026-08-21:** [ADR-0029](../decisions/0029-plan-from-one-intent-artifact.md) chooses Policy A (single artifact). The ADR-0019 composition question is closed.

## Implications / actions

- **Settled 2026-08-21 by [ADR-0029](../decisions/0029-plan-from-one-intent-artifact.md):** Policy A (single artifact).
- Informs **`okm-j1r`** (`generate` implementation): writes `.changeset/*.md` only when both change files and commits are enabled; refuses otherwise.
- Informs **`okm-22h`** (coverage gate): when change files are enabled, coverage is against pending bump files; commits never satisfy `check`. When only commits are enabled, coverage is against commit-derived intent.
- Does **not** by itself settle `init`'s default for which mechanisms are enabled ([specs/init.md](../specs/init.md) open question).

## Open questions

- Whether the commits-only plan path shares implementation with `generate`'s mapper, or is a separate direct path — ADR-0029 requires no file write in that mode; code sharing is optional.
- Should `generate` warn or skip when pending files already cover a package? Peers do not; product preference only.
- release-please / semantic-release / release-plz / cargo-release dual-input behavior — unchecked in this pass.

## Raw data (optional)

| Tool | Plan inputs | Commit → file bridge | Overlap policy when both apply |
|---|---|---|---|
| bumpy 1.18.1 | bump files only | `generate` | N/A at plan time; generate ignores existing files; multi-file plan is highest-wins among files |
| knope (main, 2026-08-21) | commits + `.changeset/` | `CreateChangeFile` / `document-change` (authoring aid, not required) | concatenate changes; version = `.max()`; no conflict |
| changesets | files only | none (issue #862 open) | N/A |
| release-please | unchecked (ADR-0019 secondary) | — | — |
| semantic-release | unchecked (ADR-0019 secondary) | — | — |

### Policies considered (settled by ADR-0029)

| Policy | Summary | Peer analog | Outcome |
|---|---|---|---|
| A. Single artifact | Files enabled → plan reads only `.changeset/*.md`; commits feed `generate` only. Files disabled → plan reads commits. | bumpy (+ ADR-0019/`generate` framing) | **Accepted** ([ADR-0029](../decisions/0029-plan-from-one-intent-artifact.md)) |
| B. Parallel merge | Both feed the plan; bump = highest wins; changelog keeps both | knope | Rejected |
| C. Conflict | Disagreeing levels for the same package fail `check`/`status` | none found | Rejected |
