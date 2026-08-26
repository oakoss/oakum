# How peers attribute a cascaded bump

- Date: 2026-08-25, revised same evening against pinned trees
- Author: Jace Babin (agent-assisted)
- Scope: what other release tools write into changelogs, GitHub release notes, and PR comments when a package versions only because a workspace dependency is releasing; evidence for `okm-qrx`. [ADR-0032](../decisions/0032-synthesize-cascade-changelog-line.md) chose B; a later `bumpAs` key must be set, and Patch stays the default

## Question

Oakum's planner already distinguishes `ChangeSource::Intent` from `ChangeSource::Cascade { trigger }`. The builtin changelog ignores it: a cascade-only package gets a version heading and nothing else. [ADR-0010](../decisions/0010-derive-cascade-from-declared-ranges.md) rejected a per-file `cascade:` block as the routine graph, but said the attribution problem that block solves "is real and oakum needs an answer."

`okm-qrx` asks how a cascaded bump should read in changelogs and comments. Before choosing, what do peers actually emit?

Three surfaces, kept separate:

1. The dependent package's `CHANGELOG.md` (and the GitHub release body, when the tool copies it).
2. The version PR / release PR body.
3. The contributor-PR comment that previews the plan.

The bead also asks a different question: when a cascade fires, what bump *level* does the dependent get (`bumpAs`)?

## Sources

