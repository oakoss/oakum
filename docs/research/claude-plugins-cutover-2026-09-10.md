# oakum 0.2.0 rollout: migrating oakoss/claude-plugins from bumpy

- Date: 2026-09-10
- Author: Jace Babin
- Scope: what a bumpy user hits migrating a three-package private monorepo to oakum 0.2.0 — install, `migrate`, a green `check` — stopping deliberately before the CI cutover and before any release
- Status (2026-09-10): every finding is filed. F7–F9 are `okm-404.7`, `.11` and `.12`; F11–F17 are `.10`, `.13` (carrying F12 and F13 together), `.15`, `.8` and `.16`; F1–F5 and the round-1 items were filed as they were reported. **F14 is a closed decision** — the maintainer chose to keep the changelog footer, and the churn objection measured weaker than it read: one line per release, inside a diff the same command is already making. **F10 was measured wrong when first written**; it is corrected in place below, with the measurement that overturned it and the reason the original reading was invalid. A dated record whose findings have moved on is still accurate about the day it describes.

## Question

Does `migrate` plus a green `check` get a real bumpy monorepo — every package `private: true` — to a config that actually manages its packages, without hand-editing and without reading oakum's source? Where it does not, is the gap a defect, a design choice, or a missing sentence in the output?

The second question, which turned out to matter more: **is a green `oakum check --strict` evidence that the migration worked?**

## Sources

