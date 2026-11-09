# GitHub App token scope and the persisted checkout credential

- Date: 2026-11-09
- Author: Jace Babin
- Scope: What an `actions/create-github-app-token` token can reach when a workflow sets no scope, and where `actions/checkout` leaves the credential later steps use.

## Question

The GitHub Actions guide tells a workflow to mint an App token for `ci pr-status`, `ci version-pr`, and `release`. What does that token reach when the workflow names no permissions and no repositories, and what does a checkout that persists credentials leave for the steps after it, such as a `pnpm install`?

## Sources

- `actions/create-github-app-token` README at tag `v3.3.0`, the version this repository's `app-token` action pins: <https://raw.githubusercontent.com/actions/create-github-app-token/v3.3.0/README.md>, read 2026-11-09
- `actions/checkout` README at tag `v7.0.1`, the version this repository and the guide pin: <https://raw.githubusercontent.com/actions/checkout/v7.0.1/README.md>, read 2026-11-09
- This repository's `.github/actions/app-token/action.yml`, `.github/workflows/ci.yml`, and `.github/workflows/oakum.yml` at `62e5fef`

## Findings

### Permissions

With no `permission-<name>` input, the token is not narrowed: "By default, the token inherits all of the installation's permissions." Inputs are the permission key prefixed with `permission-` ("e.g., `pull-requests` → `permission-pull-requests`"), and a permission the installation lacks is an error.

### Repositories

"If `owner` and `repositories` are empty, access will be scoped to only the current repository." Setting `owner` alone widens it: "If `owner` is set explicitly and `repositories` is empty, access will be scoped to all repositories in the provided repository owner's installation."

The action's outputs are `token`, `installation-id`, and `app-slug`.

### This repository

`app-token` always passes `owner: oakoss` and a `repositories:` list, and refuses before minting when no `permission-*` input is set ("no permission-* input is set, so the token would get every permission the App holds"). Each caller scopes its token: `ci.yml`'s static-analysis job to `contents: read` and `pull-requests: write`, `oakum.yml`'s version job to `contents: write` and `pull-requests: write`, its release job to `contents: write`.

### The persisted checkout credential

`persist-credentials` is "Whether to configure the token or SSH key with the local git config", default `true`. The README says the token "is configured with the local git config, which enables your scripts to run authenticated git commands." Since v6 it "now stores credentials in a separate file under `$RUNNER_TEMP` instead of directly in `.git/config`", and "`git fetch`, `git push`, etc. continue to work automatically." The README does not say whether a script can read the token itself; it says any later git command in the job is authenticated with it.

## Conclusions

An unscoped mint is the installation's full permission set on the current repository, so the guide's App-token snippets set `permission-*` inputs per job and leave `owner` unset. A checkout that persists credentials hands them to every later step, an install's scripts included, so a job that does not push should not persist them.

## Implications / actions

- The guide's fork-pull-request and release snippets set `permission-*` inputs and no `owner`.
- Re-read both READMEs when either pin moves a major.

## Open questions

- Whether a `pnpm install` lifecycle script can read the token from the `$RUNNER_TEMP` file directly, beyond running authenticated git commands; not measured.
