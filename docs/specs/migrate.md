# migrate

- Status: draft
- Version: 0.1
- Last updated: 2026-09-05
- Driving ADRs: ADR-0003, ADR-0005, ADR-0007, ADR-0011, ADR-0022, ADR-0023, ADR-0028

## Overview

`oakum migrate` adopts a repository that already uses another release tool. It is separate from [`init`](init.md) because adoption carries risks initialization does not: existing bump files may be in a dialect oakum does not write, and another tool is still reading the same directory.

ADR-0003 restricts a command to the files it owns. A command named `migrate` owns the migration — the objection it encodes is to work performed as an unrequested side effect of something else, not to a command doing the job it is named for. The line drawn here is **transform data, report tooling**.

## Requirements

### Functional

- Convert existing intent files and configuration into the form oakum writes
- Leave the repository initialized, as if `init` had run — with one deliberate exception, `versioning`, which is taken from the source tool rather than from oakum's default
- Prove the migration did not change what would be released. Prefer the source tool's before-plan when it can be run (`okm-45t.1`); when it cannot, compare against an oakum simulation for transform safety and exit unverified. One documented version exception remains: a pre-1.0 knope repository with a pending feature, where [ADR-0022](../decisions/0022-zero-major-versioning.md) deliberately plans a minor where knope planned a patch
- Name every remaining step it does not perform

### Non-functional

- Idempotent — running it twice changes nothing the second time
- Runnable non-interactively with `--yes`
- Shows what it will do before doing it

## Interface / Contract

**Transforms:**

| Change | Why |
|---|---|
| Adopts `.changeset/` in place | The directory name is already correct. bumpy renames `.changeset/` → `.bumpy/` with a plain `fs.rename`; that is pure risk here. |
| Rewrites quoted package keys to unquoted, except scoped npm names | `@changesets/cli` writes every key quoted, and knope silently skips those files with exit 0 and no output. A scoped name keeps its quotes: `@` is a YAML reserved indicator, so unquoting it makes the file unparseable by the tool being migrated away from. |
| Converts `.changeset/config.json` and `.bumpy/_config.json` → `_config.toml` | Carrying over only keys that still mean something. Today that is `privatePackages`, which both tools spell alike and changesets also accepts as a bare boolean, and bumpy's `versionCommitMessage`, which maps exactly onto `commit-message`. The second is carried only when it differs from what `ci version-pr` writes anyway, and refused with a named reason when oakum cannot write it: `commit-message` is rendered as a template, so a literal holding `{{ … }}` would fail the version PR or quietly become a different message, and bumpy takes the key as a string *or a module path*, which oakum runs none of ([ADR-0006](../decisions/0006-no-command-execution-in-templates.md)). One message reaches the file; a second source file stating another is named rather than lost. Every key oakum leaves behind is named too (neither source file is ever edited). A silently discarded key is the failure `docs/research/tool-version-pinning.md` records: a stale `prettier` key survived a changesets upgrade with no error and no warning, and formatting changed underneath the user. A source file oakum cannot open, read, or parse is named as skipped, with what the skip may have cost, rather than stopping the migration: both tools tolerate JSON that `serde_json` rejects, so a leftover config from an abandoned experiment must not block a migration it plays no part in. The exception is containment: a source-config path that escapes the checkout is refused before any of this, by the same check that governs every other path oakum reads. The skip line stands in for the key names oakum could not read, and appears in the closing summary beside them. |
| Leaves `none` / empty frontmatter unchanged (changesets / bumpy); refuses those shapes under knope | Oakum represents releaseless intent in ordinary bump files ([ADR-0028](../decisions/0028-releaseless-bump-files-like-bumpy.md)); coercing to `patch` would invent a release (`okm-ctd`). |
| Writes `_schema.json` and `README.md` | Same as `init`; [ADR-0023](../decisions/0023-name-every-verb-and-what-it-owns.md) assigns `migrate` those files plus the `.changeset/*.md` it transforms. |
| Sets `tag-format` from the tags the repository already carries | A cutover from a tool that tags `name@version` otherwise meets oakum's `{{ package }}/v{{ version }}` default at the first release, which refuses rather than cutting a tag the repository does not use — a good refusal at the worst moment, since the tags were readable at migrate time. Derivation is structural: every split of a tag into a package name, a separator holding no alphanumerics, an optional `v`, and a semver version is a candidate, and a shape is written only when exactly one template explains every tag, and only when oakum's own tag reader can read that shape back ([ADR-0030](../decisions/0030-derive-read-tag-shapes.md)), because a template the reader cannot parse would cut tags oakum then fails to attribute. It is never written when the derived shape is the default that would apply anyway. Tags that disagree, or that no split explains, write nothing and are named among the remaining steps, which list the shapes *this repository* could adopt — the reader is asked for a value, and which one is right depends on the tags the derivation could not reconcile. That list is the readable set minus any shape this workspace cannot use: above one tag-managed package the bare shape drops out, because `release` reads such a tag as leftover ambiguity, and a step that refused a bare shape in one line must not offer it in the next. A tag that claims no version at all — `v1`, `latest`, `nightly`, a date stamp — is skipped rather than allowed to cancel the derivation. `release` already classifies such a tag as someone else's and moves on, and one `v1`, the GitHub Actions convention, used to turn this feature off on exactly the histories it was written for. Skipping happens after the no-tags decision, never before it: a history of nothing but moving tags is undecided and names them, because reporting it as *no tags* would say “never released” about a repository nobody managed to read. A tag oakum's own tag reader can read a version out of is never skipped, even when no workspace package explains it: `stranger@2.0.0` stays unexplained and refusing is still right. The skipped set is exactly the set `release` ignores, which is what keeps the two commands agreeing about the same tag — so a separator oakum does not read (`retired_1.2.3`) is skipped rather than refused, and named on the derived line. Tags that cannot be read at all are reported as unread rather than treated as absent. |
| Sets `versioning` from the source tool | From changesets, bumpy, release-please, semantic-release or nx, `semver` — those take `0.1.3` to `1.0.0`, and silently renumbering an established release line is not a migration's job. From release-plz, `zero-major`: its own configuration documentation states the rule, "the transition from `0.x` to `0.(x+1)` is used for breaking changes". From knope, `zero-major`, which matches knope on breaking changes below 1.0.0 but **not** on features: knope also maps a feature to a patch, and [ADR-0022](../decisions/0022-zero-major-versioning.md) declines that. The plan-equality check below excludes that case rather than failing on it. `--versioning` overrides. This is the one place `migrate` deliberately differs from `init`, which applies oakum's default instead. |

