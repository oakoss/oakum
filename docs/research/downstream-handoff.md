# Verifying the handoff to a downstream release workflow

- Date: 2026-08-18, revised 2026-08-19 and 2026-08-28
- Author: Jace Babin
- Scope: Whether a tool that ends at a git tag can confirm the workflow meant to react to that tag actually ran.

## Question

Oakum's job may end at pushing a tag, with cargo-dist reacting to it. The failure to design against: the tag lands, the downstream workflow never runs, nothing notices, and a fix reaches nobody while every check passes. Can that handoff be verified — or removed?

## Sources

cargo-dist templates and generated workflows (`dist 0.32.0`, both trigger modes generated locally and diffed), GitHub Actions documentation source, GitHub REST OpenAPI spec, live reads against production repositories using dist.

## Findings

### The handoff can be removed rather than observed

`dispatch-releases = true` generates `on: workflow_dispatch` with a `tag` input instead of `on: push: tags`. The triggers are **mutually exclusive** in the template — dispatch mode replaces tag-push rather than supplementing it.

Generating both workflows from one project and diffing them produces five hunks: a leading comment, the `on:` block, two in the `plan` job, and one loosening `build-local-artifacts`' `if:` gate to also fire on the `dry-run` tag. Every *other* job is byte-identical; they all gate on `needs.plan.outputs.publishing`, which is redefined from `!github.event.pull_request` to `inputs.tag && inputs.tag != 'dry-run'`.

In dispatch mode **dist creates the tag itself**, with `gh release create` — creating a release implicitly creates the tag. There is no handoff left to fail, and a failed release leaves no tag behind, because dist validates the tag against workspace versions in `plan` before anything becomes permanent.

Two GitHub facts make this work:

- **`workflow_dispatch` is exempt from the `GITHUB_TOKEN` suppression rule.** The documentation, condensed from an intro sentence and its bullet list, which carries a second exception this omits: *"events triggered by the `GITHUB_TOKEN` will not create a new workflow run, with the following exceptions: `workflow_dispatch` and `repository_dispatch` events always create workflow runs."*
- **The dispatch API returns the run ID.** `return_run_details: true` yields `{workflow_run_id, run_url, html_url}` instead of a bare 204 — shipped 2026-02-19. **github.com only**; GHES 3.17 through **3.20** remain 204-only, and it first appears in the ghes-3.21 spec (`github/rest-api-description`, checked 2026-08-19).

That second point matters because post-hoc correlation is unreliable: a real dispatch run reports `head_branch: main` and `display_title: "Release"`, with the tag appearing nowhere in the run metadata, and the workflow-runs API has no ref or tag filter.

### What a tag-push run reports, measured

The polling path keys everything on the tagged commit's sha, which raised a question three reviewers flagged as unsettled by documentation: `GITHUB_SHA` is documented only as depending on the triggering event, with no per-event table and no mention of annotated-tag peeling. Measured on the live API 2026-08-27, run 33142360694 — the `v0.1.0-rc.1` push, an annotated tag, the shape `Op::AnnotatedTag` cuts:

- `head_sha` = `735fc8d2b0756a5f95c50c6d4fd4f0363369696e` — the **peeled commit**, not the tag object (`db04fc1e...`, which appears nowhere in the run)
- `head_branch` = `v0.1.0-rc.1` — the tag's **short name**
- `event` = `push`

Three consequences:

- Keying the snapshot/confirm/absorb queries on `?head_sha=<tag.commit>` is correct: the run carries exactly that commit sha even for an annotated tag.
- The sha alone is ambiguous: `?head_sha=735fc8d` also returns the CI and CodeQL runs from the branch push at the same commit, with `head_branch: main`. Filtering on workflow path and event already separates those; what it cannot separate is a leftover run of the listening workflow itself at the same commit — a branch push of a workflow listening to both, or a pre-existing run found when resuming an already-pushed tag. `head_branch` carrying the tag's short name makes that case decidable: deserialize it and require it to equal the tag name before a push run counts.
- A lightweight tag's run is unmeasured. Oakum only cuts annotated tags, so the annotated case is the load-bearing one; a lightweight data point can come from a scratch repo if ever needed.

### Production evidence, and the counterweight

GitHub's `/search/code` for the quoted phrase `"dispatch-releases = true"` returned 119 hits on 2026-08-18 and 117 on 2026-08-19; the same term unquoted returns 156 and `dispatch-releases` alone 178, so the number means nothing without the query form. Read it as roughly 120 adopters, noting that a minority of hits are documentation rather than configuration — and that this document is now among them. The named adopters are stable: `astral-sh/uv`, `ruff`, `ty`, PostHog, probe-rs, and tinymist. uv's recent release runs all show `event: workflow_dispatch`, `conclusion: success`.

Against that: dist introduced the feature as experimental (0.8.0 changelog, *"adds a new experimental mode where releases are triggered with workflow-dispatch"*), though at v0.32.0 its config reference no longer carries the experimental banner that seven other features on that page do. And **dist itself does not use it** — `axodotdev/cargo-dist` `.github/workflows/release.yml` triggers on `push: tags`.

### The failure being designed against is a known, unfixed dist bug

[`axodotdev/cargo-dist#190`](https://github.com/axodotdev/cargo-dist/issues/190), *"GH workflow doesn't run when *more than 3* tags are pushed"*, opened 2023-03-07 and still open (re-checked 2026-08-19). From Gankra the same day: *"To be clear this was indeed an intended/supported workflow that I tested but of course OF COURSE I coincidentally only tested up to 3 tags at once so I never hit this absolutely brutal and arbitrary restriction from github."* A 2025-09-22 comment reports the `GITHUB_TOKEN` variant of the same silent failure, still unresolved.

dist's only in-product warning is a comment in its generated YAML, and it misstates the limit as three tags per *commit* rather than per push.

### GitHub limits, verified

- *"Events will not be created for tags when more than three tags are pushed at once."* Whether that drops only tags four and up or the whole push is not stated; the safe reading is the whole push, and the scope question is recorded in [github-release-path.md](github-release-path.md)'s open questions.
- Whether an API-created release emits a `push` event for the tag it creates is **not documented**. `create` explicitly covers API-made refs; `push` is described only as "when you push a commit or tag". Moot under dispatch.
- `x-poll-interval` is an Events API header and is **not** returned by `/actions/runs`, which instead returns `cache-control: private, max-age=60, s-maxage=60` and an ETag. Conditional requests are free: 22 consecutive `If-None-Match` calls returning 304 moved `x-ratelimit-used` by zero.

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