- [Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/), fetched 2026-08-25 — types of change; "there should be an entry for every single version"; their own `1.1.1` puts "Upgrade dependencies" under `### Changed`; `0.0.4` removes empty sections
- changesets `main` @ [`2eb65ba`](https://github.com/changesets/changesets/commit/2eb65ba75368976251fcb36897634272e890707e) (2026-08-24), re-checked 2026-08-25:
  - [`packages/apply-release-plan/src/get-changelog-entry.ts`](https://github.com/changesets/changesets/blob/2eb65ba75368976251fcb36897634272e890707e/packages/apply-release-plan/src/get-changelog-entry.ts) — direct notes only from changesets that name this package; `getDependencyReleaseLine` always appended under patch
  - [`packages/changelog-git/src/index.ts`](https://github.com/changesets/changesets/blob/2eb65ba75368976251fcb36897634272e890707e/packages/changelog-git/src/index.ts) — default `getDependencyReleaseLine` (`@changesets/cli/changelog` re-exports this)
  - [`packages/changelog-github/src/index.ts`](https://github.com/changesets/changesets/blob/2eb65ba75368976251fcb36897634272e890707e/packages/changelog-github/src/index.ts) — joins commits on one line; backticks and a colon; commit/PR links when GitHub info resolves
  - [`packages/assemble-release-plan/src/determine-dependents.ts`](https://github.com/changesets/changesets/blob/2eb65ba75368976251fcb36897634272e890707e/packages/assemble-release-plan/src/determine-dependents.ts) — dependents enter the plan with `changesets: []` and type `patch` (including peers)
  - [`docs/decisions.md`](https://github.com/changesets/changesets/blob/2eb65ba75368976251fcb36897634272e890707e/docs/decisions.md) — "All updating of dependencies is done as a patch bump. If you want to indicate a more significant change … add a second changeset"
  - [Customize changelog format](https://changesets.dev/guide/customize-changelog-format) — two functions: `getReleaseLine` and `getDependencyReleaseLine`
  - [`packages/cli/CHANGELOG.md`](https://github.com/changesets/changesets/blob/2eb65ba75368976251fcb36897634272e890707e/packages/cli/CHANGELOG.md) — live dependent-only entries (`2.29.7`, `2.28.1`)
  - [`changesets/bot` `index.ts`](https://raw.githubusercontent.com/changesets/bot/main/index.ts) — contributor-PR table is `Name` / `Type` only
  - [`changesets/action` `src/run.ts`](https://github.com/changesets/action/blob/36f529f13ab58bbcf6331035cb950385586e8a89/src/run.ts) (`36f529f`, 2026-08-22) — version PR and GitHub release paste the changelog slice
  - Version PR [#2228](https://github.com/changesets/changesets/pull/2228) and GitHub release [`@changesets/cli@2.29.7`](https://github.com/changesets/changesets/releases/tag/@changesets/cli@2.29.7) — dependent-only bodies in the wild
- bumpy `main` @ [`be33b9d`](https://github.com/dmno-dev/bumpy/commit/be33b9d7ccbfb523921b0a2c67649cfd97e32b08) (2026-08-25); `@varlock/bumpy` 1.18.1 tag `104bb63` has the same default-formatter cascade/dep block:
  - [`packages/bumpy/src/core/changelog.ts`](https://raw.githubusercontent.com/dmno-dev/bumpy/be33b9d7ccbfb523921b0a2c67649cfd97e32b08/packages/bumpy/src/core/changelog.ts) — shipped builtin lines
  - [`packages/bumpy/src/commands/ci.ts`](https://raw.githubusercontent.com/dmno-dev/bumpy/be33b9d7ccbfb523921b0a2c67649cfd97e32b08/packages/bumpy/src/commands/ci.ts) `formatReleasePlanComment` / `formatVersionPrBody`
  - [`packages/bumpy/src/types.ts`](https://raw.githubusercontent.com/dmno-dev/bumpy/be33b9d7ccbfb523921b0a2c67649cfd97e32b08/packages/bumpy/src/types.ts) — `bumpFiles` is "direct only"; `DEFAULT_BUMP_RULES`
  - [`docs/version-propagation.md`](https://raw.githubusercontent.com/dmno-dev/bumpy/be33b9d7ccbfb523921b0a2c67649cfd97e32b08/docs/version-propagation.md) — Phase A levels; per-file `cascade:` wording
  - [`docs/changelog-formatters.md`](https://raw.githubusercontent.com/dmno-dev/bumpy/be33b9d7ccbfb523921b0a2c67649cfd97e32b08/docs/changelog-formatters.md) — custom example; does not document the synthetic builtin lines
  - [`docs/bump-files.md`](https://raw.githubusercontent.com/dmno-dev/bumpy/be33b9d7ccbfb523921b0a2c67649cfd97e32b08/docs/bump-files.md) — listing packages in one file copies the shared body; `cascade:` does not
  - [`docs/differences-from-changesets.md`](https://raw.githubusercontent.com/dmno-dev/bumpy/be33b9d7ccbfb523921b0a2c67649cfd97e32b08/docs/differences-from-changesets.md) — v3 peer dependents are hardcoded patch; bumpy Phase A peers `match`
  - [PR #60](https://github.com/dmno-dev/bumpy/pull/60) (merged 2026-04-29, `6e9195f`) — stopped inheriting the trigger's descriptions; shipped `bumpSources`
- release-please, fetched 2026-08-25:
  - [`src/plugins/node-workspace.ts`](https://raw.githubusercontent.com/googleapis/release-please/main/src/plugins/node-workspace.ts) — `getChangelogDepsNotes`; `PatchVersionUpdate` for a new candidate
  - [`src/plugins/cargo-workspace.ts`](https://raw.githubusercontent.com/googleapis/release-please/main/src/plugins/cargo-workspace.ts) — same notes sentence for Cargo
  - [`src/plugins/workspace.ts`](https://raw.githubusercontent.com/googleapis/release-please/main/src/plugins/workspace.ts) — `appendDependenciesSectionToChangelog` injects `### Dependencies`
- knope, fetched 2026-08-25:
  - [Updating dependencies](https://knope.tech/recipes/updating-dependencies/) — rewrite the dep string on the *trigger's* release, not a dependent version bump
  - [PrepareRelease](https://knope.tech/reference/config-file/steps/prepare-release/) — "runs for each package independently"
  - [Default config](https://knope.tech/reference/default-config/) — workspace members keep dep versions up to date
  - [Change: updating an internal dependency](https://knope.tech/reference/concepts/change/) — classified as a non-change unless users should care
  - knope's own [`.changeset/bump-for-deps.md`](https://github.com/knope-dev/knope/commit/105f7824333f0ae368d68b6a94629288a93eb531) (2026-01-12) — authored "Bump dependencies" when they wanted a dependent version
- cargo-release, fetched 2026-08-25:
  - [README](https://github.com/crate-ci/cargo-release/blob/master/README.md) — "Updates dependent crates in workspace when changing version" means rewrite the dep *requirement*, not the dependent's `package.version`
  - [reference](https://github.com/crate-ci/cargo-release/blob/master/docs/reference.md) — `dependent-version` is `upgrade` | `fix` for that rewrite
  - [FAQ](https://github.com/crate-ci/cargo-release/blob/master/docs/faq.md) — unopinionated about changelogs
- release-plz, fetched 2026-08-25:
  - [FAQ](https://release-plz.dev/docs/faq) — changelog includes commits that touched the crate *or one of its dependencies*
  - [Configuration `changelog_include`](https://release-plz.dev/docs/config) — opt-in copy of another package's commits
  - [`release_plz_core` `updater.rs`](https://docs.rs/release_plz_core/latest/src/release_plz_core/command/update/updater.rs.html) `calculate_package_update_result` — synthetic `chore: updated the following local packages: …`; `increment_patch`
  - [issue #2799](https://github.com/release-plz/release-plz/issues/2799) (open) — that chore is often filtered; cascaded crates re-display an old changelog entry
  - [PR #2708](https://github.com/release-plz/release-plz/pull/2708) — proposed `propagate_major_bump`; not on `main` as of this fetch; live cascade stays patch
- Nx Release, fetched 2026-08-25:
  - [Update dependents](https://nx.dev/docs/guides/nx-release/update-dependents) — side-effectful bump is "always plain patch"
  - [`packages/nx/release/changelog-renderer/index.ts`](https://raw.githubusercontent.com/nrwl/nx/master/packages/nx/release/changelog-renderer/index.ts) — `### 🧱 Updated Dependencies` / `- Updated {name} to {version}`; a dep-only entry is title plus that section
- oakum, read 2026-08-25:
  - `crates/oakum/src/cli/changelog.rs` — builtin is bump-file notes grouped as Added/Changed/Fixed; template context already has `source` and `trigger`
  - `crates/oakum/src/cli/status.rs` — `cascade from {name} ({ecosystem})`
  - [ADR-0010](../decisions/0010-derive-cascade-from-declared-ranges.md), [ADR-0015](../decisions/0015-layer-the-pr-status-channels.md), [ADR-0031](../decisions/0031-write-generated-markdown-genre-intersection.md)
  - [bump-file tool interfaces](bump-file-tool-interfaces.md) (2026-08-18/19) — earlier note that per-file `cascade:` is about attribution

Unchecked on this pass (do not treat as surveyed): Lerna, Rush, semantic-release (single-package).

## Findings

### Four attribution models, not two

| Model | What the dependent changelog claims | Who does it |
|---|---|---|
| **Synthesize a dependency line** | This package released because `{trigger}` is now `{version}` | changesets, bumpy after #60, release-please, Nx |
| **Heading only / empty body** | A version happened; no reason | oakum builtin today; release-plz when a custom git-cliff config skips `chore`; changesets heading-only for `fixed`/`linked` companions with no listable deps |
| **Copy the trigger's notes** | The dependent did the trigger's work | bumpy *before* #60; listing several packages in one bump file (that is intent, not cascade) |
| **No version cascade** | Problem does not arise | knope; cargo-release |

Nobody in the surveyed set copies the trigger package's feature/fix prose into the dependent and presents it as that package's own change. bumpy used to, and [PR #60](https://github.com/dmno-dev/bumpy/pull/60) exists to stop that: "instead of inheriting the source package's change descriptions."

### changesets: two functions, trigger notes never become release lines

`getChangelogEntry` walks changesets and calls `getReleaseLine` only when the changeset *names this package* at a non-`none` type. Dependents added by `determineDependents` have `changesets: []`, so they get no release lines.

It then always calls `getDependencyReleaseLine` with (a) the changesets that belong to the *updated dependencies* and (b) those dependency releases. The default implementation ignores the changeset summaries. `changelog-git` at `2eb65ba` emits one line per changeset, no backticks, no colon:

```text
- Updated dependencies [d0386b6]
 - @changesets/write@1.0.1
```

`@changesets/changelog-github` joins the commits on one line and adds backticks plus a colon (`Updated dependencies [\`d0386b6\`]:`). The live `@changesets/cli` changelog (`2.29.7`, `3.0.1`) uses that github form with commit links. The trigger's own "Thanks @user — added X" line stays on the trigger's changelog.

A dependent-only package in the wild looks like `@changesets/cli` `2.29.7` — heading, Patch Changes, that list, no own notes. The same slice is the GitHub release body and the version-PR section ([#2228](https://github.com/changesets/changesets/pull/2228) shows the same shape for `@changesets/apply-release-plan@8.0.0-next.11`). Mixed releases (`3.0.1`) keep both the own note and the list.

Heading-only is possible but not in the "A left B's range" case: `getDependencyReleaseLine` returning `""` is filtered out, so `## {version}` can be written for a `fixed`/`linked` companion with no listable deps, or for an `optionalDependencies`-only dependent (those bump in `determineDependents` but are not scanned by `get-changelog-entry`). Empty brackets (`Updated dependencies []:`) appear when listed deps have no changeset commits.

`docs/decisions.md` (marked outdated, still on `main`) is the product stance: dependents are always a patch; a more significant dependent change is a second changeset, not a copied note or a `match` bump.

The changelog API is built around the split — [customize-changelog-format](https://changesets.dev/guide/customize-changelog-format) documents `getReleaseLine` and `getDependencyReleaseLine` as the only two hooks. There is no config key for wording.

### bumpy: named the trigger, then stopped copying notes

[version-propagation.md](https://raw.githubusercontent.com/dmno-dev/bumpy/main/docs/version-propagation.md) is the source ADR-0010 already quoted: a per-file `cascade:` marks targets as dependency bumps, "which affects how they appear in changelogs and PR comments." Listing the same packages as ordinary frontmatter entries "are treated as independent changes and each gets the bump file's summary in their changelog."

That block is an attribution switch. The graph is derived separately.

Until April 2026 the formatter still *inherited* the trigger's summary for those marked bumps. [PR #60](https://github.com/dmno-dev/bumpy/pull/60) replaced that with `bumpSources`. The shipped builtin (`packages/bumpy/src/core/changelog.ts` at `be33b9d`, byte-identical on 1.18.1) splits two flags the docs collapse. `sourceList` wraps each name in backticks (`\`core\` v1.1.0`). The PR-comment suffixes are inferred from `formatReleasePlanComment` in `ci.ts` at the same pin; this pass did not run `bumpy ci check`.

| Flag | Typical cause | Builtin changelog line | PR-comment suffix (inferred) |
|---|---|---|---|
| `isDependencyBump` | Phase A out-of-range or Phase C dep rules | Updated dependency \`core\` v1.1.0 | `_(dep)_` |
| `isCascadeBump` | bump-file `cascade:`, `cascadeTo`, `cascadeFrom` | Version bump from \`core\` v1.1.0 | `_(cascade)_` |
| `isGroupBump` | `fixed` / `linked` | Version bump from group with \`core\` v1.1.0 | none unless also dep/cascade |

Fallbacks: `Updated dependency (internal)` and `Version bump via cascade rule` when `bumpSources` is empty. If both dep and cascade are true, the dep line wins. `bumpFiles` stays empty on cascade targets ("no inherited bump files" in the release-plan test). Oakum's graph-derived cascade is the first row, not the per-file `cascade:` row.

The version-PR body (`formatVersionPrBody`) is coarser than the changelog: `- Updated dependencies` or `- Version bump via cascade rule`. The changesets-style "Updated dependencies" string is **not** what the default changelog writes.

A blank bump-file body contributes no changelog line even for a direct package (`docs/bump-files.md`). After #60, cascade-only packages still get the synthetic line. bumpy's own `CHANGELOG.md` has no cascade-only package to quote — it is a single publishable crate.

### release-please: a Dependencies section on both changelog and PR body

`getChangelogDepsNotes` (node and cargo workspace plugins) returns either `""` or:

```text
* The following workspace dependencies were updated
  * dependencies
    * {name} bumped from {old} to {new}
```

(`workspace:` forms say `bumped to` without a from.) `appendDependenciesSectionToChangelog` places that under `### Dependencies`, creating the section if missing. The same string is appended to the candidate's pull-request body notes. A package that was not already a candidate is created with *only* those notes (`newCandidate`).

Dependents added this way are patched (`PatchVersionUpdate` / `bumpVersion`).

### Nx: same synthesize, explicit empty-entry rule

The default renderer treats `dependencyBumps` as data that is "not captured in the commit data, but that nevertheless should be included." It renders:

```text
### 🧱 Updated Dependencies

- Updated {dependencyName} to {newVersion}
```

If there are no commit changes but there *are* dependency bumps, the entry is the version title plus that section — not a heading alone, and not `entryWhenNoChanges`. Only when both are empty does it fall through to the configured empty-entry string or omit the entry.

Side-effectful dependent version bumps are "always plain patch" ([update dependents](https://nx.dev/docs/guides/nx-release/update-dependents)).

### release-plz: a fake chore, then git-cliff may drop it

`dependent_packages_update` does bump dependents (`increment_patch`). The changelog input is not a structured "updated X" line. It is a synthetic commit:

```text
chore: updated the following local packages: {dep}, {dep}
```

(`updater.rs` `calculate_package_update_result`, 2026-08-25.) That commit is then fed through git-cliff. The **default** Keep-a-Changelog parsers map unmatched messages (`^.*`) to `### Other`, so a default repo gets a real bullet. [Issue #2799](https://github.com/release-plz/release-plz/issues/2799) is the common *custom* config that skips `^chore`: the new section is empty, and the release PR "re-displays the previous (old) changelog entry" for each cascaded crate.

The FAQ's "commits that changed one of the files of the crate or one of its dependencies" is a *path* rule for crates that have real commits. `changelog_include` is a separate, opt-in copy of another package's commits (their own repo uses it so `release-plz` includes `release_plz_core`). Neither is "copy the trigger's release notes because we cascaded."

The failure mode that matches oakum's builtin is the skip-`chore` case: a version exists, the reason is not a first-class note, and the reader cannot tell why the package is in the plan. Default release-plz is closer to B, but the sentence is a commit type the project's own cliff config often hides.

### knope: no cascade, so no attribution

[Updating dependencies](https://knope.tech/recipes/updating-dependencies/) is a rewrite of the dependency version string *on the package that is already releasing*. You add `{ path = "crates/knope/Cargo.toml", dependency = "knope-versioning" }` to `knope-versioning`'s `versioned_files`. PrepareRelease "runs for each package independently." A package with no conventional commits and no change files does not get a version because a dependency changed.

That matches the earlier oakum finding (knope rewrites dep strings; it does not range-gate a dependent bump). Attribution of a cascaded *version* does not arise. When knope itself wanted a dependent release (2026-01-12) they wrote a changeset whose note was "Bump dependencies" — authored intent, not a graph line.

cargo-release is the same shape on the Cargo side: `dependent-version` (`upgrade` | `fix`) rewrites path-dep requirements. The dependent crate is released only if you select it. No changelog template for that rewrite.

### PR comments mostly do not distinguish cascade from intent

The changesets bot (`getReleasePlanMessage`) prints every publishable release in one `Name` / `Type` table. A dependent that entered through `determineDependents` shows as Patch. There is no "cascade from" column.

bumpy `ci check` (`formatReleasePlanComment` in `packages/bumpy/src/commands/ci.ts` at `be33b9d`) adds a suffix only: `_(dep)_` or `_(cascade)_`. It does not copy summaries onto packages and does not emit "Updated dependencies" on the contributor PR. The version-PR body is where the coarse synthetic lines appear.

release-please and changesets/action reuse the changelog dependency notes in the release-PR body and the GitHub release. That is attribution on the version PR, not on the contributor PR.

oakum already prints the distinction on `status`:

```text
| Package | From | To | Bump | Source |
| --- | --- | --- | --- | --- |
| pkg (`eco`) | from | to | bump | cascade from trigger (eco) |
```

[ADR-0015](../decisions/0015-layer-the-pr-status-channels.md) already assigned the cascade explanation to the summary (detail) and a short plan to the comment (verdict). No code writes the GitHub comment; `ReleaseSource` already has the data it would need.

### Keep a Changelog has no Dependencies type

The 1.1.0 types are Added, Changed, Deprecated, Removed, Fixed, Security. Their own `1.1.1` records "Upgrade dependencies: Ruby 3.2.1, Middleman, etc." under `### Changed`. `0.0.4` removed empty sections. Guiding principle: "There should be an entry for every single version."

A synthesized dependency line under Changed fits that document. A new `### Dependencies` heading (release-please, Nx's emoji variant) does not. Heading-only satisfies "an entry" only if a version heading counts; the same document's "notable changes" language is why the synthesizing tools write a sentence.

[ADR-0031](../decisions/0031-write-generated-markdown-genre-intersection.md) already pinned oakum's builtin to the genre intersection: date in the heading, Added/Changed/Fixed, no HTML. A builtin cascade line belongs under Changed if it is added. A template can still emit a Dependencies section.

### `bumpAs` is a different question, and surveyed tools mostly patch

| Tool | Ordinary runtime dep | Peer dep |
|---|---|---|
| changesets v3 | patch | patch (v2 was major; not configurable) |
| bumpy Phase A | patch | `match` the trigger |
| release-please | patch | patch (peers only if `updatePeerDependencies`) |
| release-plz | patch | n/a (Cargo); `propagate_major_bump` is a 2026 PR, not on `main` as of this fetch |
| Nx | patch | not split out |
| oakum today | `CascadeAs::Patch` | same |

bumpy's remaining complaint against changesets v3 is that peer dependents are *always* patch, so a breaking peer change needs a hand-written major. That is the `match` exception [ADR-0032](../decisions/0032-synthesize-cascade-changelog-line.md) parked. It changes how large the dependent version is, not what the changelog says.

## Conclusions

Peer evidence only. None of this is oakum's choice:

1. **The plan already knows the source. The gap is the builtin body.** Oakum stores `source` / `trigger` and prints them on `status`. Templates can already write a sentence. The missing piece is the default changelog for a cascade-only package.

2. **Tools that emit attribution synthesize a short line naming the trigger (and usually its new version). They do not copy the trigger's notes.** changesets, post-#60 bumpy, release-please, and Nx each do this. bumpy had to *undo* copying. The surveyed tools already rejected copying.

3. **Heading-only is what you get when the reason is not first-class.** release-plz #2799 (skip-`chore` configs) is the user-visible cost: a version in the plan whose changelog looks like the previous release, or like nothing. oakum's builtin is that shape without the stale body: a heading and no notes. Keep a Changelog still wants a notable-change sentence per version.

4. **Contributor-PR comments barely mark cascade.** changesets does not label it. bumpy adds `_(dep)_` / `_(cascade)_` and stops there. oakum's `status` table already names the trigger, and ADR-0015 already said the summary carries the cascade explanation. Closing `okm-qrx` does not require a new comment format.

5. **`bumpAs` can stay deferred.** Every surveyed default for a normal runtime edge is patch. The only live disagreement is peers (`match` vs patch). That is preference, not attribution, and it does not block the GitHub layer.

## Implications / actions

- Closed by [ADR-0032](../decisions/0032-synthesize-cascade-changelog-line.md) (2026-08-25): **B**, plus `bumpAs` deferred as a key that must be set, not a new default. Options as surveyed:

  | Option | Builtin cascade-only changelog | Peer precedent |
  |---|---|---|
  | **A.** Heading only / no reason line | Heading only (today) | release-plz's accidental empty; no tool *chooses* this |
  | **B.** Synthesize a dependency line | under Changed: `Updated {trigger} to {version}` | changesets, bumpy #60, release-please, Nx |
  | **C.** Copy the trigger's notes | Dependent changelog claims the trigger's work | rejected by bumpy #60; listing packages in one bump file is intent, not this |

- ADR-0032 keeps the B line under Changed, not a new Dependencies section. Templates remain the place for `### Dependencies` or Nx-style copy.
- Do not invent a `release` verb or a per-file `cascade:` block to get attribution. The plan already has the fields.
- `bumpAs` / peer `match` is deferred: a later key must be set, and Patch stays the default.

## Open questions

- Whether a package that is *both* intent and cascade should get the synthetic line. [ADR-0032](../decisions/0032-synthesize-cascade-changelog-line.md) left this unconsidered; compose keeps `ChangeSource::Intent` when both apply.
- Lerna / Rush / semantic-release — unchecked.
