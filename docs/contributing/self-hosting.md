# Self-hosting oakum

This repository cuts oakum with the **workspace binary**, not a crates.io or mise `[tools]` pin of a prior release ([ADR-0038](../decisions/0038-the-install-pin-is-the-version.md)). Local dogfood:

```bash
mise run oakum -- check
mise run oakum -- status
```

That task runs `cargo run -q -p oakum --`. Do not add an oakum pin under `[tools]` here; that would claim a registry install this tree does not use. `check` and the write gate treat `crates/oakum`'s package version as the install pin when the member is named `oakum`, so the binary built from the tree always matches it.

Dogfood CI splits across two workflow files:

**Pull requests** — [`.github/workflows/ci.yml`](../../.github/workflows/ci.yml):

- `mise run check` (repo-wide lint gate)
- `mise run oakum -- check --strict` on every pull request event except when `oakoss[bot]` opens or updates the version PR from this repository on `oakum/version-packages` (the version PR *is* the bump); a person's push to that branch still runs it
- `mise run oakum -- ci pr-status` with a GitHub App token so plan comments and version-PR authorship are the org bot, not `github-actions[bot]` (runs inside static-analysis, before CI Summary finishes aggregating other jobs)
- CI Summary gates merge readiness on static-analysis, tests, secret-scan, audit, and windows — it does not gate pr-status, which may post while a later required job is still running

**Default-branch pushes** — [`.github/workflows/oakum.yml`](../../.github/workflows/oakum.yml):

- `$/.github/actions/harden-checkout` — harden-runner + checkout (no bundled mise setup)
- `$/.github/actions/app-token` — mint an org App installation token
- `$/.github/actions/setup` — mise toolchain
- `mise run oakum -- ci version-pr` and `mise run oakum -- release` both use the App token (`GITHUB_TOKEN` does not start cargo-dist on tag push)
- Release job re-checks out with `persist-credentials: true` and configures git identity from the App token before `oakum release`

The generated `release.yml` host job uploads into the release oakum already created.

## Measured on oakum (2026-09-01)

The [GitHub Actions guide](../guide/github-actions.md) was first derived from these runs, through **v0.1.2**: pull-request `check` and `ci pr-status` in `ci.yml`, default-branch `ci version-pr` and `release` in `oakum.yml`, and cargo-dist reacting to the tag in the generated `release.yml`.

Recorded before `check` gained the two-line stdout report it opens with once it reaches a look, so the `no stdout` below is what that run produced, not what the command does now. A run that refuses before reading `_config.toml` still writes nothing to stdout.

| Event | Workflow | Command | Run |
| --- | --- | --- | --- |
| PR #151 opened | CI | `mise run oakum -- check` | [33561077356](https://github.com/oakoss/oakum/actions/runs/33561077356) — exit 0, no stdout |
| PR #151 opened | CI | `mise run oakum -- ci pr-status` | same run — exit 0 with App token |
| #151 merged to `main` | oakum | `mise run oakum -- ci version-pr` | [33561801977](https://github.com/oakoss/oakum/actions/runs/33561801977) — opened version PR #152; release job printed `nothing to release` |
| #152 merged to `main` | oakum | `mise run oakum -- release` | [33562869917](https://github.com/oakoss/oakum/actions/runs/33562869917) — tagged **v0.1.2**, created GitHub Release |
| tag **v0.1.2** pushed | Release (cargo-dist) | build + publish | [33562952734](https://github.com/oakoss/oakum/actions/runs/33562952734) — npm, crates.io, homebrew all green |

The post-#152 oakum run's version job is a no-op once changesets are consumed; the row above is the push that actually opened #152.
