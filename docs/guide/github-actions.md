# Running oakum in GitHub Actions

Oakum does not write workflow files. You write them, or an agent writes them, and oakum verifies that they match what it expects ([ADR-0003](../decisions/0003-write-only-what-a-command-owns.md)).

This guide is derived from the workflows running on [oakoss/oakum](https://github.com/oakoss/oakum) through **v0.1.2** (2026-09-01): pull-request `check` and `ci pr-status` in [`.github/workflows/ci.yml`](https://github.com/oakoss/oakum/blob/main/.github/workflows/ci.yml), default-branch `ci version-pr` and `release` in [`.github/workflows/oakum.yml`](https://github.com/oakoss/oakum/blob/main/.github/workflows/oakum.yml), and cargo-dist reacting to the tag in the generated `release.yml`.

> **Fork pull requests.** When a run has no write permission, `ci pr-status` falls back to the job summary and logs why. That path is covered by integration tests in this repository; it has not yet been observed on a live fork pull request to oakum. See [Fork pull requests](#fork-pull-requests) below.

## Pin the version, and expect oakum to enforce it

Oakum's version determines bump math, changelog output, and what it writes to your manifests. If your workflow resolves the latest version on every run, your release behavior can change with no commit in your repository.

So `.changeset/_config.toml` declares an exact version:

```toml
#:schema ./_schema.json

tool-version = "0.1.2"
```

Every write command except `upgrade` refuses to run when the binary disagrees with it, in either direction. Your workflow must install that same version.

`oakum init` prints a workflow with the oakum version filled in and `actions/checkout` pinned to the latest GitHub release. Paste that rather than copying from this page, which goes stale. If the lookup fails, `init` writes nothing.

## The two-job shape

Version and release run in parallel on a default-branch push. The first maintains a version pull request. The second tags and creates GitHub releases.

`oakum release` shares `check`'s local preconditions, then tags, pushes, and creates a GitHub release one package at a time. After each tag it looks for a run whose `on:` listens for tags (`push.tags` or `create`). A repository with no tag-listening workflow is a completed look. The default `GITHUB_TOKEN` does not start a downstream workflow; use a GitHub App installation token if cargo-dist must react. `oakum init` also prints a `check` job that runs only on pull requests. Putting `check` on the same default-branch push as `release` fails while tags are still being written.

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
      - run: cargo binstall --no-confirm oakum@0.1.2
      - run: oakum check
      - run: oakum ci pr-status
        if: success() || failure()
        continue-on-error: true
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
      - run: cargo binstall --no-confirm oakum@0.1.2
      - run: oakum ci version-pr
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}

  release:
    if: github.event_name == 'push' && github.ref == format('refs/heads/{0}', github.event.repository.default_branch)
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1
        with:
          fetch-depth: 0
      - run: cargo binstall --no-confirm oakum@0.1.2
      - run: |
          git config user.name "github-actions[bot]"
          git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
      - run: oakum release
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

`fetch-depth: 0` is not optional. Tags are the record of what has been released, and a shallow clone does not have them.

### Measured on oakum (2026-09-01)

| Event | Workflow | Command | Run |
| --- | --- | --- | --- |
| PR #151 opened | CI | `mise run oakum -- check` | [33561077356](https://github.com/oakoss/oakum/actions/runs/33561077356) — exit 0, no stdout |
| PR #151 opened | CI | `mise run oakum -- ci pr-status` | same run — exit 0 with App token |
| #151 merged to `main` | oakum | `mise run oakum -- ci version-pr` | [33561801977](https://github.com/oakoss/oakum/actions/runs/33561801977) — opened version PR #152; release job printed `nothing to release` |
| #152 merged to `main` | oakum | `mise run oakum -- release` | [33562869917](https://github.com/oakoss/oakum/actions/runs/33562869917) — tagged **v0.1.2**, created GitHub Release |
| tag **v0.1.2** pushed | Release (cargo-dist) | build + publish | [33562952734](https://github.com/oakoss/oakum/actions/runs/33562952734) — npm, crates.io, homebrew all green |

The post-#152 oakum run's version job is a no-op once changesets are consumed; the row above is the push that actually opened #152.

## Pull-request check and plan

`oakum check` stays local and belongs on pull requests. Pass `--remote` when the newest local tags should also appear in `git ls-remote --tags`. Default lookback is three; `--remote-lookback` changes it. A mismatch is unverified. Leave `--remote` off in pull-request jobs.

`oakum ci pr-status` posts the sticky comment on the pull request and writes `$GITHUB_STEP_SUMMARY`. A token does not change `check`.

`run: oakum check` is an invocation, not a pin. `check` looks at **install sites**: a versioned `cargo binstall` / `cargo install` / `install-action` line in `.github/workflows`, an exact `oakum` entry in the root `package.json`, an exact `oakum` / `cargo:oakum` pin in `.mise.toml` or `mise.toml`, or a Cargo workspace member whose package name is `oakum` (self-host). Every site it finds must match `tool-version`.

### Self-hosting oakum

This repository cuts oakum with the **workspace binary**, not a crates.io or mise `[tools]` pin of a prior release ([ADR-0007](../decisions/0007-pin-the-tool-version-in-config.md)). Local dogfood:

```bash
mise run oakum -- check
mise run oakum -- status
```

That task runs `cargo run -q -p oakum --`. Do not add `oakum = "…"` under `[tools]` here; that would claim a registry install this tree does not use. `check` treats `crates/oakum`'s package version as the install pin when the member is named `oakum`.

Dogfood CI splits across two workflow files:

**Pull requests** — [`.github/workflows/ci.yml`](https://github.com/oakoss/oakum/blob/main/.github/workflows/ci.yml):

- `mise run check` (repo-wide lint gate)
- `mise run oakum -- check` on every pull request except `oakum/version-packages` (the version PR *is* the bump)
- `mise run oakum -- ci pr-status` with a GitHub App token so plan comments and version-PR authorship are the org bot, not `github-actions[bot]` (runs inside static-analysis, before CI Summary finishes aggregating other jobs)
- CI Summary gates merge readiness on static-analysis, tests, secret-scan, and audit — it does not gate pr-status, which may post while a later required job is still running

**Default-branch pushes** — [`.github/workflows/oakum.yml`](https://github.com/oakoss/oakum/blob/main/.github/workflows/oakum.yml):

- `$/.github/actions/harden-checkout` — harden-runner + checkout (no bundled mise setup)
- `$/.github/actions/app-token` — mint an org App installation token
- `$/.github/actions/setup` — mise toolchain
- `mise run oakum -- ci version-pr` and `mise run oakum -- release` both use the App token (`GITHUB_TOKEN` does not start cargo-dist on tag push)
- Release job re-checks out with `persist-credentials: true` and configures git identity from the App token before `oakum release`

The generated `release.yml` host job uploads into the release oakum already created.

Consumers keep the binstall / npm / mise `[tools]` shapes below.

### JavaScript: pin in `package.json`

An exact `devDependencies` entry, then `pnpm install` and `pnpm exec oakum`. Do not also `cargo binstall oakum@…` unless CI actually installs that way.

```json
{
  "devDependencies": {
    "oakum": "0.1.2"
  }
}
```

```yaml
- uses: pnpm/action-setup@v4
- run: pnpm install
- run: pnpm exec oakum ci version-pr
  env:
    GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

### mise: pin in `.mise.toml`

If CI already runs `mise install` (or `jdx/mise-action`), put the same exact version in `.mise.toml`. `check` reads `oakum` and `cargo:oakum` under `[tools]`. `latest` is not a pin.

```toml
[tools]
oakum = "0.1.2"
```

```yaml
- uses: jdx/mise-action@v2
- run: oakum ci version-pr
  env:
    GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

Table form is the same pin:

```toml
[tools]
"cargo:oakum" = { version = "0.1.2" }
```

## Verifying the install pin has not drifted

Because oakum does not own the install files, it checks them instead:

```bash
oakum check
```

This finds oakum install pins and compares them against `_config.toml`. It reports **matching**, **mismatched**, or **not found**, and treats not found as a failure. An install that `check` cannot recognize is the drift this is meant to catch.

Run it in CI on pull requests so drift surfaces before a release does.

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

When you bump the pinned version (workflow, `package.json`, or `.mise.toml`), CI will fail: the config still declares the old one. That is the gate working. Fix it in the same pull request:

```bash
oakum upgrade
```

This migrates the config, writes the new version, regenerates the schema, and reports what changed, all as one reviewable commit. If a migration fails it writes nothing.

Oakum never upgrades itself in CI. Doing so would turn a loud failure back into a silent behavior change.

## Tokens

The default `GITHUB_TOKEN` can open and update a version pull request through the GitHub API when wired that way, but sticky plan comments and version-PR authorship should use your **GitHub App installation token** instead of `github-actions[bot]`. Two cases need the App token (or another non-`GITHUB_TOKEN` actor):

- **Plan comments and version PR authorship** — mint an App token for `ci pr-status` on pull requests and for `ci version-pr` on default-branch pushes so timeline comments and the version PR author are your org bot. Oakum does this via `$/.github/actions/app-token`.
- **Downstream workflows on tag push** — events created with the repository's own `GITHUB_TOKEN` do not start new workflow runs. Give `oakum release` an App token if cargo-dist or another workflow must react to the tag. `oakum release` refuses a `workflow_dispatch`-only file before tagging.

## Fork pull requests

On a fork pull request, GitHub withholds write permission from the default token. When `pr-status = "comment"` (or `"both"`) and the token cannot post, oakum writes the plan to `$GITHUB_STEP_SUMMARY`, logs a line to stderr, and exits 0 so the gate still runs:

```text
comment requested but this run has no write permission (fork pull request); wrote the plan to the job summary instead.
```

Measured from `forbidden_comment_writes_summary_and_exits_zero` in `crates/oakum/tests/pr_status_cli.rs` (simulated 403 from the comments API). The exit code from `check` — not the comment — is what fails a fork pull request missing a change file ([ADR-0015](../decisions/0015-layer-the-pr-status-channels.md)).

## Publishing

`oakum release` stops at the tag and the GitHub release. Registry publishing is out of scope; cargo-dist owns artifacts. On this repository, `release.yml` uploads into the release oakum created. It creates a release only when `gh release view` reports the tag missing; other view failures fail the job. A later oakum publish path would target trusted publishing rather than tokens: npm revoked classic tokens in December 2025, and granular tokens expire every 90 days.
