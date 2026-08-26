# Version-PR command surface: what peers actually invoke

- Date: 2026-08-26
- Author: Jace Babin
- Scope: which command opens or updates the version or release pull request in the tools oakum treats as peers. Not how they format the PR body.

## Question

`okm-kx4` must open and update oakum's version pull request. ADR-0023 assigned that write to `version`. The Actions guide (then marked intended) showed `oakum ci version-pr`. Shipped `init` printed only `oakum check`. What do the other release managers actually run?

## Sources

- `changesets/action` `version/README.md` on `main`, fetched 2026-08-26: <https://raw.githubusercontent.com/changesets/action/main/version/README.md>
- `changesets/action` root README on `main`, fetched 2026-08-26: <https://raw.githubusercontent.com/changesets/action/main/README.md>
- `changesets/action` v2.0.0 release notes (2026-08-11): <https://github.com/changesets/action/releases/tag/v2.0.0>
- `dmno-dev/bumpy` `docs/cli.md` on `main`, fetched 2026-08-26: <https://raw.githubusercontent.com/dmno-dev/bumpy/main/docs/cli.md>
- `dmno-dev/bumpy` `docs/github-actions.md` on `main`, fetched 2026-08-26: <https://raw.githubusercontent.com/dmno-dev/bumpy/main/docs/github-actions.md>
- Knope `PrepareRelease` reference, fetched 2026-08-26: <https://knope.tech/reference/config-file/steps/prepare-release/>
- Knope `CreatePullRequest` reference, fetched 2026-08-26: <https://knope.tech/reference/config-file/steps/create-pull-request/>
- Knope "Preview releases with pull requests" recipe, fetched 2026-08-26: <https://knope.tech/recipes/1-preview-releases-with-pull-requests/>
- `googleapis/release-please-action` README on `main`, fetched 2026-08-26: <https://raw.githubusercontent.com/googleapis/release-please-action/main/README.md>
- `npx --yes release-please --help`, run 2026-08-26: prints `release-please release-pr` as "create or update a PR representing the next release"; `manifest-pr` and `manifest-release` are deprecated
- release-plz `release-pr` docs, fetched 2026-08-26: <https://release-plz.dev/docs/usage/release-pr>
- oakum [ADR-0023](../decisions/0023-name-every-verb-and-what-it-owns.md), [docs/guide/github-actions.md](../guide/github-actions.md), `crates/oakum/src/cli/init.rs` `print_workflow_and_footer` (read 2026-08-26)

## Findings

### Local version-write and GitHub PR are never the same invocation

Every peer that has a local "apply the plan" command keeps GitHub PR creation off that command.

| Tool | Local write | Opens / updates the PR |
|---|---|---|
| changesets | `changeset version` | `changesets/action/version` (runs a version `script`, then commits and opens the PR) |
| bumpy | `bumpy version` (optional `--commit`) | `bumpy ci release` (version-PR mode when bump files exist) |
| knope | `PrepareRelease` step (stages; does not commit) | `CreatePullRequest` step, usually inside a `prepare-release` workflow |
| release-plz | `release-plz update` | `release-plz release-pr` (runs update, then opens the PR) |
| release-please | none — it is CI-shaped | `release-please release-pr` / the Action |

`changeset version` is only the script that edits manifests and changelogs. The `/version` README (fetched 2026-08-26) says the action "versions packages and creates or updates a pull request" and names its input `script`, with no default on that page. The root action's `version-script` input "Default to `changeset version` if not provided" (root README, fetched 2026-08-26). v2.0.0 renamed the **root** input `version` → `version-script`; `/version` was added in the same release and uses `script`.

`bumpy version` "Consume[s] all pending bump files and apply the release plan" and lists package.json, CHANGELOG, bump-file deletes, lockfile, and optional `--commit`. It does not mention a pull request. `bumpy ci release` is the CI command whose default mode "creates or updates a 'Version Packages' PR" when bump files exist (`docs/cli.md`, fetched 2026-08-26).

Knope's `PrepareRelease` page: "This step doesn’t commit the changes." `CreatePullRequest` is a separate step. The documented recipe is a workflow named `prepare-release` that humans and CI invoke as `knope prepare-release`: one CLI word that is a workflow name, not `knope version`.

