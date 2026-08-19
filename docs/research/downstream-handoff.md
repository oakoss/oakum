# Verifying the handoff to a downstream release workflow

- Date: 2026-08-18
- Author: Jace Babin
- Scope: Whether a tool that ends at a git tag can confirm the workflow meant to react to that tag actually ran.

## Question

Oakum's job may end at pushing a tag, with cargo-dist reacting to it. The failure to design against: the tag lands, the downstream workflow never runs, nothing notices, and a fix reaches nobody while every check passes. Can that handoff be verified — or removed?

## Sources

cargo-dist templates and generated workflows (`dist 0.32.0`, both trigger modes generated locally and diffed), GitHub Actions documentation source, GitHub REST OpenAPI spec, live reads against production repositories using dist.

## Findings

### The handoff can be removed rather than observed

`dispatch-releases = true` generates `on: workflow_dispatch` with a `tag` input instead of `on: push: tags`. The triggers are **mutually exclusive** in the template — dispatch mode replaces tag-push rather than supplementing it.

Generating both workflows from one project and diffing them produces five hunks, all inside the `on:` block and the `plan` job. Every downstream job is byte-identical; they all gate on `needs.plan.outputs.publishing`, which is redefined from `!github.event.pull_request` to `inputs.tag && inputs.tag != 'dry-run'`.

In dispatch mode **dist creates the tag itself**, with `gh release create` — creating a release implicitly creates the tag. There is no handoff left to fail, and a failed release leaves no tag behind, because dist validates the tag against workspace versions in `plan` before anything becomes permanent.

Two GitHub facts make this work:

- **`workflow_dispatch` is exempt from the `GITHUB_TOKEN` suppression rule.** Verbatim: *"events triggered by the `GITHUB_TOKEN` will not create a new workflow run, with the following exceptions: `workflow_dispatch` and `repository_dispatch` events always create workflow runs."*
- **The dispatch API returns the run ID.** `return_run_details: true` yields `{workflow_run_id, run_url, html_url}` instead of a bare 204 — shipped 2026-02-19. **github.com only**; GHES 3.17 through 3.19 remain 204-only.

That second point matters because post-hoc correlation is unreliable: a real dispatch run reports `head_branch: main` and `display_title: "Release"`, with the tag appearing nowhere in the run metadata, and the workflow-runs API has no ref or tag filter.

### Production evidence, and the counterweight

119 code-search hits for `dispatch-releases = true`, including `astral-sh/uv`, `ruff`, `ty`, PostHog, probe-rs, and tinymist. uv's recent release runs all show `event: workflow_dispatch`, `conclusion: success`.

Against that: dist labels the feature experimental, and **dist itself does not use it** — its own release workflow triggers on tag push.

### The failure being designed against is a known, unfixed dist bug

`axodotdev/cargo-dist#190`, open since 2023-03-07, from the maintainer: *"OF COURSE I coincidentally only tested up to 3 tags at once so I never hit this absolutely brutal and arbitrary restriction from github."* A 2025-09-22 comment reports the `GITHUB_TOKEN` variant of the same silent failure.

dist's only in-product warning is a comment in its generated YAML, and it misstates the limit as three tags per *commit* rather than per push.

### GitHub limits, verified

- *"Events will not be created for tags when more than three tags are pushed at once."* This kills the `push` event for the **whole push**, not just tags four and up.
- Whether an API-created release emits a `push` event for the tag it creates is **not documented**. `create` explicitly covers API-made refs; `push` is described only as "when you push a commit or tag". Moot under dispatch.
- `x-poll-interval` is an Events API header and is **not** returned by `/actions/runs`, which instead returns `cache-control: max-age=60` and an ETag. Conditional requests are free: 22 consecutive `If-None-Match` calls returning 304 moved `x-ratelimit-used` by zero.

### No release tool verifies its handoff — the mature ones deleted it

semantic-release's path ends at `logger.success("Created tag")`. release-please documents the non-triggering as a known constraint. changesets treats it as a configuration problem. goreleaser never owns a handoff — it runs inside the already-triggered workflow.

knope's recommended recipe is `on: workflow_dispatch` with prepare, build, and release as three jobs in **one run**. release-plz offers action outputs so work chains inside the same run. uv's release script opens a PR and never pushes a tag; a human runs the workflow with the version.

Six tools, independent convergence, same answer: collapse the handoff rather than observe it.

## Conclusions

Triggering and tracking beats polling and reporting, and it makes the failure structurally impossible rather than merely detectable.

## Implications / actions

- Detect the downstream trigger mode by parsing the generated workflow's `on:` block. dist generates that file, and `dist generate --check` reports when it has been edited. `dist plan --output-format=json` does **not** expose `dispatch_releases`.
- Where dispatch is available: `POST .../dispatches` with `return_run_details: true`, then track `GET /actions/runs/{id}`.
- Where only tag-push exists: assert the preconditions — at most three tags in a push, credentials that are not the repository's own `GITHUB_TOKEN` — and report the polling path as explicitly best-effort. Never print "fine" for "we didn't look."
- Adopting dispatch mode in a repository changes its generated `release.yml`, which is a decision for that repository's owner rather than a mechanical consequence.

## Open questions

- Whether a hybrid — push the tag, then dispatch with `ref: <that tag>` — is worth it. The API accepts a tag as `ref`, `github.sha` then resolves to the tagged commit, and `gh release create` reuses an existing tag. It buys tag ownership at the cost of reintroducing the tag-without-artifact state. Reasoned from verified primitives; not executed end to end.
- cargo-dist's maintenance status after axo.dev. Signals are contradictory and it matters twice over: the dispatch feature is experimental and undogfooded, and issue #190 has been open three years.