- Repository: [oakoss/claude-plugins](https://github.com/oakoss/claude-plugins) at `ba31179`. Three plugins — `review-cycle` 0.17.0, `prose` 0.1.0, `pr-kit` 0.1.0 — all `private: true`. Their `package.json` files are version anchors only; the real manifests are `plugins/<name>/.claude-plugin/plugin.json` plus a shared `.claude-plugin/marketplace.json`, kept in sync by an 81-line `scripts/sync-plugin-versions.mjs`. "Publishing" is git tags and GitHub releases; nothing goes to npm.
- Source tool: `@varlock/bumpy` 1.18.1, wired into `ci.yml` (`bumpy ci check --strict`), `release.yml` (`bumpy ci plan`, `bumpy ci release --expect-mode`), and a repo-local Claude Code PreToolUse hook that greps `.bumpy/*.md`. Carries a local patch, `patches/@varlock__bumpy@1.18.1.patch`, which exists to work around the private-package tagging defect that prompted this migration.
- Binary: `pnpm add -D @oakoss/oakum@0.2.0` (aarch64-apple-darwin, downloaded by the package's postinstall from the GitHub release). Round-1 retest used a debug build of branch `fix/private-packages-and-unmanaged-check`.
- oakum source read during the migration: `crates/oakum/src/cli/release.rs`, `crates/oakum/src/config.rs`, `crates/oakum/src/template.rs`, and the generated `.changeset/_schema.json`.

## Findings

### Install

**F15 — the npm package needs a pnpm `allowBuilds` entry, and nothing says so.** `@oakoss/oakum` ships a `postinstall` that downloads the platform binary. pnpm 10+ blocks postinstall by default, so the first install produced a package with no binary and no error — `ERR_PNPM_IGNORED_BUILDS` is a warning and the install exits 0:

```console
$ pnpm add -D @oakoss/oakum@0.2.0
devDependencies:
+ @oakoss/oakum 0.2.0
[ERR_PNPM_IGNORED_BUILDS] Ignored build scripts: @oakoss/oakum@0.2.0

Run "pnpm approve-builds" to pick which dependencies should be allowed to run scripts.
```

pnpm also wrote a literal placeholder into the repository's config — `'@oakoss/oakum': set this to true or false` — which is invalid until edited by hand. Setting it to `true` and reinstalling worked:

```console
$ pnpm install
.../node_modules/@oakoss/oakum postinstall$ node ./install.js
.../node_modules/@oakoss/oakum postinstall: Downloading release from https://github.com/oakoss/oakum/releases/download/v0.2.0/oakum-aarch64-apple-darwin.tar.xz
.../node_modules/@oakoss/oakum postinstall: @oakoss/oakum has been installed!
$ pnpm exec oakum --version
oakum 0.2.0
```

One line in the install docs would cover it: *"On pnpm, add `'@oakoss/oakum': true` under `allowBuilds`."*

**F19 — same-day publication trips supply-chain gating.** 0.2.0 was published the day it was installed, so pnpm's minimum-release-age policy refused the lockfile and wrote a per-version exemption into the repository's workspace file. Measured by removing it:

```console
$ pnpm install --frozen-lockfile --ignore-scripts
ERR_PNPM_MINIMUM_RELEASE_AGE_VIOLATION — @oakoss/oakum@0.2.0 was published at
2026-09-10T15:04:46.000Z, within the minimumReleaseAge cutoff (2026-09-09T16:52:52.862Z)
```

Not oakum's bug, but any repository with release-age gating that adopts oakum on release day acquires a standing exemption entry that must be hand-edited at every oakum bump — a third place to update, alongside the devDependency and `tool-version`. Belongs next to F15 as one install-docs item.

**F16 — the install pin is enforced, but `migrate` does not mention it.** Running oakum in a checkout without a matching pin:

```console
$ oakum check --strict --from HEAD~1
error: unverified: no oakum install pin in `.github/workflows`, `.github/actions`, `package.json`, `.mise.toml`, or a Cargo workspace member named `oakum`; pin the same version as `tool-version` (`0.2.0`), for example `cargo binstall --no-confirm oakum@0.2.0` or `pnpm add -D @oakoss/oakum@0.2.0`
```

Good guard, good message. The problem is sequencing: `migrate` writes `tool-version = "0.2.0"` and reports success, and does not list "add an install pin" among its remaining steps. A user who installed globally via Homebrew or `npm i -g` — two of the three documented routes, and the two that leave no pin in the repository — then finds every subsequent oakum command refusing to run, with a migration already applied. This migration never hit it in the real repository only because the devDependency added at step 1 happens to be an accepted pin location.

### `migrate`'s config output

**F1 (severe) — `migrate` dropped `privatePackages`, producing a silently no-op config.**

The source config set it explicitly:

```json
"privatePackages": { "version": true, "tag": true }
```

oakum has the identical knob, described in its own schema as "Opt-in to version and/or tag packages that are not registry-publishable". It is not opt-out; it defaults off. `migrate` did not carry it, and wrote a five-line config in total:

```toml
#:schema ./_schema.json
tool-version = "0.2.0"
change-files = true
conventional-commits = true
versioning = "semver"
```

Under that config the entire repository is invisible. Same tree, same committed diff (`HEAD~1..HEAD` changed `review-cycle`), config differing only by the `[private-packages]` block:

```console
# migrate's config exactly as written
$ oakum check --strict --from HEAD~1 ; echo "EXIT=$?"
EXIT=0
$ oakum status --from HEAD~1 --template summary
## Release plan

No packages planned.

# with [private-packages] version=true tag=true added
$ oakum check --strict --from HEAD~1 ; echo "EXIT=$?"
review-cycle (npm): changed with no covering intent; add a bump file (or `none` / empty frontmatter under --strict)
error: 1 package(s) changed with no covering intent
EXIT=1
```

No warning, no diagnostic. A release in that state would do nothing and report success.

**The trap worth naming separately:** the instruction driving this migration was "get `oakum check --strict` to exit 0". That is satisfied *instantly* by the broken config. The green check was reachable precisely because the configuration was wrong.

One path did report the problem, but only when a change file names the package by hand:

```console
$ oakum add --packages review-cycle:patch --message "..." --name migration-probe
$ oakum status --template summary
error: `review-cycle` is named by intent but is not version-managed; adjust `include`/`exclude` or set `private-packages.version = true`
```

That message is excellent — it names the fix. It fires on the wrong path and too late.

**F2 (severe) — `check --strict` did not surface the not-version-managed condition.** F1's evidence doubles as this one. `status` errored on a config that makes packages unmanaged; `check --strict`, whose own help says "Report drift and name the fix", exited 0 and silent on the identical state. `check` is what CI gates on and what the migration instructions point at.

**F3 — the parity check reported "match" over an empty set.**

```text
plan comparison: 0 package(s) planned by bumpy and by oakum; match
```

`.bumpy/` held no pending bump files, so both sides planned nothing and the comparison was vacuous. As printed it reads like the migration was verified against the old tool. It verified nothing, and it could never have caught the dropped `privatePackages`.

This matters more than it looks, because the empty case is the *normal* case: you migrate right after a release, when no bump files are pending. The parity check is designed for a situation most migrations will not be in.

**F4 — parity degrades to self-comparison when the old tool is not on PATH, then exits 1 with files written.** Reproduced in a throwaway clone with four real bumpy bump files present and `bumpy` absent from PATH:

```console
$ oakum migrate --yes ; echo "EXIT=$?"
plan comparison: source tool bumpy not runnable (`bumpy` not found on PATH); using oakum simulation — will exit unverified
pending:
  rewrite .changeset/verify-release-notes.md
  ... (3 more)
plan comparison: 3 package(s) planned by the oakum simulation and by oakum; match (unverified: bumpy did not run)
...
error: unverified: migrated files were kept; source-tool before-plan unavailable (bumpy): `bumpy` not found on PATH
EXIT=1
```

Two problems. First, `3 package(s) planned by the oakum simulation and by oakum; match` is oakum compared against oakum — it cannot detect a mapping error, which is the only thing the parity check exists to detect. The `(unverified: ...)` tag is honest, but the word "match" still carries the sentence.

Second, and more practically: **this is what the recommended install produces.** Two of the three install routes (`npm i -g`, Homebrew) put `oakum` on PATH but not `bumpy` — a devDependency's binaries only reach PATH through `pnpm exec` / `npm exec`. A user who installs oakum globally, as the docs suggest, and runs `oakum migrate` in a pnpm repository gets a red `error:` and exit 1 on a migration that actually succeeded. Confirmed against the release build on a clean clone:

```console
$ oakum migrate --yes ; echo "EXIT=$?"
EXIT=1
error: unverified: migrated files were kept; source-tool before-plan unavailable (bumpy): `bumpy` not found on PATH
$ command -v bumpy || echo "(no)"
(no)
```

Suggested: detect the old tool in `node_modules/.bin` (and the Cargo/mise equivalents) before declaring it unrunnable, and separate "migration failed" from "migration completed, parity unverified" in the exit code.

**F5 — migrated bump files are copied, not moved, and the cleanup list does not mention the originals.** `migrate` says `rewrite .changeset/<name>.md`. The verb suggests transformation in place. What it does is write a new file under `.changeset/` and leave the original in `.bumpy/` untouched — byte-identical, since both tools use the same changesets-style frontmatter (`diff` reports no difference).

The "remaining (oakum does not perform these)" list names only `- remove .bumpy/_config.json (bumpy)`. Follow it literally and the bump files stay behind. In the clone, after `migrate --yes` and `oakum version` had consumed the `.changeset/` copies and released `review-cycle` 0.17.0 → 0.18.0:

```console
$ bumpy status
3 bump file(s) pending

Minor
  pr-kit: 0.1.0 → 0.2.0
  review-cycle: 0.18.0 → 0.19.0

Patch
  prose: 0.1.0 → 0.1.1
```

bumpy would release the same changes a second time. In this repository that is not hypothetical: `release.yml` is still wired to `bumpy ci release` and fires on every push to main, so merging a migration that kept `.bumpy/*.md` would trigger a duplicate release on the merge commit. Suggested: move rather than copy, or at minimum list the bump files in the cleanup steps.

**F6 — `versioning = "semver"` is written silently, and it is not the default.** The schema says `zero-major` is the default and that `semver` means "the next major file produces 1.0.0". `migrate` wrote the non-default without saying so. For a repository whose packages are all below 1.0.0 this is the single most consequential line in the file.

Measured rather than assumed. A `major` change file against `review-cycle` at 0.17.0:

| Tool / setting | Result |
| --- | --- |
| bumpy 1.18.1 | 0.17.0 → 1.0.0 |
| oakum, `versioning = "semver"` | 0.17.0 → 1.0.0 |
| oakum, `versioning = "zero-major"` | 0.17.0 → 0.18.0 (bump downgraded to minor) |

So migrate's choice is **correct** — it preserves bumpy's behavior, which has no zero-major concept at all (`versioning` does not exist in bumpy's schema). The finding is only that a load-bearing, non-default, behavior-defining choice is made silently. One line in the plan output would fix it.

**F13 — `versionCommitMessage` is not carried.** bumpy: `"chore(release): version packages"`. oakum has `commit-message`, an exact equivalent. Not carried, restored by hand. oakum's default was not measured, because the only command that consumes it (`oakum ci version-pr`) writes to GitHub — *inferred, not measured*: if the default is not conventional-commit shaped, it would collide with this repository's commitlint.

**F12 — the `changelog` formatter is not carried.** bumpy was configured with `["github", { internalAuthors: ["jbabin91"] }]`, which appends PR and author links and suppresses them for the listed maintainer. oakum's nearest equivalent is `template`. Not carried, no mention. Measured effect at cutover (see F14's snippet): entries lose PR links, the bump-type marker, and author attribution.

**F11 — no `gitUser` equivalent; the printed workflow changes the commit identity.** bumpy committed releases as `oakoss[bot] <oakoss[bot]@users.noreply.github.com>`, configured once. oakum's schema has no identity key, and the workflow `migrate` prints hardcodes a different one:

```yaml
- run: |
    git config user.name "github-actions[bot]"
    git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
```

Paste it as-is and every future release commit and tag changes author. This repository mints a token from its own GitHub App precisely so bot pushes retrigger CI — `github.token` pushes do not, by GitHub's anti-recursion guard — so the printed workflow is not a drop-in here for a second reason. Both are fine as manual steps; neither is called out in the "remaining" list, which is where a reader looks for exactly this.

### Silence

**F7 — `oakum version` rewrites the repository and prints nothing.**

```console
$ oakum version ; echo "EXIT=$?"
EXIT=0
```

That run bumped `package.json`, `.claude-plugin/plugin.json`, `.claude-plugin/marketplace.json`, rewrote `CHANGELOG.md`, and deleted the consumed change file — five files, zero output. `bumpy status` prints a table of what it will do. For the command that performs the irreversible part of a release, silence is the wrong default.

**F8 — `check --strict` prints nothing on success.**

```console
$ oakum check --strict ; echo "EXIT=$?"
EXIT=0
```

No indication of what it examined, which packages it considered, or what ref it diffed from. When the answer is "nothing changed since `review-cycle@0.17.0`", saying that is the difference between a check that passed and a check that ran. It also compounds F1 directly: silent success is indistinguishable from silent non-participation, and this silence is what let the broken config read as green.

**F9 — a config error names the line but not the key.** Adding `commit-message` after the `[private-packages]` table scoped it into that table. oakum said:

```text
error: `.changeset/_config.toml` is not a valid oakum config: TOML parse error at line 17, column 1: unknown configuration key
```

The line number and the diagnosis are right. Naming the key it resolved — `private-packages.commit-message` — would have made the table-scoping mistake obvious instead of a puzzle. Easy to hit, because the config mixes top-level scalars with tables and every scalar added after a table lands in the wrong place.

**F17 — `status` and `status --template summary` are identical.** Bare `status` renders the summary template. Neither `--help` nor the command description says the summary is the default; the description reads "Print the versioned release state as JSON or a named render", which suggests JSON is.

### Coverage rules and the CI surface

**F10 (corrected) — no `changedFilePatterns` equivalent, but this is not a regression.**

**This finding was wrong when first written, and the correction is the finding.** It was originally filed as high severity: that bumpy passes a documentation-only PR where oakum fails one, leaving no knob to restore the exemption. A reviewer contradicted it. Re-measuring on the shape CI actually uses — a feature branch off `main` carrying one commit touching only `plugins/prose/README.md`:

```console
$ bumpy check --strict --base main
error 1 changed package(s) missing bump files:
  prose
EXIT=1

$ oakum check --strict --from main
prose (npm): changed with no covering intent; add a bump file (or `none` / empty frontmatter under --strict)
error: 1 package(s) changed with no covering intent
EXIT=1
```

Both fail, identically. The original measurement used `--base <ancestor-ref-on-the-same-branch>`, where bumpy printed `info No changed files detected.` — an invocation shape that yields an empty changed-file set for some reason other than the exclusion working. That was read as the exclusion applying. It does not.

What survives, and points at bumpy rather than oakum: `.bumpy/_config.json` lists `!README.md` and `!tests/**`, and no invocation could be constructed where those patterns actually excluded `plugins/<name>/README.md`. This repository's documented docs exemption was already not being honoured before oakum entered the picture.

What remains true about oakum: its schema has no `changedFilePatterns` equivalent, so the exemption cannot be configured even in principle. The escape hatch is a `--none` or `--empty` change file per docs PR, which oakum's error message names. **The cutover does not change doc-only PR behavior; it makes the existing behavior unconfigurable.** Demoted from high to low.

One difference does stand, unrelated to docs: `oakum check` ignores the uncommitted working tree, where bumpy inspects it. Nothing here depends on that (lefthook runs neither tool's `check`), but a project gating on `bumpy check --hook pre-commit` would find the equivalent gate silently stops firing.

**F18 — the CI command surface has no mapping from bumpy's.** bumpy's `release.yml` is built on a plan/act split: `bumpy ci plan` reports a mode (`version-pr`, `publish`, or nothing) as a job output, and two downstream jobs run `bumpy ci release --expect-mode <mode>`, so the wrong action cannot fire. oakum offers `oakum ci version-pr` and `oakum release` with no plan step and no `--expect-mode` guard; the printed workflow runs both jobs unconditionally on every push to main and lets each decide internally whether to act.

That may well be the better design, but it means this repository's release workflow cannot be translated mechanically — it has to be rewritten, and the `expect-mode` safety property is not expressible.

### F14 — the changelog footer (closed decision, recorded for completeness)

`oakum version` appended this as the **last line of the file**, below the oldest 0.1.0 entry, 540 lines from the top:

```diff
 - Embedded comment and fix-vs-defer policies inside the skills, ...
+
+Generated by oakum 0.2.0.
```

Filed as an objection on two grounds: buried at the bottom it informs nobody, and pinning the version means churn on every oakum upgrade. **The maintainer chose to keep it**, and the churn objection measured weaker than it read — one line per release, inside a diff the same command is already making. Recorded here because the placement observation stands even though the decision went the other way.

### Where oakum is better than bumpy

Not everything was a finding. Four measured improvements.

**`extra-files` can retire `scripts/sync-plugin-versions.mjs` entirely.** This repository maintains 81 lines of Node, plus a `--check` drift gate in CI, plus a bats suite, purely to copy a version from `package.json` into `plugin.json` and into the right array element of `marketplace.json`. oakum declares that:

```toml
[[packages.review-cycle.extra-files]]
path = ".claude-plugin/plugin.json"
format = "json"
key = "version"

[[packages.review-cycle.extra-files]]
path = "/.claude-plugin/marketplace.json"
format = "json"
key = "plugins.{name=review-cycle}.version"
```

Verified: `oakum version` wrote 0.18.0 into all three files in one pass, including selecting the correct `marketplace.json` array element by `{name=review-cycle}`. This is the single strongest argument for the migration and `migrate` never mentions it — it cannot know about the sync script, but a migration guide could ask "does your repository have a script that copies versions into other files?"

**The generated changelog passes this repository's markdownlint clean.** bumpy writes the version heading and its `<sub>` date on adjacent lines, tripping MD022 and failing the whole version PR; `release.yml` carries a documented `markdownlint-cli2 --fix` workaround with a nine-line comment explaining it. oakum's output needed no fixing (`Summary: 0 issues in 0 files`), so that workaround can be deleted at cutover.

**Error messages name the fix.** `set private-packages.version = true`, `add a bump file (or none / empty frontmatter under --strict)`, `pin the same version as tool-version`. Consistently better than bumpy's.

**The install pin guard has no bumpy equivalent** and would have caught a class of "CI ran a different version than you did" problem. See F16 for the sequencing issue, not the idea.

### Where the documentation gap is

This is the section with no natural home in a tracker, and the one the migration turned up that a bead cannot hold. Four places where the only way to learn a rule was to read oakum's source or schema. **Two of the four have since produced their own bugs**, which is evidence the section measures something real rather than listing inconveniences.

1. **`.changeset/_schema.json` — to discover `private-packages` existed at all.** The migrate output gave no hint that a setting had been dropped. The only reason to go looking was independent knowledge that every package in this repository is private. Without that, the repository would have released nothing, silently, and F1 would have been found in production. *This one produced a bug.*

2. **`.changeset/_schema.json` again — for `extra-files`**, discovered by noticing the phrase "declared extra-files" in `oakum version --help` and then digging into `properties.packages.additionalProperties`. Nothing surfaces it in `migrate`, `--help`, or the generated README — and it is the strongest argument for adopting oakum in a repository shaped like this one.

3. **`crates/oakum/src/cli/release.rs:26` — for the `tag-format` template variable.** `{{ name }}` yields `error: undefined value (in tag-format:1)`, which names neither the offending variable nor the valid set. The correct variable is `{{ package }}`, and the default multi-package format is `{{ package }}/v{{ version }}`. The schema description does not name the variables, and every `tag-format` example in oakum's own test suite uses `{{ version }}` alone — so nothing in the repository shows a reader that a package variable exists or what it is called. *This one produced a bug.*

4. **`crates/oakum/src/template.rs:152` — for the placeholder syntax itself** (`{{ name }}`, Handlebars-style), before discovering in (3) that the variable name was wrong anyway.

Reading oakum's *source* was never necessary to understand its behavior — the schema descriptions are good. The problem is that `migrate`, the command whose whole job is to hand a user a working config, does not point at them.

## Round 1 retest (branch `fix/private-packages-and-unmanaged-check`)

Tested the same day against a debug build, on a pristine clone at `ba31179` with the pin added by hand.

### Fixed

**F1 is fixed.** `migrate` now prints `carried over: privatePackages from .bumpy/_config.json (private-packages = { version = true, tag = true })` and writes that line. No hand-editing.

**F1's root cause is fixed too**, which matters more: `migrate` now reports every key it did *not* carry, one line each — `$schema`, `baseBranch`, `changedFilePatterns`, `changelog`, `gitUser`, `versionCommitMessage`. The absence of exactly this reporting is what made F1 invisible.

**F2 is fixed for `check`.** On a config with no `private-packages`, no `include` and no `exclude`:

```text
error: unverified: this config manages no package on either axis, so no plan can name one
and a release would do nothing; every selected package is private, so set
`private-packages.version = true` to version them, and `private-packages.tag = true` to tag them
```

Which condition it keys on — **selection non-empty and no axis able to produce work**, not "did include/exclude narrow":

| config | `check --strict` |
| --- | --- |
| `private-packages` absent, no include/exclude | exit 1, refuses |
| `private-packages` set, `exclude` naming all three plugins | exit 0, silent |
| `private-packages` absent **and** `exclude` naming all three | exit 0, silent |

The third row is the precedence answer: exclusion empties the selection first, so the axis question never arises. Defensible, and worth confirming as intended rather than incidental.

### New from the retest

**R1 (high) — `migrate` does not set `tag-format`, and the default does not match this repository's tags, so the first `release` refuses.** After migrate, `oakum version` and a commit:

```console
$ oakum release
error: unverified: tag-format renders `pr-kit/v0.1.0` for pr-kit 0.1.0, but the tag that
exists for that version is `pr-kit@0.1.0`; the resume asks only about `pr-kit/v0.1.0`,
so reconcile the tag-format with the existing tags
```

The default multi-package format is `{{ package }}/v{{ version }}`; every tag this repository has is `name@version`, as are a changesets or bumpy monorepo's. Refusing beats cutting wrong tags, but eleven `name@version` tags were sitting in the clone and `migrate` read none of them. Same class as F1: a source-side fact available at migrate time that migrate does not pick up. The schema says of `tag-format` that existing tags are derived, not configured, citing ADR-0004; the derivation did not pick up `@` here.

**R2 (moderate) — the `tag-format` template variable is undocumented and the error is unhelpful.** See documentation gap (3) above.

**R3 (moderate) — `versionCommitMessage` is reported as "not an oakum config key", which is false.** `commit-message` is in the schema `migrate` wrote in the same run. It is the one remaining mechanically-carryable setting — a direct rename — and the new reporting now forecloses the question rather than leaving it open.

**R4 (moderate) — `status` is still silent where `check` now refuses.** On the omission config, `status --template summary` prints `No packages planned.` and `status --json` returns `{"packages": [], "uncovered": []}`, both exit 0. A dashboard or CI step reading that JSON concludes there is nothing to release, from a config that can never release anything.

**F20 — oakum rejects a quoted unscoped package name.**

```console
$ oakum status --template summary
error: `zz-quoted-probe.md` is not a bump file: package `review-cycle` must not be quoted (only scoped npm names are quoted)
```

bumpy accepts both forms, and this repository's `AGENTS.md` documents the quoted form. So a bump file valid under bumpy is rejected by oakum, and `migrate` neither rewrites nor warns about it. Compounded by the generated `.changeset/README.md:48`, which states oakum reads a quoted bare name too — that file ships compiled into the crate and is written into every repository `init` and `migrate` touches, so the document teaching the rule contradicts the parser enforcing it.

### The tag axis: still inferred

`oakum release` exits 1 on `error: oakum release needs GITHUB_TOKEN or GH_TOKEN` before reaching any tagging, so with no token there is no path to exercise it, and fabricating a credential to probe an external API was not warranted. Stronger than schema-reading, though: `config.rs:240` gates taggability on `selected(package_name) && (publishable || self.private_packages.tag)`, so for a private package the axis is load-bearing by construction. Carried correctly by migrate (measured in the written config); its effect read from source, not run.

## Conclusions

`migrate` on this repository produced a configuration that reported success, passed the strict check, and would have released nothing. That is the finding the rest of the report orbits. Round 1 fixed both halves — the carry and the refusal — and the not-carried reporting it added is the more durable fix, because it turns every future omission of this class into a visible line rather than a silent default.

The second conclusion is about the instruction, not the tool: **"get `check --strict` to exit 0" is not a migration acceptance test.** It was satisfied by the broken config, instantly. An acceptance test that would have caught F1 is "`status` names every package you expect to release" — the plan, not the gate.

Third: the gap between what oakum's schema knows and what its output says is where this migration lost time. `private-packages`, `extra-files`, `commit-message` and `tag-format` are all in the schema; none is mentioned by the command whose job is to produce a working config from an existing one.

## Implications / actions

- `migrate` should carry every source setting with a direct counterpart, and name the ones it cannot (round 1 does both; `versionCommitMessage` → `commit-message` is still reported as having no counterpart — R3).
- `migrate` should derive `tag-format` from existing tags, or refuse at migrate time rather than at first release (R1).
- The parity check should not print "match" over an empty set (F3) or over a self-comparison (F4), and should look in `node_modules/.bin` before declaring the source tool unrunnable.
- `check` and `version` should say what they did (F7, F8). `check`'s silence is what let a broken config read as green.
- The generated bump-file README and the parser must move in the same commit on the quoted-name rule (F20).
- Install docs need one line for pnpm `allowBuilds`, and one for release-age gating on a same-day version (F15, F19).

## Open questions

- **Does `private-packages.tag` change tagging?** Unobservable through `status`, `check`, or `check --remote`; only `release` exercises it, and `release` requires a token before reaching the tagging path. Inferred from `config.rs:240`, not measured.
- **Does the `exclude`-wins precedence in round 1's refusal match intent?** A config with no `private-packages` *and* an `exclude` covering everything stays silent. Defensible — you excluded everything, so you decided — but it means the refusal cannot fire once anything empties the selection first.
- **`bumpy ci check --strict` in CI.** Outside GitHub Actions it reports "No bump files found in this PR" regardless of tree state, because it cannot resolve PR context. That the CI wrapper inherits bumpy's inability to read `.changeset/` is inferred, not measured.
- **oakum's skip list beyond the four sampled names.** `README`/`AGENTS`/`CLAUDE`/`GEMINI` and their case variants were measured against the 0.2.0 binary. Whether any other filename is skipped was not enumerated — the binary is native, so it could only be sampled.

## Raw data

**Migration state at the end.** `oakum check --strict` exits 0; `status --template summary` prints "No packages planned", which is correct — `review-cycle@0.17.0` points at `HEAD` and neither `.bumpy/` nor `.changeset/` holds a pending bump file. bumpy is deliberately left fully wired and working alongside oakum. Nothing was published, tagged, or pushed.

**Config that was actually shipped**, after hand-restoring what `migrate` dropped:

```toml
#:schema ./_schema.json
tool-version = "0.2.0"
change-files = true
conventional-commits = true

# `semver` (migrate's choice, not oakum's default) preserves bumpy's behavior:
# a major change file takes review-cycle 0.17.0 to 1.0.0, not 0.18.0.
versioning = "semver"

# Carried from bumpy's `versionCommitMessage`.
commit-message = "chore(release): version packages"

# Undecided: bumpy's `changelog` setting (github formatter, internalAuthors)
# has no counterpart here, so the default template runs and entries lose PR
# links and author attribution. Settle before the CI cutover.

# Every plugin is `private: true` — the package.json files are version anchors,
# not registry packages. Without both axes oakum plans and tags nothing.
[private-packages]
version = true
tag = true
```

**One thing the migration broke on the consumer side, unrelated to oakum but caused by adopting it.** The repository's PreToolUse commit gate scanned only `.bumpy/*.md`, so making `.changeset/` live meant a commit carrying a valid `oakum add` bump file was rejected with a message telling the developer to use bumpy. Fixing it took a pathspec widening, then narrowing by oakum's skip list, then `:(glob)` to stop `*` crossing `/`, plus six regression tests — every one of which was found by a reviewer measuring rather than reading. Worth noting in a rollout guide: **a repository that gates commits on bump-file presence has a gate to update, and it is not in oakum's remaining-steps list.**

**Method note.** Every destructive probe ran in a `git clone --no-hardlinks` in a scratch directory; the real checkout never had `oakum version`, `oakum release`, or `oakum ci version-pr` run against it. The tag probe used a local bare repository as `origin` with no token. One measurement in this report was invalid and is called out where it appears (F10): on a case-insensitive macOS volume, writing `.bumpy/readme.md` overwrites `.bumpy/README.md` rather than creating a distinct file, which silently destroyed a tracked file and produced a meaningless result. Restored from git and verified by tree hash.
