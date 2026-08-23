# Running oakum in GitHub Actions

> Oakum is pre-release. This describes intended behavior; nothing here works yet.

Oakum does not write workflow files. You write them, or an agent writes them for you, and oakum verifies that what you wrote matches what it expects. That is a deliberate constraint — see [ADR-0003](../decisions/0003-write-only-what-a-command-owns.md) — and it has one consequence worth understanding before you copy anything below.

## Pin the version, and expect oakum to enforce it

Oakum's version determines bump math, changelog output, and what it writes to your manifests. If your workflow resolves the latest version on every run, your release behavior can change with no commit in your repository.

So `.changeset/_config.toml` declares an exact version:

```toml
#:schema ./_schema.json

tool-version = "0.1.0"
```

and every command except `upgrade` refuses to run when the binary disagrees with it, in either direction. Your workflow must install that same version.

`oakum init` prints a workflow with the version already filled in. Paste what it gives you rather than copying from this page, which will go stale.

## The two-job shape

Releasing splits into two jobs because they run at different times. The first maintains a version pull request as changes land. The second runs when that pull request merges.

**Only the first job is shown below.** The publish job's shape is not settled — it depends on the partial-failure ordering and the tag-and-verify sequence, which are still open. Copying this page today gives you the version pull request and no release; `oakum init` prints whatever is current, which is the reason to paste from it rather than from here.

```yaml
name: Release

on:
  push:
    branches: [main]

permissions:
  contents: write
  pull-requests: write

jobs:
  version:
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
```

`fetch-depth: 0` is not optional. Tags are the record of what has been released, and a shallow clone does not have them.

`oakum check` stays local. Pass `--remote` when the newest local tags should also appear in `git ls-remote --tags`. Default lookback is three; `--remote-lookback` changes it. A mismatch is unverified. Leave `--remote` off in pull-request jobs.

`run: oakum check` is an invocation, not a pin. `check` looks at **install sites**: a versioned `cargo binstall` / `cargo install` / `install-action` line in `.github/workflows`, an exact `oakum` entry in the root `package.json`, or an exact `oakum` / `cargo:oakum` pin in `.mise.toml` or `mise.toml`. Every site it finds must match `tool-version`.

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

This migrates the config, writes the new version, regenerates the schema, and reports what changed — all as one reviewable commit. If a migration fails it writes nothing.

Oakum never upgrades itself in CI. Doing so would turn a loud failure back into a silent behavior change.

## A note on tokens

The default `GITHUB_TOKEN` is enough for maintaining a version pull request. It is **not** enough if something downstream needs to react to a tag oakum pushes: events created with the repository's own `GITHUB_TOKEN` do not start new workflow runs. If you have a downstream release workflow, either give oakum a GitHub App installation token, or have the downstream workflow accept a `workflow_dispatch`, which is exempt from that rule.

Oakum reports which of these applies rather than assuming.

## Publishing

Not yet supported. When it lands, it will target trusted publishing rather than tokens — npm revoked classic tokens in December 2025, and granular tokens expire every 90 days, which makes a token-based release path something you have to maintain four times a year.