release-plz: "`release-plz release-pr` runs release-plz update and opens a GitHub Pull Request" (<https://release-plz.dev/docs/usage/release-pr>, fetched 2026-08-26).

release-please has no local analog of `oakum version`. `npx --yes release-please --help` (run 2026-08-26) still lists `release-pr` as current. The Action README's migration table is about the old Action `command` input: `command: release-pr` maps to `skip-github-release: true`, with the cell text "This command was used for only opening release PRs."

No surveyed tool documents "if `GITHUB_TOKEN` is set, the local version command also opens a PR."

### Privilege split is a separate command

Bumpy's recommended workflow is three jobs (`docs/github-actions.md`, fetched 2026-08-26):

1. `bumpy ci plan` — no write permissions — emits `mode` of `version-pr` / `publish` / `nothing`
2. `bumpy ci release --expect-mode version-pr` — `contents: write` + `pull-requests: write`
3. `bumpy ci release --expect-mode publish` — separate `environment: publish`

`--expect-mode` exists so a job cannot silently take the other path. `bumpy version` is not used in those jobs.

changesets splits the same way in v2 by **Action**, not CLI: `/version` vs `/publish` vs the combined root action (`v2.0.0`, 2026-08-11).

Knope splits by **workflow file**: `knope prepare-release` on push to main, `knope release` after merge.

release-please's Action combines PR and GitHub release unless you set `skip-github-pull-request` or `skip-github-release`. `release-pr` and `github-release` are current CLI verbs (`release-please --help`, run 2026-08-26). The Action dropped its `command` input.

### How the commit reaches GitHub

release-plz's `release-pr` docs (fetched 2026-08-26): on GitHub it "create[s] a commit through the GraphQL API rather than making a commit locally and pushing", so the commit is Verified without a GPG key. That is the same mutation oakum implemented in `okm-dlo`.

`changesets/action/version` (fetched 2026-08-26) defaults to the GitHub API (`push-with-git-cli` defaults to `false`) and says those commits "are signed using GitHub's GPG key".

Bumpy's `ci release` pushes the version branch with `GH_TOKEN`, and uses `BUMPY_GH_TOKEN` when set so the version PR actually triggers workflows (`docs/cli.md`).

Knope's recipe uses `git commit` / `git push` as `Command` steps, then `CreatePullRequest`.

### What oakum prints

`print_workflow_and_footer` in `crates/oakum/src/cli/init.rs` emits a `check` job and a push-only `version` job that runs `oakum ci version-pr`.

[docs/guide/github-actions.md](../guide/github-actions.md) marks `oakum ci version-pr` as shipped and the publish job as still open.

ADR-0023 (amended 2026-08-26) gives `ci version-pr` the pull request and `version` the file bytes. The same table gives `check` the exit-code gate and [ADR-0015](../decisions/0015-layer-the-pr-status-channels.md) the comment/summary.

## Conclusions

Peers split "write the bump" from "put that bump on a GitHub pull request." The PR is a CI command, a named workflow, or a GitHub Action that calls the local command.

Putting the PR on bare `oakum version` whenever a token is in the environment would be a shape none of these tools document. A developer with `gh` auth exported would open a remote PR from a local bump.

`oakum ci version-pr` matches bumpy's `ci release --expect-mode version-pr` and the existing guide. `oakum version --pr` would keep a single verb and still make GitHub opt-in. Both preserve ADR-0023 if `version` remains the code path that produces the files the PR contains.

**Locked 2026-08-26:** `oakum ci version-pr` opens and updates the version PR. `oakum version` stays a local working-tree write. A token in the environment does not change `version`.

## Implications / actions

- `okm-kx4` implements `oakum ci version-pr`. Do not infer GitHub from `GITHUB_TOKEN` alone on `oakum version`.
- The write set `version` produces is the PR payload. `ci version-pr` commits it via `createCommitOnBranch`, opens or updates one PR, and stamps `tool-version` in the body (ADR-0007).
- `init`'s printed workflow includes the version-PR job.
- Tag and GitHub release stay `okm-mog` / the future `release` verb (ADR-0023). Do not copy bumpy's single `ci release` that smart-routes PR vs publish unless oakum later locks that.

## Open questions

- Whether `oakum ci plan` (bumpy's low-privilege gate) is in `okm-kx4` or later. Bumpy treats it as the reason the version-PR job can hold only PR-write creds.
- Branch name, PR title, and whether an existing human-touched version PR is updated in place (changesets/bumpy) or closed and reopened (release-plz when the PR has non-bot commits).