Remaining steps also name the install pin when the repository has none: `migrate` writes `tool-version`, and every later command refuses without a matching pin ([ADR-0007](../decisions/0007-pin-the-tool-version-in-config.md)), so a reader who installed globally would otherwise meet that refusal with the migration already applied. The quoted command is the one the detected ecosystem can run, from the same term that decides the printed workflow's install step — a step offering `cargo binstall` above a workflow that installs through npm contradicts itself.

**Flags:**

| Flag | Effect |
|---|---|
| `--versioning <semver\|zero-major>` | Overrides what would be inferred from the source tool ([ADR-0022](../decisions/0022-zero-major-versioning.md)) |
| `--yes` | Apply without the confirmation prompt. Required when stdin is not a terminal: a non-interactive run without it prints the plan, names the flag, and exits non-zero having written nothing. |

**Reports, does not perform:**

- Removing the old tool's dependency. bumpy shells out to `pnpm remove @changesets/cli`, touching `package.json`, the lockfile, and `node_modules` — three mutations that can fail in ways oakum cannot repair, against one command the user can run and verify.
- Editing or deleting the old tool's workflow. Not oakum's file, by the same decision that makes `check` read-only.
- Deleting the old tool's config.
- Removing a staging file (`.<name>.oakum-write.*`) that an interrupted oakum write left in `.changeset/`. `migrate` names it before the plan on every run, as `init` does; `check` reports it as `unverified`. Nothing sweeps it, because a run still in progress could own it, so the message conditions its advice on no oakum run being in progress.
- Retitling a changelog. changesets writes the package name as the `CHANGELOG.md` title; `version` appends only under `# Changelog` and refuses anything else. `migrate` lists each such file under the remaining steps with the same words `check` uses, and `check` reports it as `unverified` until the first line changes, so the refusal surfaces before the version job fails in CI. `release` does not ask: it never reads the title line ([ADR-0020](../decisions/0020-one-precondition-path.md)). A UTF-8 byte-order mark is the other state `version` refuses, with its own fix. The file is not oakum's to rewrite ([ADR-0003](../decisions/0003-write-only-what-a-command-owns.md)); the old title can stay as a line under the new one.
- Adding oakum to a workflow. It prints the same YAML [`init`](init.md) does, with the exact `tool-version` [ADR-0007](../decisions/0007-pin-the-tool-version-in-config.md) requires already substituted and `actions/checkout` pinned to the latest GitHub release. A missed look is unverified and writes nothing. ADR-0003 forbids writing the file; omitting the YAML from a successful run would be worse, because the end state would be a repository holding oakum config with no oakum invocation anywhere, which `check` reports as not found.

