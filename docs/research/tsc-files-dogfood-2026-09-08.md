# oakum 0.1.4 dogfood: migrating jbabin91/tsc-files from changesets

- Date: 2026-09-08
- Author: Jace Babin
- Scope: what a changesets user hits when migrating a single-package npm repository to oakum 0.1.4, end to end, before the first release lands
- Status (2026-09-10): every finding below is closed, tracked as epic `okm-6vf`. The source repository has since been archived as the Sources section anticipated, so the migration cannot be re-run against it; the fixes are held by oakum's own suites and, next, by the rollout targets in `okm-lhh`. File and line citations below are as of 2026-09-08 and several have since moved.

## Question

Does `migrate` plus the printed workflow get a real changesets repository to a passing `check` and a working release without reading oakum's source? Where it does not, is the gap a defect, a documented design choice, or a missing sentence in the output?

## Sources

- Repository: [jbabin91/tsc-files](https://github.com/jbabin91/tsc-files), `@jbabin91/tsc-files` 0.8.4, changesets 2.30 with `@changesets/changelog-github`, release with `changesets/action` on `workflow_run` after CI, GitHub App token for signed commits, `NPM_TOKEN` secret (not OIDC). Goal: one final patch release (archive notice), then `npm deprecate` and repository archive.
- Binary: `pnpm dlx @oakoss/oakum@0.1.4` (aarch64-apple-darwin, downloaded from the GitHub release). `main` at `89ab395` still carries `version = "0.1.4"` but is 11 commits past the `v0.1.4` tag (`git diff --stat v0.1.4 HEAD -- crates/`: 37 files, +4450 −693; six of the source files cited below changed). Every finding was re-checked against `main`'s source and a `cargo build -p oakum` of it; the one that changed between the two (item 13) says so.
- oakum source: `crates/oakum/src/cli/{status,migrate,init,install_pin,changelog,ci,add}.rs`, `crates/oakum/src/changeset/{read,format}.rs`, `crates/oakum/src/cli/changeset-readme.md`.
- oakum docs: [specs/migrate.md](../specs/migrate.md), [specs/bump-files.md](../specs/bump-files.md), [guide/github-actions.md](../guide/github-actions.md), [ADR-0003](../decisions/0003-write-only-what-a-command-owns.md), [ADR-0007](../decisions/0007-pin-the-tool-version-in-config.md), [ADR-0012](../decisions/0012-scope-v0-to-version-math-and-the-github-layer.md), [ADR-0031](../decisions/0031-write-generated-markdown-genre-intersection.md).

## Findings

### Pin verification and `check`

1. **`check` with no `_config.toml` exits 0 with no output.** Pre-migration, in a repository that has changesets config and no oakum config, `oakum check` printed nothing and exited 0; `status` printed "No packages planned". `load_config` returns defaults when the file is absent, so `tool_version()` is `None` and the pin check is skipped. Once `_config.toml` existed, `check` correctly failed with "unverified: no oakum install pin". [ADR-0007](../decisions/0007-pin-the-tool-version-in-config.md) says "A missed look is never treated as fine"; the no-config case is exactly that.

2. **`check` never recognizes the npm channel pin.** `pnpm add -D -E @oakoss/oakum@0.1.4` wrote `"@oakoss/oakum": "0.1.4"` to devDependencies; `check` still reports "unverified: no oakum install pin in ... `package.json`". `read_package_json_pin` (`install_pin.rs:290`) does `deps.get("oakum")`, the unscoped name, while the published package is `@oakoss/oakum`. This gap is recorded, not accidental: ADR-0007 names "an exact root `package.json` `oakum` dependency" as the install site, and [guide/github-actions.md](../guide/github-actions.md) repeats it and names the `.mise.toml` workaround. So the README's `pnpm add -D @oakoss/oakum` channel can never satisfy `check` through package.json, and fixing it means amending ADR-0007 as well as the code. What is missing regardless: (a) the `check` error message should say so, since it names `package.json` as a place it looked; (b) the README's npm install channel should say this channel alone leaves `check` unverified. Suggested fix: look up `@oakoss/oakum` (keep `oakum` for an alias), and reject `npm:` alias specs with the existing "not an exact version" message.

3. **Workflow pin scanner only knows cargo.** `is_install_line` (`install_pin.rs:861`) gates on `binstall`, `cargo install`, or a `tool:` line, so `pnpm add -D @oakoss/oakum@0.1.4`, `npm i -g @oakoss/oakum@0.1.4`, `npx @oakoss/oakum@0.1.4 check`, or `pnpm exec oakum` in a workflow is invisible. An npm-only repository has no supported way to pin oakum in CI except `cargo binstall` (plus the extra step to install cargo-binstall on the runner) or a `.mise.toml` pin.

4. **mise key for the npm backend is not recognized.** `is_mise_oakum_key` (`install_pin.rs:401`) accepts `oakum` and `cargo:oakum` only. `npm:@oakoss/oakum = "0.1.4"` in `.mise.toml` draws no diagnostic of its own: `check` reports the generic `unverified: no oakum install pin in ... .mise.toml ...`, naming `.mise.toml` as a place it looked without saying it saw an oakum entry there it does not understand. Same root cause as items 2 and 3: the npm channel exists in the README and the release pipeline but not in the pin verifier.

### `migrate` and `init` output

5. **`migrate` wrote files without confirmation when stdin was not a TTY and `--yes` was not passed.** Ran `oakum migrate </dev/null`; it printed the `pending:` list and immediately proceeded to `created ...`. This is documented: [specs/migrate.md](../specs/migrate.md) says non-interactive runs "proceed without prompting and without reading stdin". Kept as a design question rather than a defect: `init --interactive` documents "Exits non-zero when stdin is not a terminal", so the two commands take opposite defaults, and CI is exactly where a non-TTY `migrate` would run by accident. `--help` says `--yes` skips "the confirmation prompt", which does not tell the reader the prompt is already skipped.

6. **`migrate` prints `dropped `<key>` from `.changeset/config.json` (not an oakum config key)` nine times, but config.json is byte-identical afterward** (`git diff -- .changeset/config.json` is empty). The line (`migrate.rs:183`) describes what was not carried into `_config.toml`; the verb "dropped … from" reads as an edit. Editing the file is not an option: config.json is not oakum-owned ([ADR-0003](../decisions/0003-write-only-what-a-command-owns.md)). Reword the verb: "not carried over: `<key>` (not an oakum config key)".

7. **`migrate` output is inconsistent about `.changeset/README.md`.** First line: "`README.md` aborts knope; oakum and @changesets/cli v3 skip it. Expected when migrating from changesets." A changesets user does not know what knope is. Then the `pending:` list still says "write .changeset/_config.toml, .changeset/_schema.json, and .changeset/README.md", the file is not written (no diff), and nothing says it was skipped. The closing "remove `.changeset/_config.toml`, `.changeset/_schema.json`, and `.changeset/README.md` to uninstall" would delete changesets' README, which oakum never wrote.

8. **`migrate`'s "remaining" list does not mention publishing.** The old changesets workflow was also the publish step. The printed workflow ends at `oakum release` (tags + GitHub release). A changesets user following the list would remove the only `npm publish` in the repository and not be told to add one back.

9. **A skipped `.changeset/README.md` is never written later.** First `migrate` skipped README.md because changesets' file existed. After deleting that file, `migrate --yes` prints "already migrated" and exits without writing it. Only `init` writes it, and in my run `init` also left `_config.toml` with `versioning` reset to `zero-major` (see item 10, which I could not reproduce). Either `migrate` should write the missing owned files on rerun, or the "already migrated" path should name what is missing and which command restores it.

10. **Unreproduced: `oakum init` appeared to overwrite an existing `_config.toml`.** In the tsc-files run, `init` (non-interactive) with `_config.toml` present printed `created .changeset/_config.toml` and the file afterwards had `versioning = "zero-major"` instead of `"semver"`. The source does not do that: `write_owned_files` in `init.rs` creates the config with `write_file_exclusive` (`create_new`), and `init.rs` is byte-identical between `v0.1.4` and `main`. Measured on `main` in a scratch repository with `package.json` and an existing `.changeset/_config.toml` holding `versioning = "semver"`: `oakum init </dev/null` printed `already initialized`, exited 0, and left the file byte-identical. The observed reset most likely came from a different step in that session (a `migrate` rerun, or the config being deleted alongside changesets' README before `init`). Reproduce before filing; if `init` ever does replace an existing config, it should refuse or require a flag, the same way `migrate` refuses on a `tool-version` mismatch.

11. **`migrate` does not convert the changesets changelog heading, and `version` refuses it.** Changesets writes `# @jbabin91/tsc-files` as the CHANGELOG.md title. `oakum version` on that tree exits 1: `error: CHANGELOG.md does not start with `# Changelog`; oakum will not append without a recognized heading` (`changelog.rs` pins `const TITLE = "# Changelog"`). `migrate`, `check --strict`, and `status` all pass, so the first sign is the Version PR job failing in CI after merge. `migrate` should rewrite the heading (or at least list it under remaining steps), and `check` should verify the changelog heading the same way it verifies the install pin.

12. **The `init` workflow template runs `oakum check` on the version PR, where it always fails with tag drift.** oakum's own `ci.yml` guards the step with `github.head_ref != 'oakum/version-packages'`; the template printed by `init`/`migrate` (`init.rs:283`) omits that guard. Measured in a copy with real tags: `manifest 0.8.5 is above tagged 0.8.4 / error: 1 package(s) bumped without a tag`, exit 1.

13. **The before-plan comparison does not report what it compared.** On 0.1.4, `migrate` compared the changesets plan with its own and printed nothing about it; with zero bump files both plans were empty and the comparison was invisible. `main` changed this after the tag (`b785228`, #175): `migrate` now prints `plan comparison: before-plan from changesets` when it could run the source tool, or `plan comparison: source tool changesets not runnable (...); using oakum simulation — will exit unverified` when it could not. Still missing on `main`: the package counts and an explicit match line, so "before-plan: 0 packages; after-plan: 0 packages; match" would tell the reader the comparison had something to compare.

### Generated files and the repository's formatter

14. **`add` writes the bump file without a trailing newline.** `pnpm lint:md` (markdownlint-cli2) fails on it with MD047, and Prettier rewrites it. [ADR-0031](../decisions/0031-write-generated-markdown-genre-intersection.md) promises the opposite: "Mechanical envelope only: trailing newline and a blank after the closing `---`". `write` in `changeset/format.rs` appends the note verbatim after `---\n` with neither; measured on a `main` build, `oakum add --empty --message x` wrote nine bytes, `---\n---\nx`. This is a defect against a recorded decision, not a formatter preference. Any repository with a markdown linter in pre-commit will trip on every `oakum add`. The `README.md` and `_schema.json` that `init` writes also fail `prettier --check` (item 16).

15. **Single-quoted scoped package keys are rejected, and the rejection does not fail `check`.** This repository's Prettier config has `singleQuote: true`, which Prettier applies to YAML frontmatter, so `pnpm format` rewrote `"@jbabin91/tsc-files": patch` to `'@jbabin91/tsc-files': patch`. That is valid YAML and `@changesets/cli` reads it. oakum prints `bump file `archive-notice.md`: bump file frontmatter line is not `name: level`: '@jbabin91/tsc-files': patch`, reports "No packages planned", and both `check` and `check --strict` exit 0 (`status --json` returns `packages: []` with the message on stderr). Repository-side workaround applied in tsc-files: a Prettier override setting `singleQuote: false` for `.changeset/*.md`. Two parts:
    - (a) Defect: the parser accepts only double quotes on a scoped name. [specs/bump-files.md](../specs/bump-files.md) says a scoped name "must be quoted" without naming a quote style, so single quotes are within the spec as written.
    - (b) Design question: the skip is documented. The bundled README (`changeset-readme.md:114`) says "A malformed bump file is named on stderr and skipped", and `changeset/read.rs` states the same in its module docs. That choice means a formatter in the PR flow converts a release into a silent no-op, which is the exact failure the oakum README's opening paragraph describes. Worth reopening: a bump file that fails to parse should make `check` exit non-zero, or at least `check --strict` should. Any repository running Prettier with `singleQuote: true` on markdown hits this on every scoped package.

16. **`init` and `add` output does not survive the repository's formatter.** Reproduced in a scratch fixture with oakum 0.1.4 `init` + `add` and Prettier 3.9.6 under tsc-files' base config (`{ singleQuote: true }`, no override); `prettier --check .changeset/*` printed (the directory form, `prettier --check .changeset`, skips `_config.toml` and prints only the three warnings):

    ```text
    Checking formatting...
    [error] No parser could be inferred for file ".../.changeset/_config.toml".
    [warn] .changeset/_schema.json
    [warn] .changeset/oakum-18d374a644d92ad8.md
    [warn] .changeset/README.md
    ```

    The `_config.toml` line is Prettier having no TOML parser, not a formatting issue. What `prettier --write` changes in each: `_schema.json` gets short arrays collapsed onto one line (`"required": ["change-files", "conventional-commits"]`), content unchanged; the bump file gets the scoped key single-quoted plus the missing trailing newline and blank line (items 14 and 15); `README.md` gets table padding, and its `"@scope/my-package": minor` example on line 53 is single-quoted too. Collapse short arrays in the emitted JSON (it already has 2-space indentation and a trailing newline), or document adding `.changeset/_schema.json` to `.prettierignore`; the bump-file half is item 14.

### Text

17. **Doc drift in `status --help`.** `--from` says "Same default as `generate` / `plan-intent`" (`status.rs:25`), but `plan-intent` is hidden plumbing (`hide = true` in `cli/mod.rs`) and does not appear in `oakum --help`. No other `--from` help string names a hidden command; the closest siblings (`version.rs:60`, `preconditions.rs:91`) say "Same default as `generate` / `status`". The fix is the wording, not exposing the command.

18. **Printed workflow assumes `cargo binstall` on the runner.** For an npm-only repository the natural install is the npm channel the README advertises (`pnpm add -D @oakoss/oakum`, then `pnpm exec oakum`). `cargo-binstall` is not on `ubuntu-latest` by default, so the pasted workflow fails on its first step. `migrate` could detect the ecosystem (it already reads package.json to find the package) and print the matching install step. `actions/checkout@v7.0.1` in the template does exist (verified against the tag).

19. **The bundled `.changeset/README.md` talks about knope as the repository's release tool.** Lines 47 and 80 of the template written by `init` explain the unquoted-name rule through knope's behavior and say "Do not introduce those files while knope is still the repository's release tool". For a repository that never used knope the condition never holds, so the sentence is noise about a tool the reader has never seen, next to the same README's advice to use `--empty`/`--none`. The template should be tool-neutral or conditional on the migration source.

### Installation and CI gating

Both measured on tsc-files with oakum 0.1.4 by a second session on 2026-09-08 and cross-checked here against `main`'s source.

20. **pnpm 10 blocks the npm package's postinstall.** `pnpm install` prints `Ignored build scripts: @oakoss/oakum@0.1.4. Run "pnpm approve-builds"` unless `@oakoss/oakum` is listed in `onlyBuiltDependencies`. The CLI still works because `run-oakum.js` downloads the binary on first invocation, so the cost is a warning on every install and a network fetch deferred to the first `pnpm exec oakum`. Neither the README's `pnpm add -D @oakoss/oakum` line nor [guide/github-actions.md](../guide/github-actions.md) mentions the allowlist entry; the guide says `postinstall` downloads the binary, which is not what happens under pnpm 10 defaults.

21. **Plain `check` does not gate on a missing bump file, and the template runs plain `check`.** With a changed package and no bump file, `oakum check` prints `<id>: changed with no covering intent; add a bump file (or `none` / empty frontmatter under --strict)` and exits 0; only `--strict` exits 1 (`preconditions.rs`, `evaluate_coverage`: the `eprintln!` runs unconditionally, `CliError::uncovered` only under `strict`). This is documented in the bundled README ("`--strict` fails when coverage is missing"), and oakum's own `ci.yml` also runs plain `check`. The workflow template printed by `init`/`migrate` (`init.rs:283`) runs `oakum check`, so a changesets user who pastes it gets a CI job that never fails for a forgotten bump file, which is the one thing `changesets/action`'s check did for them. Either the template should use `--strict`, or its comment and the guide should say the check job is informational without it.

22. **The template's version and release jobs fail on `ubuntu-latest` for any npm workspace: pnpm is not provisioned.** First live run of the migrated tsc-files workflow ([run 34283661307](https://github.com/jbabin91/tsc-files/actions/runs/34283661307), push `9462209`): both `oakum ci version-pr` and `oakum release` printed `error: workspace discovery failed (pnpm: could not run pnpm: No such file or directory (os error 2))` and exited 1 before doing anything (`gh run view --log-failed` shows the line in both jobs). oakum asks the package manager for the workspace ([workspace-discovery.md](workspace-discovery.md)), the template (`init.rs` lines 279 to 316) provisions oakum with `cargo binstall` and nothing else, and `ubuntu-latest` does not ship pnpm. [guide/github-actions.md](../guide/github-actions.md) does show `pnpm/action-setup@v4` before `pnpm exec oakum`, so the guide's npm-channel snippet is right and the printed template is not. Same shape as item 18: `init` already knows it found `package.json`, so it can emit the setup step (`pnpm/action-setup`, or `corepack enable`) ahead of every oakum step, or print a comment that pnpm must be on PATH. tsc-files works around it with its own setup composite action before each oakum step.

### Release loop

Both measured on the tsc-files 0.8.5 release and the 0.8.6 version-PR cycle ([#99](https://github.com/jbabin91/tsc-files/pull/99), open at the time of writing) with oakum 0.1.4, by the second session on 2026-09-08, and cross-checked here against `main`'s source.

23. **The GitHub release body carries no changelog entry.** After `oakum release` ([run 34284626905](https://github.com/jbabin91/tsc-files/actions/runs/34284626905)), `gh release view v0.8.5 --json name,body` printed name `@jbabin91/tsc-files 0.8.5` and body `@jbabin91/tsc-files 0.8.5`, while `CHANGELOG.md` on the tagged commit has a `## 0.8.5 (2026-09-08)` section with a `### Fixed` entry. `release.rs` sets the body to the title unconditionally. `version --notes-file` is not a workaround: it feeds the changelog, not the release body, so nothing today produces a body. [ADR-0032](../decisions/0032-synthesize-cascade-changelog-line.md) already assumes "a GitHub release body that pastes the changelog slice", and [ADR-0016](../decisions/0016-emit-release-state-render-it-never-deliver-it.md) notes bumpy's `changelog` versus `github-release` target as worth copying, so the direction is settled: the body should be the package's changelog section for that version.

24. **Tag drift is measured against local tags only, so a stale clone fails falsely.** With `v0.8.5` pushed by CI but not yet fetched, `oakum check --strict` printed `@jbabin91/tsc-files (npm): manifest 0.8.5 is above tagged 0.8.4` and `error: 1 package(s) bumped without a tag`, exit 1; after `git fetch --tags origin` it exited 0 with no other change. The message (`preconditions.rs`, `report_pending`) does not say which tags it compared, and `--remote` covers the opposite direction (newest local tags present on the remote). CI checkouts are fresh, so this is a local paper cut: the message should name local tags and suggest `git fetch --tags`.

Everything else in the loop behaved across the release and the following version-PR cycle: `ci version-pr` opened [#98](https://github.com/jbabin91/tsc-files/pull/98) (run 34284247368) and later rebuilt [#99](https://github.com/jbabin91/tsc-files/pull/99) as one fresh commit after `main` moved and auto-merge was disabled (run 34291373746), the release job no-opped on every non-version push, and the `v0.8.5` tag plus GitHub release triggered the hand-written publish job (run 34284663730, published with provenance).

## Migration outcome (tsc-files)

- `oakum check`, `check --strict`, and `status` all pass with the pin carried by three `cargo binstall --no-confirm oakum@0.1.4` lines in `.github/workflows/release.yaml`. The `@oakoss/oakum` devDependency is kept for local `pnpm exec oakum`, but nothing verifies it against `tool-version` (item 2); the binary's own tool-version gate is the only guard.
- Publishing had to be written by hand as a fourth job on `push: tags: v*`; `oakum release` pushes the tag with a GitHub App token so that job fires.
- The version PR branch name `oakum/version-packages` is only discoverable from source (`crates/oakum/src/cli/ci.rs`); the auto-merge workflow needed it. Worth printing in `migrate`'s remaining-steps list for anyone with a branch-name filter.
- The tsc-files cutover contradicts ADR-0012's 2026-08-31 amendment ("do not update other projects; prove the release loop on oakum"). Deliberate: the maintainer chose to dogfood here.

## changesets versus oakum on the same repository

Compared by the second session on 2026-09-08 from the artifacts each tool produced for tsc-files: the last changesets release, 0.8.4 ([#67](https://github.com/jbabin91/tsc-files/pull/67), run 19874712060, tag `v0.8.4`), and the first oakum release, 0.8.5 ([#98](https://github.com/jbabin91/tsc-files/pull/98), runs 34284247368, 34284626905, 34284663730, tag `v0.8.5`). Cross-checked here: `git ls-remote --tags` shows `v0.8.5^{}` (annotated) and no peeled entry for `v0.8.4` (lightweight); the four runs and both PRs exist with the stated conclusions.

| Artifact | changesets 2.x | oakum 0.1.4 |
| --- | --- | --- |
| Version PR title | `chore(release): :hammer: version package` (repository-configured) | `Version Packages` |
| Version PR body | What merging does, then the full changelog entry with PR, commit, and author links | Plan table (package, from, to, bump, source) and a footer; no changelog text |
| Version commit | App-signed, verified (mechanism inferred from the action's documentation; the run log has expired) | App-signed, verified (GraphQL `createCommitOnBranch`) |
| Changelog entry | `## 0.8.4` / `### Patch Changes` / bullet with PR link, commit link, `Thanks @author`, body | `## 0.8.5 (2026-09-08)` / `### Fixed` / plain body, no links, no author |
| Git tag | Lightweight, on the squash commit | Annotated, tagger `github-actions[bot]`, subject `v0.8.5` |
| GitHub release | Name `v0.8.4`, body is the changelog entry | Name `@jbabin91/tsc-files 0.8.5`, body is that same string (item 23) |
| npm | SLSA v1 provenance | SLSA v1 provenance, from the repository's own publish job |
| Wall clock after the version PR merged | 337 s: CI must finish first (the Release workflow is `workflow_run`-gated, measured on run 19874712060), then one 68 s run does version-or-publish | 98 s: 32 s to tag on the merge push, then a tag-triggered 69 s publish run |

Where oakum came out ahead: tag drift is verified (`check` refusing a bumped-but-untagged manifest is a guard changesets never had, and it fired within the hour); the plan is visible before merge through `status --template summary`; the tag is annotated with a tagger; every workflow step is a command that runs locally instead of one opaque action; and errors name the fix (`check` names the pin, `version-pr` named the branch, `version` refused a foreign changelog heading rather than appending under it).

Where changesets came out ahead: changelog quality (PR, commit, and author links against a bare body under a fixed heading); the release body; a version PR body that tells the reader what merging does; one workflow for version, PR, and publish; and no install-pin ceremony (no `tool-version` gate, no upgrade commit per patch release, no unverified state when the pin sits in `package.json` under the scoped name, items 2 to 4).

Two more defects fall out of the table. The `patch` to `### Fixed` mapping mislabeled 0.8.5, which was an archive notice, not a fix: the level should pick a heading only when the message does not say otherwise. And the changelog entry carries no PR or author link, which changesets' `changelog-github` supplies and which readers of a release page expect.

Net for a single-package npm repository: oakum's release correctness is better, its release output (changelog, release notes, PR body) is worse, and its setup cost is higher. The output gaps are template and text, not architecture.

## Observations (not defects)

- `migrate` inferred `versioning = "semver"` for a 0.8.4 package. Changesets has no such concept so there is nothing to infer from; the flag help says "inferred from the source tool". Worth stating what the default is when the source tool is silent.
- `check` post-migration names the exact fix (pin the same version as `tool-version`), including where it looks (workflows, package.json, .mise.toml, Cargo member). Good.
- Install through `pnpm dlx` works; postinstall prints the GitHub release URL it fetches, which is useful when the second origin is blocked.

## Conclusions

One release went through the loop end to end and a second version PR followed without manual steps, the evidence ADR-0012's rollout bar asks for; what shipped had a release body holding only the title (item 23). A changesets user can reach a passing `check` with 0.1.4, but only by reading oakum's source in four places: the pin scanner (items 2 to 4), the version-PR branch name, the changelog title constant (item 11), and the unguarded template step (item 12). The npm distribution channel is advertised but not verifiable: nothing `check` reads recognizes `@oakoss/oakum`. One output claims a write that did not happen (item 6) and one omits a write that was skipped (item 7), and two generated files contradict ADR-0031's own formatting promise (items 14, 16). The malformed-bump-file skip (item 15b) and the non-TTY auto-proceed (item 5) are documented choices that produced the worst surprises in practice.

## Implications / actions

- Items 2, 3, 4 are one change: recognize the npm channel in every pin source, and amend ADR-0007's install-site list to match.
- Items 6, 7, 8, 9, 11, 12, 13 are `migrate`/`init` output and remaining-steps fixes; no design change needed.
- Item 1 is a "we didn't look" gap in `check`; item 10 needs a reproduction before it is anything.
- Items 14 and 16 are defects against ADR-0031.
- Item 15a is a parser fix within the spec; 15b and item 5 need a decision before code.
- Items 17, 18, 19 are text; item 20 is a README and guide fix.
- Item 21 is a decision: strict by default in the template, or say plainly that the pasted job is informational.
- Item 22 belongs with the template fixes (items 12 and 18): an npm workspace needs its package manager provisioned before any oakum step.
- Item 23 is a `release` defect with its shape already decided by ADR-0032; item 24 is a message fix in `check`.
- The comparison adds two changelog-entry gaps (level-derived heading, no PR or author links); both are template and text work on `version`. Addressed by `okm-6vf.15` (2026-09-09): a note's opening heading picks its section, and the template context carries the adding commit, pull request, and author.
- Filed as epic `okm-6vf` with one child per bullet above (item 10 unfiled until reproduced). Items 12, 14, 16, and 22 are fixed in [#183](https://github.com/oakoss/oakum/pull/183) (open at the time of writing).

## Open questions

- Should `check --strict` fail on a malformed bump file, or should every `check` (item 15b)?
- Should `migrate` match `--interactive`'s non-TTY behavior, or is proceed-by-default the right call for a command that is meant to be run once by a person (item 5)?
- Should `migrate` rewrite a foreign changelog heading, or only report it (item 11)? Rewriting touches a file oakum did not create.
- Should the printed workflow run `check --strict` (item 21)? oakum's own CI does not, but oakum's contributors know the rule and a changesets migrant expects the gate they had.
