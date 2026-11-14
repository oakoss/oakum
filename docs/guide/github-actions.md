# Running oakum in GitHub Actions

Oakum does not write workflow files. You write them, or an agent writes them, and oakum verifies that they match what it expects ([ADR-0003](../decisions/0003-write-only-what-a-command-owns.md)).

This guide is derived from the workflows running on [oakoss/oakum](https://github.com/oakoss/oakum): pull-request `check` and `ci pr-status` in [`.github/workflows/ci.yml`](https://github.com/oakoss/oakum/blob/main/.github/workflows/ci.yml), default-branch `ci version-pr` and `release` in [`.github/workflows/oakum.yml`](https://github.com/oakoss/oakum/blob/main/.github/workflows/oakum.yml). How that repository runs its own oakum, and the runs first measured, are in [Self-hosting oakum](../contributing/self-hosting.md).

> **Fork pull requests.** When a run has no write permission, `ci pr-status` falls back to the job summary and logs why. That path is covered by integration tests in this repository; it has not yet been observed on a live fork pull request to oakum. See [Fork pull requests](#fork-pull-requests) below.

## Pin the version, and expect oakum to enforce it

Oakum's version determines bump math, changelog output, and what it writes to your manifests. If your workflow resolves the latest version on every run, your release behavior can change with no commit in your repository.

So the place your repository installs oakum names an exact version, and that **install pin** is the version oakum enforces ([ADR-0038](../decisions/0038-the-install-pin-is-the-version.md)): a versioned install line in a workflow, an `@oakoss/oakum` devDependency in `package.json`, or a `[tools]` entry in `.mise.toml`. Nothing in `.changeset/_config.toml` repeats it.

The write commands (`add`, `generate`, `version`, `ci version-pr`, `release`, `upgrade`, `init`, `migrate`) refuse with exit `1` when the binary differs from the pin, naming the fix for that direction:

```text
error: this binary is `0.5.1` but the install pin is `0.6.0` (.mise.toml); install oakum `0.6.0`
```

`add`, `generate`, `version`, and `ci version-pr` exit `2` (unverified) when nothing pins oakum, when two pins disagree, or when a pin is a range or cannot be read. With no pin they name the cheapest one for your repository: `pnpm add -D --allow-build=@oakoss/oakum @oakoss/oakum@<version>` in an npm workspace, otherwise `"github:oakoss/oakum" = "<version>"` under `[tools]` in `.mise.toml`. `release` reports a missing pin through its install-pin precondition, as `check` does. `upgrade` continues without a pin and says so on stderr. `init` and `migrate` write their files and print the pin as the next step.

`oakum init` prints a workflow with every action pinned by commit SHA, with its tag as a trailing comment (`actions/checkout@<sha> # v7.0.1`), and, in a Cargo repository, the oakum version filled in. For a looked-up action, that is the commit its latest GitHub release tags, an annotated tag resolved to its commit. A tag can be moved to other code after you paste the workflow, and a commit cannot; a job holding a write token runs whatever the tag names on the day it runs, so the pin is the commit you saw, as in this repository's own workflows. Renovate raises such a pin and its tag comment together: this repository's Renovate pull requests (#322, under `helpers:pinGitHubActionDigestsToSemver`) move both. Paste the printed workflow rather than copying from this page, which goes stale. `oakum workflow` prints the same workflow alone and writes nothing, in a repository oakum already manages too: `mkdir -p .github/workflows && oakum workflow > .github/workflows/oakum.yml` saves it, and what it still needs, such as an install pin, goes to stderr. If a lookup fails, `init` writes nothing, and `oakum workflow` prints nothing on stdout. The shell empties a redirect's target before oakum runs, so refresh a saved workflow through another file: `oakum workflow > oakum.yml.new && mv oakum.yml.new .github/workflows/oakum.yml`.

## The two-job shape

Version and release run in parallel on a default-branch push. The first maintains a version pull request. The second tags and creates GitHub releases.

The version pull request body carries the release plan and, under **Release notes**, each package's changelog entry in a collapsible block, exactly as the branch writes it. An entry that would take the body past GitHub's 65,536-character limit, or that holds raw HTML which could break the page, is named with its `CHANGELOG.md` path instead.

Its title and its commit message are both `chore(release): version packages` unless the [`title`](configuration.md#title) and [`commit-message`](configuration.md#commit-message) keys set them, for instance when a commit linter's scope list has no `release`.

A `ci version-pr` run that versions anything writes one commit and force-updates the `oakum/version-packages` branch to it. A commit someone pushed to that branch is replaced by the next run, which is also how a version pull request someone edited by hand gets back to what oakum writes.

`oakum release` shares `check`'s local preconditions, then tags, pushes, and creates a GitHub release one package at a time. The release body is the package's `CHANGELOG.md` section for that version, read at the tagged commit, without the heading. When the changelog has no such section, or the section is empty, the body is the release title and `release` says so on stderr before creating the release. GitHub marks one release per repository Latest, which `releases/latest/download/…` URLs follow, so a release takes Latest only when no release holds it or the holder is an earlier version of the same package, and never as a prerelease; otherwise `release` says on stderr where Latest stays. oakum never moves Latest between packages, so in a repository with several, put it on the package whose downloads people follow once with `gh release edit <tag> --latest`, and each later release of that package keeps it. After each tag it looks for a run whose `on:` listens for tags (`push.tags` or `create`). A repository with no tag-listening workflow is a completed look. The look reads the commit's workflow runs, before the first tag as well as after each, which a private repository refuses a token without Actions read: give the release job `actions: read` beside `contents: write`, as the workflow `init` prints does, or `release` exits 2 before writing a tag and names the permission. A tag pushed with the default `GITHUB_TOKEN` does not start a downstream workflow; push with a GitHub App installation token if cargo-dist must react ([Tokens](#tokens)). `oakum init` also prints a `check` job that runs only on pull requests, and a `post` job after it that posts the plan comment ([Posting the comment from another job](#posting-the-comment-from-another-job)). Putting `check` on the same default-branch push as `release` fails while tags are still being written. A changelog that cannot be read at that commit stops the run before any tag is written.

Consumer repositories typically install a released binary:

```yaml
name: oakum

on:
  pull_request:
  push:

permissions:
  contents: read

jobs:
  check:
    if: github.event_name == 'pull_request'
    runs-on: ubuntu-latest
    permissions:
      contents: read
      pull-requests: write
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          fetch-depth: 0
          persist-credentials: false
      - uses: taiki-e/install-action@83ac0ad63c0167e6f06796fab0fce28db1bf3db0 # v2.87.22
        with:
          tool: oakum@0.1.2
      # Identity, not a branch name: a fork, a person, another bot, and a push
      # to the bot's branch each fail one of these terms and get checked. The
      # login is the version job's token's account; swap that token for an App's
      # and swap this to the App's `<slug>[bot]`.
      - run: oakum check --strict
        if: >-
          github.head_ref != 'oakum/version-packages'
          || github.event.pull_request.head.repo.full_name != github.repository
          || github.event.pull_request.user.login != 'github-actions[bot]'
          || github.event.sender.login != 'github-actions[bot]'
      - run: oakum ci pr-status --emit-comment oakum-comment
        id: pr-status
        if: success() || failure()
        continue-on-error: true
      # A step that failed must not read as one that reported. The `post` job
      # still marks the comment stale from what the step left.
      - run: echo "::warning title=oakum ci pr-status::the step failed, so its plan may not have reached the job summary or the pull request comment; the check above still decides"
        if: (success() || failure()) && steps.pr-status.outcome == 'failure'
      - uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1
        if: always()
        with:
          name: oakum-comment
          path: oakum-comment/

  # Holds the write token and runs no pull-request code: it checks out the
  # base commit, and the artifact is data. A fork's token cannot post, nor
  # can a Dependabot run's (read-only, with no Actions secrets), so both skip
  # it and keep the plan in the `check` job's summary.
  post:
    needs: check
    if: >-
      always()
      && github.event_name == 'pull_request'
      && github.event.pull_request.head.repo.full_name == github.repository
      && github.actor != 'dependabot[bot]'
    runs-on: ubuntu-latest
    permissions:
      contents: read
      pull-requests: write
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          ref: ${{ github.event.pull_request.base.sha }}
          persist-credentials: false
      - uses: taiki-e/install-action@83ac0ad63c0167e6f06796fab0fce28db1bf3db0 # v2.87.22
        with:
          tool: oakum@0.1.2
      - uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c # v8.0.1
        with:
          name: oakum-comment
          path: oakum-comment
      - run: oakum ci post-comment oakum-comment
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}

  version:
    if: github.event_name == 'push' && github.ref == format('refs/heads/{0}', github.event.repository.default_branch)
    runs-on: ubuntu-latest
    permissions:
      contents: write
      pull-requests: write
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          fetch-depth: 0
          persist-credentials: false
      - uses: taiki-e/install-action@83ac0ad63c0167e6f06796fab0fce28db1bf3db0 # v2.87.22
        with:
          tool: oakum@0.1.2
      - run: oakum ci version-pr
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}

  release:
    if: github.event_name == 'push' && github.ref == format('refs/heads/{0}', github.event.repository.default_branch)
    runs-on: ubuntu-latest
    permissions:
      contents: write
      actions: read
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          fetch-depth: 0
      - uses: taiki-e/install-action@83ac0ad63c0167e6f06796fab0fce28db1bf3db0 # v2.87.22
        with:
          tool: oakum@0.1.2
      # Tags oakum pushes carry this as their tagger; it matches this job's
      # secrets.GITHUB_TOKEN. Swap the token and swap this too. The version
      # commit is written through the GitHub API and carries the token's own
      # account, which no git config here can change.
      - run: |
          git config user.name "github-actions[bot]"
          git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
      - run: oakum release
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

`fetch-depth: 0` is not optional. Tags are the record of what has been released, and a shallow clone does not have them. A clone that fetches one branch (`git clone --single-branch`) misses the tags on the others, so `release` and `migrate`, which read every tag, report it as unverified too; `actions/checkout` at `fetch-depth: 0` keeps the refspec for every branch.

### Running the version job on demand

On every default-branch push that leaves bump files pending, the version job rewrites the version pull request, and each rewrite runs that pull request's CI again. oakoss/ui reported 12 of its 25 pull-request CI runs over one stretch as such rewrites. To rewrite it only when you ask, run the version job on `workflow_dispatch` and keep `release` on `push`:

```yaml
on:
  pull_request:
  push:
  workflow_dispatch:

jobs:
  version:
    if: github.event_name == 'workflow_dispatch' && github.ref == format('refs/heads/{0}', github.event.repository.default_branch)
    runs-on: ubuntu-latest
    permissions:
      contents: write
      pull-requests: write
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          fetch-depth: 0
      - uses: taiki-e/install-action@83ac0ad63c0167e6f06796fab0fce28db1bf3db0 # v2.87.22
        with:
          tool: oakum@0.1.2
      - run: oakum ci version-pr
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

The cost is that the version pull request is only as current as the last run. Bump files merged after it wait for the next dispatch, so merging the pull request releases what it held, not everything pending; run the job again before merging when that matters. `release` still runs on every push, so it tags whatever version commit lands.

### Concurrency, and why the printed workflow has none

The workflow above carries no `concurrency:` block. That is not a recommendation to omit one — it is that the question does not arise the same way here, and whether you add one depends on a property of the pipeline rather than on taste.

A release pipeline built on a plan/act split computes a decision in one job and hands it to another: plan reports a mode, act runs guarded by it. That decision can go stale between the two, which is what makes a cancellation window matter, and what a `--expect-mode` guard exists to close.

oakum has no plan step. `ci version-pr` and `release` each derive their own precondition from repository state when they run — pending bump files for one, an untagged manifest version for the other — so nothing computed earlier is carried forward to go stale. A dropped run is recovered by the push behind it: that checkout already contains the commit whose run was cancelled, and its `release` sees the same untagged version. A `ci version-pr` run that finds the default branch already moved past its checkout, to a commit descending from it, writes nothing and exits 0, leaving the version pull request to the newer push's run. That relies on the newer push starting this workflow: a `[skip ci]` commit, a push made with `GITHUB_TOKEN`, or one a `paths:` filter excludes starts none, and the bump files wait for the next push that does.

So per-command preconditions remove the hazard `--expect-mode` exists to guard: a decision cannot go stale between jobs, because no decision crosses a job boundary.

They do not serialize releases, and it is worth being exact about that. `release` evaluates readiness and then tags and pushes; two overlapping runs on the default branch can both evaluate the same version as untagged before either pushes. The second push of an existing tag ref fails, and preflight refuses a tag whose name collides or whose remote object points elsewhere — so the outcome is a failed job rather than a wrong release — but that is a narrower claim than "safe", and it is read from `cli/release/mod.rs` rather than measured under real concurrency.

A group is therefore reasonable if you want overlapping pushes serialized, and unnecessary if you are content for the later run to do the work. This repository's own `oakum` workflow carries one (`cancel-in-progress: false`); the workflow `init` prints does not.

One thing to know if you add one: a group with `cancel-in-progress: false` still lets GitHub cancel an older *pending* run when a newer one queues, so a middle push can be dropped. Under oakum that is recoverable — a run is only cancelled when a successor exists, and the successor re-derives the same preconditions. Under a mode-guarded pipeline it is not, and if the dropped push was the version-PR merge the release never happens and nothing says so.

## Pull-request check and plan

`oakum check` stays local and belongs on pull requests. Pass `--remote` when the newest local tags should also appear in `git ls-remote --tags`. Default lookback is three; `--remote-lookback` changes it. A mismatch is unverified. Leave `--remote` off in pull-request jobs.

`oakum ci pr-status` posts the sticky comment on the pull request and writes `$GITHUB_STEP_SUMMARY`; with `--emit-comment DIR`, as in the printed workflow, it writes the comment to `DIR` for a later job to post instead. A token does not change `check`. The comment opens with the commit it describes, the pull request's head commit, and links the run that wrote it (`Release plan for abc1234 · run #812`), so a plan an older run left, one cancelled or skipped by a path filter, reads as older.

`run: oakum check` and `run: pnpm exec oakum check` are invocations, not pins. `check` looks at **install sites**: a versioned `cargo binstall` / `cargo install` / `install-action` line or a versioned `npm i` / `npm install` / `npm add` / `pnpm add` / `pnpm install` / `pnpm dlx` / `npx @oakoss/oakum@x` line in `.github/workflows` or a local composite action under `.github/actions`, an exact `@oakoss/oakum` (or `oakum`) entry in the root `package.json`, an exact `github:oakoss/oakum` / `cargo:oakum` / `npm:@oakoss/oakum` / `oakum` pin in `.mise.toml` or `mise.toml`, or a Cargo workspace member whose package name is `oakum` (self-host). Every site it finds must name the same version; sites that disagree, and a bare `@oakoss/oakum` with no version, are `unverified`.

Workflow pins come from static YAML: `run:`, `tool:`, and `with.*` strings; `strategy.matrix` cell strings (including `include`); and `workflow_call` `inputs.*.default` install strings. A step that only runs `${{ matrix.install }}` (or another expression) is not a pin; put the install command in the matrix cell itself.

Besides the install-action shape above, oakum can be pinned in `package.json` or under `[tools]` in `.mise.toml`.

### JavaScript: pin in `package.json`

The published package is `@oakoss/oakum` (cargo-dist `npm-scope`). Add it with `pnpm add -D --allow-build=@oakoss/oakum @oakoss/oakum@<version>`, then `pnpm install` and `pnpm exec oakum` in CI. It is a fetcher: `postinstall` downloads the platform binary from the GitHub release, so install needs the npm registry and the release artifact host. pnpm 10 and later skip that script unless the package is approved, and `--allow-build` writes the approval (`allowBuilds`). Without it pnpm 11 and later fail `pnpm install` with `ERR_PNPM_IGNORED_BUILDS` (measured on 11.25.0 and 12.11.2); pnpm 10 only warns, and the first `pnpm exec oakum` fetches the binary inside the oakum step. Do not also `cargo binstall oakum@…` unless CI actually installs that way.

`check` reads that exact `@oakoss/oakum` entry as the install pin; a range or an `npm:` alias spec is `unverified`. The workflow `init` and `migrate` print for an npm workspace runs that entry: `pnpm install --frozen-lockfile --ignore-scripts`, then `pnpm exec oakum`, so the workflow names no version and the devDependency stays the one pin. Otherwise it installs with `taiki-e/install-action` (`tool: oakum@<version>`, the running binary's version), since `ubuntu-latest` has no `cargo-binstall` for a bare `cargo binstall` step to call.

In an npm workspace whose root `package.json` lists no `@oakoss/oakum`, `init` and `migrate` name the `pnpm add -D --allow-build=…` command above as a next step, whatever else pins oakum: `pnpm exec oakum` finds oakum in `node_modules/.bin` or on `PATH`, and the printed workflow puts it on neither without the dependency, so in a repository pinned through `.mise.toml` alone every job fails with `Command "oakum" not found` (measured on pnpm 12.16.1). Commit what the command changes: `package.json`, `pnpm-lock.yaml`, and, on pnpm 12, the `allowBuilds` entry it adds to `pnpm-workspace.yaml` (measured on 12.16.1, which created that file in a repository that had none). Without a committed `pnpm-lock.yaml`, `pnpm install --frozen-lockfile` fails with `ERR_PNPM_NO_LOCKFILE` and writes nothing, rather than leaving an untracked lockfile that would make `release` refuse a dirty tree. The pull request that commits the dependency with the workflow fails its `post` job once, since that job installs from the base branch, which has neither yet; its plan is still in the `check` job's summary.

`--ignore-scripts` is there because pnpm's default does not cover the repository itself. pnpm 10 and later skip a dependency's build scripts unless the package is approved, but a frozen install still runs the root `package.json`'s `postinstall` and `prepare`, and oakum's approved `postinstall` (measured on pnpm 10.34.5 and 12.16.1). With the flag nothing runs during the install, and oakum's launcher downloads the binary from the GitHub release on the first `pnpm exec oakum` in the job instead (measured on the same two versions with an empty store, on macOS arm64). The printed workflow makes that first run `pnpm exec oakum --version`, right after the install, so in the pull-request jobs, `check` and `post`, the download happens before any step holds a token.

That rule covers the pull-request jobs only. `version` and `release` run on the default branch and execute the repository's own code with the token: `release` installs after its checkout has persisted the push credential, and `--ignore-scripts` does not skip a `.pnpmfile.cjs`. pnpm loads its hooks on `pnpm install` and again on every `pnpm exec`, so each `pnpm exec oakum` step that holds `GITHUB_TOKEN` runs them with the token in their environment (measured on pnpm 12.16.1; reported on 11.25.0 too). In `post` the hooks are the base commit's, never the pull request's.

```json
{
  "devDependencies": {
    "@oakoss/oakum": "…"
  }
}
```

A repository that sets `minimumReleaseAge` in `pnpm-workspace.yaml` refuses an oakum release younger than that window, on the Renovate pull request that raises the pin. Exempt the package by name, without a version, so the entry outlives every bump:

```yaml
# pnpm-workspace.yaml
minimumReleaseAgeExclude:
  - '@oakoss/oakum'
```

That exempts every version of `@oakoss/oakum`, leaving the pin, which a person reviews on each bump, to decide which one installs. An entry with a version, such as `'@oakoss/oakum@0.9.2'`, exempts only that version and needs an edit on every bump. Measured on pnpm 10.34.5, 11.25.0 and 12.16.1; pnpm 10 checks the age only when it resolves a version, and 11 and 12 also on a frozen install ([pin upkeep across tools](../research/pin-upkeep-across-tools.md)).

```yaml
- uses: pnpm/action-setup@ea17c68df8912ef543352723c149a84f56e3d413 # v6.1.0
- run: pnpm install --frozen-lockfile --ignore-scripts
- run: pnpm exec oakum --version
- run: pnpm exec oakum ci version-pr
  env:
    GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

### mise: pin in `.mise.toml`

If CI already runs `mise install` (or `jdx/mise-action`), put an exact version in `.mise.toml`. `github:oakoss/oakum` installs the prebuilt release binary; `check` also reads `cargo:oakum`, `npm:@oakoss/oakum`, and `oakum` under `[tools]`. `latest` is not a pin.

```toml
[tools]
"github:oakoss/oakum" = "…"
```

```yaml
- uses: jdx/mise-action@6612f0ef07ed2a736cd1e00a94a065522dd41195 # v5.2.5
- run: oakum ci version-pr
  env:
    GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

Table form is the same pin:

```toml
[tools]
"cargo:oakum" = { version = "…" }
```

## Lockfiles

`version` and `ci version-pr` rewrite the lockfile entries a bump invalidates, so the version pull request passes a `--locked` build or a frozen install (`pnpm install --frozen-lockfile`, the default under `CI=true`). In `Cargo.lock` that is each bumped member's row. In `pnpm-lock.yaml` it is each importer specifier whose workspace range moved and each catalog entry whose range moved, where the entry links the workspace member. Nothing else in either file changes, and no package manager runs.

A `pnpm-lock.yaml` entry oakum cannot rewrite is left as it was and named, and the run still exits 0:

- the entry had already drifted from the manifest before the bump;
- the range resolves from the registry rather than to the workspace member;
- a catalog entry no importer records through the catalog;
- a value oakum cannot read;
- a `lockfileVersion` other than `9.0`, YAML anchors, or a file with more documents than pnpm's environment document and the lockfile, which leaves the whole file.

`version` names each entry on stderr. `ci version-pr` names them on stderr and in the version pull request body, because nobody reads a workflow's stderr. A frozen install fails until `pnpm install` updates those entries. A `ci version-pr` run that versions anything replaces the version branch, so fix an entry that had already drifted on the default branch, and push any other fix to the version branch after the last run.

A catalog pin that shares a YAML anchor with another entry is rewritten alone: an alias pin is written out as a quoted range, and the aliases of an anchored pin are written out with the old value. An entry that reaches the pin through an alias of a table holding it still moves with it. The rewrite names each of these entries the same way, on stderr and in the version pull request body.

A workspace with `shared-workspace-lockfile=false` keeps a lockfile per package; oakum does not rewrite those.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Ok. The command answered the question it was asked. |
| `2` | Unverified. The command could not make the check — a shallow or single-branch clone, an unreachable remote, a source tool it could not run. Nothing is claimed either way, including whether anything was written. **`clap` also exits 2 on a malformed command line**, before oakum runs at all; both mean the run did not answer your question, neither means a check failed. |
| `1` | Error. The command made the check and the answer is no, or it failed outright. |

A GitHub Actions step fails on any non-zero code, so **the default is already right** and most workflows need no change: `run: oakum check --strict` fails the job on both `1` and `2`.

Treat them differently only if you have decided that an unverified run should not block you, and read what that admits first. Exit `2` covers every outcome oakum could not settle — including a repository with no `.changeset/_config.toml` at all, a `check` whose tag look hit a shallow clone, and a `release` push that never answered when the re-check that would settle it failed too. A job that passes on `2` passes those.

```yaml
- id: check
  continue-on-error: true
  run: |
    oakum check --strict || echo "code=$?" >> "$GITHUB_OUTPUT"
- if: steps.check.outputs.code == '1'
  run: exit 1
```

A `run` step publishes nothing to `steps.<id>.outputs` on its own — that context is filled only by writes to `$GITHUB_OUTPUT` — so the code has to be captured deliberately. A step that sets `continue-on-error` and then tests an output nobody wrote never fails the job at all.

`unverified` is never silent: the reason is on stderr beside the code. See [ADR-0034](../decisions/0034-exit-two-for-unverified.md) for why the third outcome gets its own number rather than sharing `1`.

### A tool that never answers

Oakum's `git` commands, and the `cargo metadata` and `pnpm` calls that discover your packages, each run under a deadline of five minutes. One that has not answered by then is killed, and oakum names the tool and `OAKUM_REMOTE_DEADLINE` on stderr, so a FIFO, a lock, or a credential helper or signing program waiting on a prompt nobody can see ends the job instead of hanging it. A call that never answers settles nothing, so the command exits `2` (unverified). `release` re-checks a tag or push that never answered (the tag locally, the push on the remote). Once the re-check settles what happened, it stops with exit `1` and names the stage; if the re-check fails too, it stays `2`. `status` instead prints an unverified note when its coverage look timed out and still renders its report at exit `0`. What the killed process started and is still waiting on, such as the helper git ran, is killed with it and named; a process whose parent had already exited is not reached, and one oakum could not list or kill is named as possibly still running. The `bumpy`, `changeset` and `knope` calls `oakum migrate` makes to read another release tool's plan run under the same deadline; one that times out is named on stdout, and `migrate` compares against its own simulation and exits `2`.

Set `OAKUM_REMOTE_DEADLINE` to a whole number of seconds to change it: higher when a first run legitimately takes longer, such as `cargo` installing a pinned Rust toolchain; lower to fail well inside the job's own `timeout-minutes`. Oakum refuses a value that is not a positive whole number, naming the variable, rather than falling back to the default.

```yaml
env:
  OAKUM_REMOTE_DEADLINE: "600"
```

## Verifying the install pin has not drifted

Because oakum does not own the install files, it checks them instead:

```bash
oakum check
```

This finds oakum install pins and compares them with each other. It reports `unverified` when pins **disagree** or **none is found**, naming the reason on stderr; pins that **agree** print nothing and exit `0`. When the pins name the running binary, it also compares `.changeset/_schema.json` with the schema that binary generates and reports a stale one, or a missing one that the `#:schema` line names, as a note naming `oakum upgrade`, without changing the exit code, and notes a `.changeset/README.md` an earlier oakum wrote the same way; when they name another version, the note says the schema was not compared. The two-line report `check` opens with names the looks it is about to make, never their verdict. An install that `check` cannot recognize is the drift this is meant to catch. With no `_config.toml` at all there is no oakum setup to check, so `check`, `release`, and the writers (`add`, `generate`, `version`, `ci version-pr`) exit `unverified` and name `oakum init` and `oakum migrate`.

Run it in CI on pull requests so drift surfaces before a release does. The printed workflow runs `check --strict`, which also fails when a changed package has no covering bump file; drop `--strict` only if you want that reported without failing the job. A change that should release nothing, such as a metadata-only edit to a manifest, is covered by a bump file too: `oakum add --none --packages <name>:none`, or `oakum add --empty` for every changed package ([What covers a change](coverage.md#what-covers-it), [Writing bump files](bump-files.md#creating-one)).

`status` prints to stdout. GitHub's output file is a `name<<delimiter` protocol, not a dump of the process. Wire it in the workflow:

```yaml
- id: plan
  run: |
    {
      echo 'json<<EOF'
      oakum status --json
      echo EOF
    } >> "$GITHUB_OUTPUT"
```

## Upgrading

Raising the install pin is the upgrade: a Renovate or Dependabot pull request that bumps your only pin goes green on its own. When `check` notes that `.changeset/_schema.json` or `.changeset/README.md` is out of date, or a command warns on stderr that `.changeset/_config.toml` still sets the deprecated `tool-version`, run `oakum upgrade` through the new pin (`pnpm exec oakum upgrade`, or mise's shim) and commit the result:

```text
migrations: removed tool-version
schema: .changeset/_schema.json regenerated
```

`upgrade` runs config migrations, regenerates the schema (or writes none, when `_schema.json` is absent and the `#:schema` line names another one, as an opt-in to the npm package's copy does; see the [configuration reference](configuration.md)), and refreshes a `.changeset/README.md` an earlier oakum wrote, leaving a README of your own alone; it writes no version. A binary that differs from the pin refuses, like every write command. If a migration fails it writes nothing.

Oakum never upgrades itself in CI. Doing so would turn a loud failure back into a silent behavior change.

### Two pins, one pull request

A repository that pins oakum in two places, such as a `package.json` devDependency and a `.mise.toml` entry, gets one Renovate pull request per pin, and each fails `check` alone because the pins then disagree. Group them, naming the package as each source spells it: `@oakoss/oakum` for npm and mise's `npm:` backend, `oakum` for the crate and mise's `cargo:` backend, `oakoss/oakum` for mise's `github:` backend.

```json
{
  "packageRules": [
    {
      "matchPackageNames": ["@oakoss/oakum", "oakum", "oakoss/oakum"],
      "groupName": "oakum"
    }
  ]
}
```

Renovate does not read a workflow install line at all: neither `tool: oakum@…` under `taiki-e/install-action` nor `npm i -g @oakoss/oakum@…`. Beside a second pin, such a line is never raised and every Renovate pull request for the other pin fails `check`. Drop the second pin, or make the line a dependency with a regex custom manager whose name the rule above matches:

```json
{
  "customManagers": [
    {
      "customType": "regex",
      "managerFilePatterns": ["/(^|/)\\.github/workflows/[^/]+\\.ya?ml$/"],
      "matchStrings": ["npm i(?:nstall)? -g @oakoss/oakum@(?<currentValue>[^\\s]+)"],
      "depNameTemplate": "@oakoss/oakum",
      "datasourceTemplate": "npm"
    },
    {
      "customType": "regex",
      "managerFilePatterns": ["/(^|/)\\.github/workflows/[^/]+\\.ya?ml$/"],
      "matchStrings": ["tool: oakum@(?<currentValue>[^\\s]+)"],
      "depNameTemplate": "oakum",
      "datasourceTemplate": "crate",
      "versioningTemplate": "semver"
    }
  ]
}
```

Without `versioningTemplate: "semver"` the crate lookup proposes nothing (inferred cause: it reads `0.9.2` as a Cargo range that `0.9.3` already satisfies). Measured with Renovate 44.181.7 in a lookup-only dry run, which plans branches but writes no files; the `github:` backend's lookup was not run ([pin upkeep across tools](../research/pin-upkeep-across-tools.md)).

## Tokens

The default `GITHUB_TOKEN` can open and update a version pull request through the GitHub API when wired that way, but sticky plan comments and version-PR authorship should use your **GitHub App installation token** instead of `github-actions[bot]`. Three cases need the App token (or another non-`GITHUB_TOKEN` actor):

- **Plan comments and version PR authorship** — mint an App token for `ci pr-status` on pull requests and for `ci version-pr` on default-branch pushes so timeline comments and the version PR author are your org bot. Oakum does this via `$/.github/actions/app-token`. Name the App's `<slug>[bot]` login in the `oakum check --strict` skip in place of `github-actions[bot]`; until then the check runs on the version pull request, and `ci version-pr` says so on stderr.
- **Downstream workflows on tag push** — events created with the repository's own `GITHUB_TOKEN` do not start new workflow runs. Push the tags with an App token if cargo-dist or another workflow must react to them (the first wiring below). A `workflow_dispatch`-only file listens for no tag: when no workflow listens for tags, `oakum release` names it and tags anyway, so dispatch it with the new tag if it builds the release. The workflow dist generates for `dispatch-releases = true` creates the tag and release itself, so `oakum release` refuses before tagging when it finds one: a workflow with `workflow_dispatch` whose first line is dist's `# This file was autogenerated by dist:` header and which runs `gh release create` in this repository. With dist's `create-release = false` it uploads to the release oakum creates instead, and with `github-releases-repo` naming another repository it creates its release there, so nothing refuses; dispatch it with the tag. One whose header was edited away is not recognized, nor one from dist before 0.21.1, which wrote no header; regenerate it with a current `dist init`.
- **Bot attribution on tags and releases** — so the tags and GitHub releases read as your App's bot rather than `github-actions[bot]`.

### Asking `init` for another token

`oakum init`, `oakum migrate`, and `oakum workflow` print a workflow wired to `secrets.GITHUB_TOKEN` unless a flag names another token. The flags change only the printed workflow; they cannot be combined, and `init` and `migrate` on a repository oakum already manages, which get no workflow, refuse them with exit `1` and name `oakum workflow`, which takes them.

- `--token <SECRET>` wires each writing step (`ci post-comment`, `ci version-pr`, `release`) to `${{ secrets.<SECRET> || github.token }}`, and checks the `release` job out with the same token and `persist-credentials: true`, so the tags are pushed with it. The fallback keeps a fork's run working: a fork receives none of the repository's secrets, and an absent one is empty. `--token GITHUB_TOKEN` prints the default workflow. `init` then names what it cannot do for you: store the secret under that name, give it Actions read on a private repository, and put its account in the `release` job's `git config`.
- `--app` mints a GitHub App token with `actions/create-github-app-token`, pinned like every other action, in `post`, `version`, and `release`, each asking for the permissions in the table under [Fork pull requests](#fork-pull-requests), with the same `|| github.token` fallback. `post` and `version` mint after the install and the binary fetch; `release` mints first, since its checkout pushes the tags with the token, and sets the tagger to the App's bot. `post` already skips a fork and Dependabot. The mint reads the `APP_CLIENT_ID` and `APP_PRIVATE_KEY` secrets; store them as Dependabot secrets too and you can drop `post`'s `github.actor != 'dependabot[bot]'` clause, since the App token, unlike a Dependabot run's own, can post ([Dependency bot pull requests](#dependency-bot-pull-requests)). `init` cannot check that the App is installed or that the two secrets are stored, so it names both as a next step.

Under either flag the printed `check --strict` skip matches the version pull request by its branch, `oakum/version-packages`, and this repository, without the default's two `github-actions[bot]` login terms. `init` cannot know a secret's account or an App's slug, and a skip naming the wrong login would run the check on every version pull request. The narrower skip trusts anyone who can push that branch in this repository, who can edit the workflow anyway.

oakum reads its token from `GITHUB_TOKEN`, falling back to `GH_TOKEN` for `release` and `ci version-pr` (`ci pr-status` and `ci post-comment` read `GITHUB_TOKEN` only), and uses it for the GitHub API alone. `release` creates each tag with `git tag -m` and pushes it with a plain `git push`, handing git no credential and no identity. So the App token reaches the release in three separate places, and setting it on the `oakum release` step alone moves only the release's author:

1. **The push** — check out with `persist-credentials: true` and `token:` set to the App token, so the credential git pushes the tag with is the App's. This is what lets a tag-listening workflow start.
2. **The tagger** — an annotated tag records git's configured identity. Set `user.name` and `user.email` to the App's bot: `<slug>[bot]` and `<user id>+<slug>[bot]@users.noreply.github.com`, where the user id is `gh api "/users/<slug>%5Bbot%5D" --jq .id`.
3. **The release author** — `GITHUB_TOKEN` in the `oakum release` step's `env:` is the token the GitHub release is created with.

This repository's `release` job wires all three through its `app-token` action (`resolve-identity: true` outputs the user and email), and its tags and releases read `oakoss[bot]`: `git for-each-ref` reports that tagger on `v0.3.1` and `v0.9.0` through `v0.9.3`, and the releases API that author on `v0.8.3` through `v0.9.3` (both read 2026-11-09). With the public action the job looks like this:

```yaml
release:
  if: github.event_name == 'push' && github.ref == format('refs/heads/{0}', github.event.repository.default_branch)
  runs-on: ubuntu-latest
  permissions:
    contents: write
    actions: read
  steps:
    - name: Mint an App token
      id: app-token
      uses: actions/create-github-app-token@64ab729a6294000f5aa4a4bc0b73d1f32bc45510 # v3.3.0
      with:
        client-id: ${{ secrets.APP_CLIENT_ID }}
        private-key: ${{ secrets.APP_PRIVATE_KEY }}
        permission-contents: write
        permission-actions: read
    - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
      with:
        fetch-depth: 0
        persist-credentials: true
        token: ${{ steps.app-token.outputs.token }}
    - uses: taiki-e/install-action@83ac0ad63c0167e6f06796fab0fce28db1bf3db0 # v2.87.22
      with:
        tool: oakum@0.1.2
    - name: Tag as the App
      env:
        GH_TOKEN: ${{ steps.app-token.outputs.token }}
        SLUG: ${{ steps.app-token.outputs.app-slug }}
      run: |
        id="$(gh api "/users/${SLUG}%5Bbot%5D" --jq .id)"
        git config user.name "${SLUG}[bot]"
        git config user.email "${id}+${SLUG}[bot]@users.noreply.github.com"
    - run: oakum release
      env:
        GITHUB_TOKEN: ${{ steps.app-token.outputs.token }}
```

## Fork pull requests

On a fork pull request, GitHub withholds write permission from the default token. When `pr-status = "comment"` (or `"both"`) and the token cannot post, oakum writes the plan to `$GITHUB_STEP_SUMMARY`, logs a line to stderr, and exits 0 so the gate still runs:

```text
comment requested but this run has no write permission (fork pull request); wrote the plan to the job summary instead.
```

Measured from `forbidden_comment_writes_summary_and_exits_zero` in `crates/oakum/tests/it/pr_status_cli.rs` (simulated 403 from the comments API). The exit code from `check` — not the comment — is what fails a fork pull request missing a change file ([ADR-0015](../decisions/0015-layer-the-pr-status-channels.md)).

The workflow has to let oakum reach that fallback, and the default one does. `secrets.GITHUB_TOKEN` is handed to every run, read-only on a fork, so a single-job workflow needs nothing here: `pr-status` gets a token, cannot post with it, and writes the summary. The workflow `init` prints splits the comment instead: its `check` job holds no write token and `pr-status --emit-comment` writes the summary on every run, and its `post` job skips a fork ([Posting the comment from another job](#posting-the-comment-from-another-job)), so a fork sees the plan in the summary. It skips Dependabot's pull requests the same way (`github.actor != 'dependabot[bot]'`): a run Dependabot starts gets a read-only `GITHUB_TOKEN` and no Actions secrets, so `post-comment` would exit `2` on every one ([Dependency bot pull requests](#dependency-bot-pull-requests)). Under `--app`, storing the App's two secrets as Dependabot secrets as well lets you drop that clause.

A stronger token needs some care, because a fork pull request receives none of your own secrets. GitHub's own words: "With the exception of `GITHUB_TOKEN`, secrets are not passed to the runner when a workflow is triggered from a forked repository." A personal access token and a GitHub App's credentials are withheld alike.

With a PAT, the value is simply empty on a fork, so give the step the default token to fall back to:

```yaml
- run: oakum ci pr-status
  env:
    GITHUB_TOKEN: ${{ secrets.YOUR_PAT || github.token }}
```

With an App token there is a step in between, and an empty input fails it rather than yielding an empty value — so the step itself has to be skipped, and the same fallback then covers what it no longer supplies:

```yaml
- name: Mint an App token
  id: app-token
  if: >-
    (success() || failure())
    && github.event_name == 'pull_request'
    && github.event.pull_request.head.repo.full_name == github.repository
  uses: actions/create-github-app-token@64ab729a6294000f5aa4a4bc0b73d1f32bc45510 # v3.3.0
  with:
    client-id: ${{ secrets.APP_CLIENT_ID }}
    private-key: ${{ secrets.APP_PRIVATE_KEY }}
    permission-contents: read
    permission-pull-requests: write

- run: oakum ci pr-status
  if: success() || failure()
  env:
    GITHUB_TOKEN: ${{ steps.app-token.outputs.token || github.token }}
```

The guard is the part that matters: without it the mint fails and fails the job with it, before oakum runs at all. The fallback is a refinement — `pr-status` degrades to the job summary with no token whatsoever, pinned by `missing_token_degrades_to_summary` in `crates/oakum/tests/it/pr_status_cli.rs` — so what it buys is the read-only default, which lets the run read the pull request and take the no-write-permission path above rather than the unset-token one. This repository's own `ci.yml` uses the second shape.

`success() || failure()` matters when the mint sits after `oakum check --strict`, as it does when it is placed beside `pr-status`: a failed check would otherwise skip the mint, and `pr-status` would post as `github-actions[bot]` through the fallback.

Scope the token to what the job does. With no `permission-<name>` input, the action's token "inherits all of the installation's permissions"; with neither `owner` nor `repositories`, "access will be scoped to only the current repository", and `owner` alone widens it to every repository in that owner's installation ([github-app-token-scope.md](../research/github-app-token-scope.md)). This repository's own `app-token` action refuses a mint with no `permission-*` input, and each job asks for its own:

| Job | Runs | Token permissions |
| --- | --- | --- |
| pull request | `ci pr-status` | `permission-contents: read`, `permission-pull-requests: write` |
| version | `ci version-pr` | `permission-contents: write`, `permission-pull-requests: write` |
| release | `release` | `permission-contents: write` |

This repository is public, where reading workflow runs needs no Actions permission. On a private repository `release` needs `permission-actions: read` as well, which the workflow `init` prints asks for.

A job that holds the token and installs oakum through `package.json` (`pnpm install`, then `pnpm exec oakum`) runs the install after `actions/checkout` has persisted the checkout's token for git (since checkout v6, in a file under `$RUNNER_TEMP` that the repository's git config points to), so every later process in the job, install scripts included, can run authenticated git commands with it. A job that does not push needs no persisted credential: check out with `persist-credentials: false` and pass the token to oakum through `env:` alone, as the workflow `init` prints does for every job but `release`. `release` pushes, so its checkout persists the token; there, install with `pnpm install --frozen-lockfile --ignore-scripts`, which runs no install script, the repository's own included, or by a route that runs none, such as `taiki-e/install-action`. Neither keeps the repository's own `.pnpmfile.cjs` out of a `pnpm` call in that job ([JavaScript: pin in `package.json`](#javascript-pin-in-packagejson)).

## Dependency bot pull requests

Under `check --strict`, a pull request from Renovate or Dependabot that changes a package oakum versions needs covering intent like any other, and a bot cannot write a bump file. What its commit message says decides the outcome. Measured on one change to a Cargo package, with `change-files` and `conventional-commits` both on:

| Bot commit | `check --strict` | `oakum generate` writes |
| --- | --- | --- |
| `Bump serde from 1.0.1 to 1.0.2` | exits 1: not conventional, so it asks for a release | `demo: patch` |
| `chore(deps): update serde to 1.0.2` | exits 0: covered by a commit that releases nothing | `demo: none` |

**Most repositories: give the bot a commit type that releases nothing.** With both modes on, a `chore(deps):` commit covers the change and releases nothing, so the pull request passes and can automerge. The update ships with your next release.

- Renovate: add `:semanticCommits` and `:semanticCommitTypeAll(chore)` to `extends`, after `config:recommended`. That preset alone types updates to production dependencies `fix` (through `:semanticPrefixFixDepsChoreOthers`), which asks for a patch, and without `:semanticCommits` Renovate uses semantic prefixes only when the repository's history already does.
- Dependabot: set `commit-message: { prefix: "chore", include: "scope" }` on each `updates` entry in `dependabot.yml`, which writes `chore(deps): …`, or `chore(deps-dev): …` for development dependencies. Its default message follows the patterns in the repository's history, so it is not always conventional.

**When a bot's update should release.** Some updates change what users get: a dependency you re-export, or a pinned version in output your package prints. Leave those on a release-nothing type and they wait for an unrelated release. Two ways to release them:

- Have a human add the bump file on the pull request. This repository does that for its install-action pin: a Renovate rule gives it its own pull request with automerge off and a note naming the `oakum add` command (`.github/renovate.json`).
- Keep the bot's releasing type (`fix(deps)`) and write the bump file in CI. A workflow on the bot's pull requests checks out the pull request's branch (`ref: ${{ github.head_ref }}`, since the default merge ref cannot be pushed) with `fetch-depth: 0`, runs `oakum generate --from "origin/$BASE"`, with `BASE: ${{ github.base_ref }}` set under the step's `env:` rather than written into the command, following GitHub's script-injection guidance (`generate` needs both `change-files` and `conventional-commits`; without `--from` it finds only `main` or `master`), and commits and pushes the file it writes. Guard the step so it runs only while the branch adds no file under `.changeset/` against that same base: `generate` writes a new file on every run, and the push starts the workflow again. Push with a GitHub App token or a personal access token: a push made with the repository's own `GITHUB_TOKEN` leaves the pull request's checks waiting until someone with write access approves them. Bot pull requests come from branches in your repository, so `pull_request` is enough; do not use `pull_request_target` for this, since it would run with your secrets and GitHub warns against checking out a pull request's code under it. A workflow Dependabot triggers gets a read-only `GITHUB_TOKEN` by default and only Dependabot secrets, not Actions secrets, so store the App's credentials as Dependabot secrets. A push from another author freezes the bot's pull request by default: Renovate stops committing to it until the App's commit email is in `gitIgnoredAuthors`, and Dependabot stops rebasing it unless the commit message carries `[dependabot skip]`. When the bot then pushes over the bump file, the guard lets the next run write it again. Renovate's `postUpgradeTasks` is no substitute: its commands run before Renovate commits, when `generate` has no commit to read, and they are blocked by default on the hosted app.

The sources are in [dependency bot commits](../research/dependency-bot-commits.md).

## Posting the comment from another job

To keep the write token out of the job that runs pull-request code, split the comment across two jobs in the same workflow; the workflow `oakum init` and `oakum migrate` print does this, as `check` and `post`. The job that checks out the pull request runs `oakum ci pr-status --emit-comment DIR` and uploads `DIR` as an artifact; a later job that `needs` it holds the token, downloads the artifact, and runs `oakum ci post-comment DIR`. That job reads no config, but it runs oakum, so it checks out the base branch to install the version the project pins ([The posting job](#the-posting-job)). Give the upload step and the posting job `if: always()`: a run that cannot build the plan fails, and without it the artifact that says so never reaches the poster.

`--emit-comment` always leaves exactly one file in `DIR`, written whole: `oakum-pr-comment.md` with the body, `oakum-pr-comment.none` when the run has no comment, or `oakum-pr-comment.failed` with the reason when the run could not build the plan. A run that stops before it reads the config, or before it starts, leaves none. Beside it, `oakum-pr-comment.format` records the artifact's format and the oakum that wrote it. The posting job can run a different oakum from the emitting one, as on a pull request that upgrades oakum, where it runs the base branch's: a poster older than the artifact exits `2` naming both versions and touches nothing, and the comment updates again once the upgrade merges. A directory without the record is read as format 1, which is what oakum before 0.9.0 emits. `post-comment` posts or updates oakum's plan comment for the first, removes it for the second, and for the third marks it stale (below), owning only the comments `pr-status` itself would. It exits `2` (unverified) without touching the pull request when the artifact cannot be trusted whole: none of the three or more than one, a body or reason cut short, or an entry that is not a regular file. A lost artifact never reads as "nothing to post". A body that does not start with oakum's marker, or is over GitHub's 65,536-character limit, is refused with exit `1`. The plan is posted before any older copy is removed; when a removal then fails, `post-comment` exits `2` (unverified) saying the plan was posted and which removal failed, and `pr-status` says the same on stderr, without the `unverified:` prefix, and exits `0`.

When a run cannot build the plan (a shallow clone, unreadable tags, a coverage look that could not run, a selection naming no package), `pr-status` still fails with the exit code it always did, and oakum's comment is marked stale rather than left reading as current: a warning naming the commit and run it could not check, with the reason (for a shallow clone, the `fetch-depth: 0` fix), above the last plan it did check, collapsed. A second failure keeps that same plan; with no earlier plan the warning stands alone. Without `--emit-comment`, `pr-status` posts this itself; in the split, `post-comment` composes it from `oakum-pr-comment.failed`, naming the commit and run from its own event, and exits `0`, since the emitting job has already failed; it exits `2` when an older copy of the comment could not be removed. With `pr-status = "summary"` a failed run writes `oakum-pr-comment.none`, as a run that succeeds does. A poster from oakum before 0.9.0 reads that artifact as holding neither file and exits `2` without touching the pull request.

The pull request number comes from the posting job's own event (`pull_request`, or a `workflow_run` started by a same-repository pull request), never from the artifact. In the same-workflow `pull_request` split, a fork's posting job gets a read-only token and `post-comment` exits `2` saying so; skip that job on forks with `if: github.event.pull_request.head.repo.full_name == github.repository`. A `workflow_run` poster needs no such guard: a fork's run names no pull request, and `post-comment` refuses with exit `1`. Either way a fork sees the plan only in the emitting job's summary, which `pr-status = "both"` (the default) or `"summary"` writes; under `"comment"` a fork gets neither.

### The posting job

The posting job checks out the pull request's base commit, installs oakum the way the project pins it, and only then mints the write token and posts. With the pin in `package.json`:

```yaml
name: oakum plan

on:
  pull_request:

permissions:
  contents: read

jobs:
  plan:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          fetch-depth: 0
          persist-credentials: false
      - uses: pnpm/action-setup@ea17c68df8912ef543352723c149a84f56e3d413 # v6.1.0
      - run: pnpm install --frozen-lockfile --ignore-scripts
      # `pnpm exec oakum check --strict`, guarded as in the workflow above, goes here.
      - run: pnpm exec oakum ci pr-status --emit-comment oakum-comment
        if: success() || failure()
      - uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.1
        if: always()
        with:
          name: oakum-comment
          path: oakum-comment/

  post:
    needs: plan
    if: always() && github.event.pull_request.head.repo.full_name == github.repository
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          ref: ${{ github.event.pull_request.base.sha }}
          persist-credentials: false
      - uses: pnpm/action-setup@ea17c68df8912ef543352723c149a84f56e3d413 # v6.1.0
      - run: pnpm install --frozen-lockfile --ignore-scripts
      - run: pnpm exec oakum --version
      - uses: actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c # v8.0.1
        with:
          name: oakum-comment
          path: oakum-comment
      - name: Mint an App token
        id: app-token
        uses: actions/create-github-app-token@64ab729a6294000f5aa4a4bc0b73d1f32bc45510 # v3.3.0
        with:
          client-id: ${{ secrets.APP_CLIENT_ID }}
          private-key: ${{ secrets.APP_PRIVATE_KEY }}
          permission-pull-requests: write
      - run: pnpm exec oakum ci post-comment oakum-comment
        env:
          GITHUB_TOKEN: ${{ steps.app-token.outputs.token }}
```

With the pin in `.mise.toml`, the posting job runs `jdx/mise-action` after the base checkout in place of the pnpm steps, and `oakum ci post-comment` in place of `pnpm exec`. With `secrets.GITHUB_TOKEN` in place of an App token, drop the mint and give the job `pull-requests: write`, as the workflow `init` prints does. That workflow's Cargo shape pins oakum in the workflow file itself (`tool: oakum@<version>`), which a pull request's run reads from the pull request's merge commit, so there the first point below does not hold: the poster installs the version the pull request's workflow names.

Three things follow from the order:

- **The base branch's pin decides which oakum holds the token.** The install reads the base commit's `package.json` and lockfile, or its `.mise.toml`, so a pull request that raises or edits the pin changes what its emitting job runs, never its poster. The version stays in one place: no second version line in the workflow for `check` to compare, and no variable install line such as `npx @oakoss/oakum@$V`, which `check` reports as a pin that is not an exact version and exits `2`.
- **No pull-request code runs in the job.** The checkout is the base commit; the artifact is data, which `post-comment` refuses unless it is exactly one whole file oakum wrote.
- **No token exists during the install.** The checkout persists no credential, the job's own token has only `contents: read`, and the App token is minted after the install. `--ignore-scripts` runs no install script, the root `package.json`'s included, and `pnpm exec oakum --version` then fetches oakum's binary before the mint. The posting step's `pnpm exec` does load the base commit's `.pnpmfile.cjs` with the token in its environment ([JavaScript: pin in `package.json`](#javascript-pin-in-packagejson)).

On a pull request that upgrades oakum, the emitting job runs the new version and the poster the base branch's; when the emit format changed between them, the poster exits `2` naming both versions, as above, and the comment updates once the upgrade merges. Keep the posting job out of required checks so that pull request can merge.

### Replacing a hand-written poster

A repository that posts the emitted body with its own script, looking up its comment by a marker and creating or updating it through the API, can hand that job to oakum:

1. Replace the script with the base checkout, the install, and `oakum ci post-comment DIR` above, pointed at the same artifact. `post-comment` takes over a comment the script posted when a bot account posted it and its body starts with oakum's marker, as the emitted body does: it updates the newest such comment and removes the rest. A comment the script wrapped in a marker of its own is not oakum's, and stays until someone deletes it.
2. Remove the version line the poster used to install oakum. It was a second pin, and every oakum upgrade had to raise it too.
3. Keep `if: always()` on the upload step and the posting job. `post-comment` then also handles what a script usually does not: `oakum-pr-comment.none` removes a stale plan, and `oakum-pr-comment.failed` marks the last plan stale.

## Publishing

`oakum release` stops at the tag and the GitHub release. Registry publishing is out of scope; cargo-dist owns artifacts. On this repository, `release.yml` uploads into the release oakum created. It creates a release only when `gh release view` reports the tag missing; other view failures fail the job. A later oakum publish path would target trusted publishing rather than tokens: npm revoked classic tokens in December 2025, and granular tokens expire every 90 days.