## Behavior

### Breaking the old tool is the transition, not collateral damage

Writing `.changeset/README.md` breaks knope: it treats every `.md` there as a bump file and aborts on the first parse failure. `migrate` writes it anyway, and says so — knope will fail until `knope.toml` and its workflow are removed.

That is deliberate, at a moment the user chose, with the fix named. It is the opposite of `init` silently breaking a tool still in use, which is why the README conditional moved here rather than being handled with a permanently degraded filename.

### `none`-level entries: preserve, never coerce (`okm-ctd`)

[ADR-0028](../decisions/0028-releaseless-bump-files-like-bumpy.md) decides how oakum *expresses* releaseless intent (`name: none` in ordinary bump files). Migration still needs a rule for files another tool already wrote.

| Source | What `migrate` does |
|---|---|
| changesets / bumpy | Leave `none` entries as `none` (and empty frontmatter as empty). Coercing to `patch` would invent a release — the failure knope has for custom types. |
| knope | Refuse if a bump file contains `none` or empty frontmatter. knope should not produce those shapes; if one appears, it is unsafe to treat knope's custom-type reading as oakum releaseless semantics. |

Dropping the entry while keeping the note loses intent. Prompting the user to rewrite each one by hand is unnecessary when the source already used a level oakum understands. The firm constraint: never silently become a patch.

### Verify the plan did not change

Every tool below is looked for in this repository's `node_modules/.bin` before `PATH`. `npm i -g` and Homebrew put `oakum` on `PATH` and a devDependency's binaries nowhere near it, so the recommended install would otherwise leave the parity check unrunnable on the one command whose purpose is comparing against that tool. Never a silent `npx` or network install during migrate. Cargo and mise need no arm: `cargo install` writes to `~/.cargo/bin` and mise installs behind shims, both on `PATH` or the tool does not run at all.

Prefer a **source-tool before-plan** when the detected tool can supply one (`okm-45t.1`):

| Tool | Command | Notes |
|---|---|---|
| bumpy | `bumpy status --json` | Exit 1 with no releases and nothing on stderr is "nothing pending" and still a usable plan; taking that reading is printed, because a crash that happens to emit parseable JSON looks the same. Any other failure is a tool that did not answer, not an empty plan. |
| changesets | `changeset status --output <tempfile>` | |
| knope | `knope <workflow> --dry-run` (`prepare-release` if named in `knope.toml`, else `release`; a `knope.toml` that exists and cannot be read is unavailable, never a silent fallback to `release`) | Narrow scrape of `Would add the following to …: <version>` or `…: version = <version>` (knope ≥0.23). Empty scrape is unavailable, not agreement. |

When that succeeds, the before fingerprint is the tool's output (no oakum remapping of knope's patch-for-feature rule). Compare it to oakum's after-plan (`plan::migrate_compare`). Expected knope feature→patch vs oakum minor fallout is still reported and still exits zero. Unexpected diffs remain hard failures; writes are kept.

When the source tool is missing, fails, or produces nothing usable, migrate still transforms and still runs an **oakum simulation** before-plan (including knope feature→patch remap when `knope.toml` is present) to catch transform corruption. Even when that comparison matches, it exits `unverified` with writes kept. Missing evidence is never treated as agreement.

