# Dependency bot commits and oakum coverage

- Date: 2026-11-09
- Author: Jace Babin
- Scope: What decides whether a Renovate or Dependabot pull request passes `oakum check --strict`, and what a workflow that writes the bump file in CI runs into (`okm-4ios`).

## Question

A dependency bot cannot write a bump file. Under `check --strict`, when does its pull request need one, and how can one be written without a human?

## Sources

- Dependabot options reference, `commit-message`: <https://docs.github.com/en/code-security/dependabot/working-with-dependabot/dependabot-options-reference>
- Dependabot on GitHub Actions: <https://docs.github.com/en/code-security/dependabot/troubleshooting-dependabot/troubleshooting-dependabot-on-github-actions>
- Triggering a workflow: <https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/trigger-a-workflow>
- `pull_request_target`: <https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows>, <https://docs.github.com/en/actions/reference/security/secure-use>
- Dependabot pull requests: <https://docs.github.com/en/code-security/dependabot/working-with-dependabot/managing-pull-requests-for-dependency-updates>
- Renovate presets and options: <https://docs.renovatebot.com/presets-default/>, <https://docs.renovatebot.com/presets-config/>, <https://docs.renovatebot.com/configuration-options/>, <https://docs.renovatebot.com/self-hosted-configuration/>
- oakum built from `main` at `6878736`, run on a scratch Cargo repository with `change-files` and `conventional-commits` on

## Findings

Read 2026-11-09.

**oakum.** Measured on one change to a Cargo package:

- `Bump serde from 1.0.1 to 1.0.2`: `check --strict` exits 1, naming the commit as not conventional and asking for a release; `generate --dry-run` writes `demo: patch`.
- `chore(deps): update serde to 1.0.2`: `check --strict` exits 0, "covered by commits that release nothing (chore)"; `generate --dry-run` writes `demo: none`.
- `fix(deps): update serde to 1.0.2`: `check --strict` exits 1; `generate` writes `demo: patch`, after which `check --strict` exits 0.
- `generate` reads commits since the merge base and writes a new file on each run: run again after its file is committed, it writes another.

**Dependabot.**

- Default messages "follow similar patterns to those detected in the repository", so they are not always conventional.
- `commit-message` options: `prefix` is "used for all commit messages unless `prefix-development` is also defined"; Dependabot inserts a colon after a prefix ending in a letter, number, `)` or `]`; `include` "supports only the value `scope`", after which the prefix is followed by `deps` or `deps-dev`.
- "By default, GitHub Actions workflows triggered by Dependabot get a `GITHUB_TOKEN` with read-only permissions", and "the only secrets available to the workflow are Dependabot secrets. GitHub Actions secrets are not available."

**GitHub Actions.**

- Events triggered by the `GITHUB_TOKEN` "will not create a new workflow run", with two exceptions: `workflow_dispatch` and `repository_dispatch` "always create workflow runs", and when a workflow using `GITHUB_TOKEN` creates or updates a pull request, the `opened`, `synchronize` or `reopened` event "creates workflow runs in an approval-required state", which a user with write access starts by approving them. An App installation token or a personal access token "also lets `pull_request` workflows run automatically (without the approval prompt)".
- `pull_request_target` runs "in the context of the default branch of the base repository", and such workflows "may have repository write access and access to referenced secrets". GitHub: workflows using it "must not explicitly check out untrusted code".

**Renovate.**

- `:semanticCommits` turns semantic prefixes on; `semanticCommits` otherwise defaults to `auto`, which follows the repository's history.
- `config:recommended` extends `:semanticPrefixFixDepsChoreOthers`: "Use semantic commit type `fix` for dependencies and `chore` for all others if semantic commits are in use". Lockfile-only updates are set back to `chore`.
- `:semanticCommitTypeAll(arg0)` "will use semantic commit type `arg0` for all commits".
- `postUpgradeTasks` commands are blocked by default on Mend-hosted apps; self-hosted, they must match `allowedCommands` (formerly `allowedPostUpgradeCommands`), set in the admin config. They run before Renovate commits, so `oakum generate`, which reads commits, has nothing to read there (inferred from Renovate's documented order; measured that `generate` finds nothing while the change is uncommitted).

**A push from another actor.**

- Renovate: "By default, Renovate will treat any PR as modified if another Git author has added to the branch. When a PR is considered modified, Renovate won't perform any further commits". `gitIgnoredAuthors` ("Specify commit author emails ignored by Renovate") lifts that for the authors it lists. <https://docs.renovatebot.com/configuration-options/>
- Dependabot: "By default, Dependabot will stop rebasing a pull request once extra commits have been pushed to it." A commit whose message carries `[dependabot skip]` (or `[skip dependabot]`, `[dependabot-skip]`, `[skip-dependabot]`) lets Dependabot force-push over it. <https://docs.github.com/en/code-security/dependabot/working-with-dependabot/managing-pull-requests-for-dependency-updates>
- When the bot does push over the bump-file commit, the branch adds no `.changeset/` file again, so a guarded workflow writes a fresh one on its next run. Inferred from the documented bot behavior and the guard's condition; no live workflow was run.

**`generate`'s base.** Without `--from`, `generate` tries only `origin/main`, `main` and `master` (docs/specs/generate.md). Measured by Codex on a scratch repository whose default branch is `trunk`: bare `generate --dry-run` exits 1 ("could not find a default base ref"), and `--from origin/trunk` writes `demo: patch`. A workflow passes the pull request's base.
