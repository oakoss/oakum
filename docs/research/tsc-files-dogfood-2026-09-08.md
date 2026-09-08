# oakum 0.1.4 dogfood: migrating jbabin91/tsc-files from changesets

- Date: 2026-09-08
- Author: Jace Babin
- Scope: what a changesets user hits when migrating a single-package npm repository to oakum 0.1.4, end to end, before the first release lands

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

## Migration outcome (tsc-files)

- `oakum check`, `check --strict`, and `status` all pass with the pin carried by three `cargo binstall --no-confirm oakum@0.1.4` lines in `.github/workflows/release.yaml`. The `@oakoss/oakum` devDependency is kept for local `pnpm exec oakum`, but nothing verifies it against `tool-version` (item 2); the binary's own tool-version gate is the only guard.
- Publishing had to be written by hand as a fourth job on `push: tags: v*`; `oakum release` pushes the tag with a GitHub App token so that job fires.
- The version PR branch name `oakum/version-packages` is only discoverable from source (`crates/oakum/src/cli/ci.rs`); the auto-merge workflow needed it. Worth printing in `migrate`'s remaining-steps list for anyone with a branch-name filter.
- The tsc-files cutover contradicts ADR-0012's 2026-08-31 amendment ("do not update other projects; prove the release loop on oakum"). Deliberate: the maintainer chose to dogfood here.

## Observations (not defects)

- `migrate` inferred `versioning = "semver"` for a 0.8.4 package. Changesets has no such concept so there is nothing to infer from; the flag help says "inferred from the source tool". Worth stating what the default is when the source tool is silent.
- `check` post-migration names the exact fix (pin the same version as `tool-version`), including where it looks (workflows, package.json, .mise.toml, Cargo member). Good.
- Install through `pnpm dlx` works; postinstall prints the GitHub release URL it fetches, which is useful when the second origin is blocked.

## Conclusions

A changesets user can reach a passing `check` with 0.1.4, but only by reading oakum's source in four places: the pin scanner (items 2 to 4), the version-PR branch name, the changelog title constant (item 11), and the unguarded template step (item 12). The npm distribution channel is advertised but not verifiable: nothing `check` reads recognizes `@oakoss/oakum`. One output claims a write that did not happen (item 6) and one omits a write that was skipped (item 7), and two generated files contradict ADR-0031's own formatting promise (items 14, 16). The malformed-bump-file skip (item 15b) and the non-TTY auto-proceed (item 5) are documented choices that produced the worst surprises in practice.

## Implications / actions

- Items 2, 3, 4 are one change: recognize the npm channel in every pin source, and amend ADR-0007's install-site list to match.
- Items 6, 7, 8, 9, 11, 12, 13 are `migrate`/`init` output and remaining-steps fixes; no design change needed.
- Item 1 is a "we didn't look" gap in `check`; item 10 needs a reproduction before it is anything.
- Items 14 and 16 are defects against ADR-0031.
- Item 15a is a parser fix within the spec; 15b and item 5 need a decision before code.
- Items 17, 18, 19 are text.
- Filed as epic `okm-6vf` with one child per bullet above (item 10 unfiled until reproduced).

## Open questions

- Should `check --strict` fail on a malformed bump file, or should every `check` (item 15b)?
- Should `migrate` match `--interactive`'s non-TTY behavior, or is proceed-by-default the right call for a command that is meant to be run once by a person (item 5)?
- Should `migrate` rewrite a foreign changelog heading, or only report it (item 11)? Rewriting touches a file oakum did not create.