Two empty plans are not a match. With nothing pending on either side the comparison exercises no transform, so it says the transform was not exercised rather than reporting agreement. This is the common case rather than the rare one, because a repository is usually migrated right after a release, when nothing is pending — a parity check designed for a state most migrations are not in must not read as verification of one they are.

A difference is reported, not silently accepted, and never auto-resolved — the two tools disagreeing about a version is exactly the kind of thing a human should look at.

### Order

1. Detect the source tool and refuse if none is found
2. Attempt a source-tool before-plan; if unavailable, compute an oakum simulation before-plan and mark the run unverified
3. Show every change to be made, and stop unless confirmed or run with `--yes`; without a terminal, only `--yes` continues
4. Transform
5. Recompute the oakum after-plan and compare
6. Report remaining manual steps, including that the old tool will now fail; exit unverified when the before-plan was simulated, or when the gate look could not run

A comparison that found a difference outranks a look that could not happen: the first is a finding, the second is only missing evidence, and reporting the second would tell a caller the transform went unverified when oakum had in fact verified that it changed the release plan ([ADR-0034](../decisions/0034-exit-two-for-unverified.md)).

Nothing is written if any step before 4 fails — [ADR-0011](../decisions/0011-stop-at-the-tag.md)'s replacement for rollback, applied to a transformation: preflight the whole set so most failures abort with nothing to recover.

## Edge cases

- **Nothing to migrate** — reports it and names `oakum init`.
- **Already migrated** — the [ADR-0007](../decisions/0007-pin-the-tool-version-in-config.md) version gate runs first, since it exempts only `upgrade`. If the existing `_config.toml` pins a `tool-version` this binary does not match, `migrate` refuses in either direction and names `oakum upgrade`. Matching, it lists any owned file that has since gone missing (`_schema.json`, `README.md`; never `_config.toml`) under `pending:`, confirms like a first run, restores them, and reports that it has already run; with nothing missing it writes nothing and exits zero.
- **`none`-level bump file** — kept as `none` when the source already uses that level (changesets / bumpy). Oakum understands it ([ADR-0028](../decisions/0028-releaseless-bump-files-like-bumpy.md)). Do not rewrite it to `patch` (`okm-ctd`). A knope source should not produce `none` files; if one appears, report it and refuse rather than let knope's custom-type reading become oakum's release plan.
- **Empty frontmatter bump file** — kept empty when the source is changesets / bumpy. Under knope, refuse the same way as `none`: empty is unsafe while `knope.toml` is present.
- **An agent instruction file already in `.changeset/`** — warned about, never blocking. `AGENTS.md`, `CLAUDE.md`, and `GEMINI.md` match exactly; `README.md` matches in any case. All four abort a knope run, and `migrate` is the only command that runs in a knope repository, so it is the only place the warning can fire. Warnings name the file and which reader it breaks. A lowercase `agents.md` is skipped by neither reader and is the worst of them: warn on it as a bump file, not as a skip. `README.md` warns but never blocks: it is the expected state migrating from changesets, `migrate` leaves an existing one in place, and blocking would break idempotency on a second run.
- **Bump files naming packages not in the workspace** — reported by path, not dropped. The old tool may have been ignoring them silently.
- **Subdirectories in `.changeset/`** — reported. Fatal under `@changesets/cli` v2 and invisible to knope, so they were already doing nothing useful.
- **Plans differ before and after** — reported in full, transformation is kept; reverting would leave the repository in a third state nobody asked for. A difference measured against a before-plan the source tool stated cleanly is a hard failure, exit 1. A difference measured against one read under a tool's convention about what a non-zero exit means — today only bumpy's exit 1 with no releases — is `unverified`, exit 2: a crashed run is indistinguishable from the convention, so the difference is not evidence the transform changed anything.
- **A knope repository with a pending feature below 1.0.0** — the plans differ by construction, because knope maps a feature to a patch there and oakum maps it to a minor ([ADR-0022](../decisions/0022-zero-major-versioning.md)). Report it as an expected divergence naming the packages and both versions. With a real knope before-plan, exit zero; with oakum simulation fallback, exit unverified after the same report. Every other difference is a hard failure unless the before-plan was read under a convention, as above.
- **Source tool cannot supply a before-plan** — transform proceeds; oakum-vs-oakum simulation still runs for transform safety; matching plans still exit unverified (`okm-45t.1`).
- **A scoped npm package alongside `knope.toml`** — refuse, per ADR-0005. Quoting the scoped name satisfies `@changesets/cli` and makes knope skip the file silently; unquoting it inverts which reader breaks. `migrate` is the only command that runs in a knope repository, so this is the one place the rule can fire.

