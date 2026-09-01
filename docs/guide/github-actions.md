# Running oakum in GitHub Actions

> Oakum is pre-release. `oakum ci version-pr` and `oakum release` are shipped. `oakum release` refuses a `workflow_dispatch`-only file before tagging, prints the matching run URL after each tag, and reports unverified if a look does not finish.

Oakum does not write workflow files. You write them, or an agent writes them, and oakum verifies that they match what it expects ([ADR-0003](../decisions/0003-write-only-what-a-command-owns.md)).

## Pin the version, and expect oakum to enforce it

Oakum's version determines bump math, changelog output, and what it writes to your manifests. If your workflow resolves the latest version on every run, your release behavior can change with no commit in your repository.

So `.changeset/_config.toml` declares an exact version:

```toml
#:schema ./_schema.json

tool-version = "0.1.0"
```

and every write command except `upgrade` refuses to run when the binary disagrees with it, in either direction. Your workflow must install that same version.

`oakum init` prints a workflow with the oakum version filled in and `actions/checkout` pinned to the latest GitHub release. Paste that rather than copying from this page, which goes stale. If the lookup fails, `init` writes nothing.

## The two-job shape

Version and release run in parallel on a default-branch push. The first maintains a version pull request. The second tags and creates GitHub releases.

`oakum release` shares `check`'s local preconditions, then tags, pushes, and creates a GitHub release one package at a time. After each tag it looks for a run whose `on:` listens for tags (`push.tags` or `create`). A repository with no tag-listening workflow is a completed look. The default `GITHUB_TOKEN` does not start a downstream workflow; use a GitHub App installation token if cargo-dist must react. `oakum init` also prints a `check` job that runs only on pull requests. Putting `check` on the same default-branch push as `release` fails while tags are still being written.

```yaml
name: Release

on:
  push:

permissions:
  contents: write
  pull-requests: write

jobs:
  version:
    if: github.ref == format('refs/heads/{0}', github.event.repository.default_branch)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
        with:
          fetch-depth: 0 # oakum reads tags to determine the last release
      - name: Install oakum
        run: cargo binstall --no-confirm oakum@0.1.0
      - run: oakum ci version-pr
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
  release:
    if: github.ref == format('refs/heads/{0}', github.event.repository.default_branch)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
        with:
          fetch-depth: 0
      - name: Install oakum
        run: cargo binstall --no-confirm oakum@0.1.0
      - run: |
          git config user.name "github-actions[bot]"
          git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
      - run: oakum release
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

`fetch-depth: 0` is not optional. Tags are the record of what has been released, and a shallow clone does not have them.

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

Dogfood CI splits by event: pull requests run `mise run oakum -- check` and `ci pr-status` in `.github/workflows/ci.yml` (gated by CI Summary); default-branch pushes run `ci version-pr` and `release` in `.github/workflows/oakum.yml` (GitHub App token for both — version-pr so the PR is not `github-actions[bot]` and CI runs without a manual approval gate; release so tag pushes start cargo-dist). The generated `release.yml` host job uploads into the release oakum created.

Consumers keep the binstall / npm / mise `[tools]` shapes below.

### JavaScript: pin in `package.json`

An exact `devDependencies` entry, then `pnpm install` and `pnpm exec oakum`. Do not also `cargo binstall oakum@…` unless CI actually installs that way.

```json
{
  "devDependencies": {
    "oakum": "0.1.0"
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
oakum = "0.1.0"
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
"cargo:oakum" = { version = "0.1.0" }
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

## A note on tokens

The default `GITHUB_TOKEN` can open and update a version pull request through the GitHub API. Two cases still need a **GitHub App installation token** (or another non-`GITHUB_TOKEN` actor):

- **Version PR CI stuck on approval** — pull requests authored by `github-actions[bot]` may need a maintainer to approve workflow runs before CI executes. Mint the version commit with an App token so the PR author is your org bot instead. This repository does that in `.github/workflows/oakum.yml` via `./.github/actions/app-token`.
- **Downstream workflows on tag push** — events created with the repository's own `GITHUB_TOKEN` do not start new workflow runs. Give `oakum release` an App token if cargo-dist or another workflow must react to the tag. `oakum release` refuses a `workflow_dispatch`-only file before tagging.

## Publishing

`oakum release` stops at the tag and the GitHub release. Registry publishing is out of scope; cargo-dist owns artifacts. On this repository, `release.yml` uploads into the release oakum created. It creates a release only when `gh release view` reports the tag missing; other view failures fail the job. A later oakum publish path would target trusted publishing rather than tokens: npm revoked classic tokens in December 2025, and granular tokens expire every 90 days.