## Open questions

- Whether `migrate` should support the reverse direction. Being able to leave is a reasonable thing to promise, and no surveyed tool offers it.

## Change log

- 2026-08-18: initial draft (v0.1)
- 2026-08-19: `versioning` preserved from the source tool per ADR-0022 (v0.1)
- 2026-08-19: ADR-0007, ADR-0011, and ADR-0023 added to the driving list — the workflow pin, the never-roll-back preflight, and this command's file ownership were each relied on without being declared. ADR-0020 was cited first and is the wrong driver: it settles that `check` and `release` share one precondition *implementation*, which `migrate` does not touch (v0.1)
- 2026-08-21: ADR-0028 makes oakum able to keep `none` / empty; migrate transform policy still open as `okm-ctd` (v0.1)
- 2026-08-23: warnings name the file and reader; case variants of the three agent names are warned as bump files (`okm-3a3`) (v0.1)
- 2026-08-26: printed workflow pin for `actions/checkout` is looked up with `init` (v0.1)
- 2026-09-05: migrate `none` / empty policy settled — preserve from changesets/bumpy, refuse under knope (`okm-ctd`) (v0.1)
- 2026-09-05: source-tool before-plan when runnable; oakum simulation fallback exits unverified (`okm-45t.1`) (v0.1)
- 2026-09-05: plan comparison extracted to pure `plan::migrate_compare`; migration bump load collects unknowns (`okm-45t.3`) (v0.1)
- 2026-09-08: printed workflow gains the pnpm setup step and version-PR guard with `init` (`okm-6vf.4`, `okm-6vf.12`) (v0.1)
- 2026-09-08: output names what it left alone (`config.json` keys, an existing `README.md`), the remaining steps name publishing and the version-PR branch, the plan comparison says which tool planned each side, the uninstall line names only files oakum wrote, owned-file preconditions are checked before the prompt, single-quoted scoped keys parse, and a rerun restores missing owned files after confirmation (`okm-6vf.3`, `okm-6vf.7`) (v0.1)
- 2026-09-09: a changelog `version` would refuse (a changesets package-name title) is a remaining step, and `check` reports it (`okm-6vf.5`) (v0.1)
- 2026-09-09: a non-interactive run needs `--yes`; without a terminal the plan is printed and the run refuses, matching `init --interactive` (`okm-6vf.9`) (v0.1)
- 2026-09-11: the source tool is looked for in `node_modules/.bin` before `PATH`; an unverified outcome exits 2 and an error exits 1 ([ADR-0034](../decisions/0034-exit-two-for-unverified.md)); a tag stating no version is skipped rather than cancelling the tag-format derivation, and a derivation names the tags it stepped over; a bump file copied out of the old tool's directory is described as a copy and every original left behind is named; the `versioning` mode and what settled it are printed; a mention of the old bump-file directory is searched for in the index at a token boundary and named for the reader to judge, with the failure to look reported as unverified; directory listings are sorted (`okm-404.3`, `okm-404.4`, `okm-404.9`, `okm-404.24`, `okm-404.30`) (v0.1)
- 2026-09-11: `versionCommitMessage` is carried into `commit-message` when it differs from the default, announced before the write in the form the file receives, and refused with a named reason when empty, whitespace-only, control-bearing, template-shaped, or a module path; `changelog` is named as a decision the reader owes rather than translated (`okm-404.13`) (v0.1)
- 2026-09-11: `gitUser` is named as a remaining step rather than a forgettable dropped key — it decided who authored release commits and tags, oakum has no counterpart, and the printed workflow now says, at the line that sets the identity, what that identity signs: tags take it as their tagger, while the version commit goes through the GitHub API under the token's own account; a failed bump-file gate look states one fact, with the printed step and the `unverified` refusal derived from one source (`okm-404.10`, `okm-404.46`) (v0.1)
