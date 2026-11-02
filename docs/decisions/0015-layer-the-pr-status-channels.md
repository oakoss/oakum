# Layer the pull-request status channels; gate on the exit code alone

- Status: accepted
- Date: 2026-08-18
- Deciders: Jace Babin

## Context and Problem Statement

On a pull request, oakum has something to say: which packages will release at which versions, and which changed packages have no change file. There are three ways to say it and they have different permission requirements, different failure modes, and different audiences. Which one carries the message, and what happens when it is unavailable?

## Decision Drivers

- A fork's `pull_request` run gets a read-only token with secrets withheld, enforced server-side, so a comment cannot be posted there
- The coverage gate is the reason the check exists; presentation is not
- A reviewer looking at a pull request sees the timeline, not the run UI
- changesets solves this with a GitHub App the user has to install and trust; [ADR-0012](0012-scope-v0-to-version-math-and-the-github-layer.md) rules out a bundled action or app

## Considered Options

- Comment only, as changesets and bumpy do
- Job summary only
- Layer all three by what each is actually good for

## Decision Outcome

Chosen option: **layer all three**. They are not redundant channels at different reliability levels; they carry different granularity.

| Channel | Permissions | Works on forks | Role |
|---|---|---|---|
| Job exit code | none | yes | the gate |
| `$GITHUB_STEP_SUMMARY` | none — it is a file write | yes | the detail |
| Pull-request comment | `pull-requests: write` | no | the verdict |

**The comment carries the verdict and a concise plan** — packages releasing with their versions, or a coverage gap naming the package. Short enough to read without expanding. **The summary carries the detail** — the per-package table, each computed version with the reason it was chosen, the cascade explanation, coverage per package. It is what you open when the verdict surprises you.

Presentation is configurable as `pr-status = "comment" | "summary" | "both" | "none"`, defaulting to `both` — kebab-case, matching `tool-version` ([ADR-0007](0007-pin-the-tool-version-in-config.md)) and Cargo's own convention for the TOML the config lives in. **That settles the spelling for every config key, not just this one**, which is why [ADR-0009](0009-delivery-artifacts-always-cascade.md)'s `resolves-dependencies-at` carries an amendment note: it was written the same day in snake_case. Three rules come with the setting, and without them it is a footgun:

**`none` disables presentation, never the gate.** The exit code is not configurable. Someone setting `none` to quiet a busy repository must not silently stop enforcing coverage, so the key is named for what it controls.

**Degrade loudly, not silently.** On a fork pull request, `comment` and `both` cannot post. Do not fail the job and do not skip quietly — fall back to the summary and say why in the log: *comment requested but this run has no write permission (fork pull request); wrote the plan to the job summary instead.* That is the fork story solved without bumpy's `workflow_run` artifact dance, and it keeps the user's intent visible rather than swallowed.

**Comment when the tool has an opinion.** Releases pending or a coverage gap both get a comment, including the happy-path "these will release" case. Skip entirely when the diff touches nothing oakum tracks, which is the one case where a comment really is wallpaper.

Outside Actions — a local `check` — neither channel applies and output goes to stdout, so the setting is CI-scoped.

### Consequences

- Good, because a fork contributor still sees the full plan, in the checks UI rather than the timeline. That removes most of the reason for bumpy's two-workflow dance
- Good, because each layer degrades independently: no permissions still gates and still shows the plan; no Actions environment at all still gates and prints
- Good, because no GitHub App is involved. changesets ships `changeset-bot` — a third-party app to install and trust — for one comment
- Bad, because the one thing given up against `changeset-bot` is a comment on fork pull requests. `--emit-comment <dir>` keeps bumpy's dance available to anyone who wants it, but it is not the default path or a documented requirement
- Neutral, because release-please and knope do not comment on ordinary pull requests at all, so there is no convention being broken

### Confirmation

**Verified, because the permission claim is the load-bearing one.** This repository's own `CI Summary` job writes `$GITHUB_STEP_SUMMARY` under workflow-level `permissions: contents: read` — no `pull-requests: write`, no elevated token — and renders on every run. GitHub's documentation describes job summaries as a file on the runner that you append to and states no token permission requirement. A read-only fork token does not block a file write.

**One limit to design around:** 1 MiB per step, and exceeding it fails the step's upload. An open runner issue reports content being silently dropped as the limit approaches ([actions/runner#4337](https://github.com/actions/runner/issues/4337)). A release plan is a few KB, so this is nowhere near a practical limit — but it is the reason the summary must never be the only record.

## More Information

**The invariant to write down: the gate must never depend on the comment.** The exit code is what fails a fork pull request missing a change file. If the comment ever becomes load-bearing, forks silently stop being gated, and that gets discovered from a contributor rather than from the tool.

**One template per surface with conditionals inside**, rendered against the shared context object from [ADR-0016](0016-emit-release-state-render-it-never-deliver-it.md) — not a separate template per state. The states are not mutually exclusive: a pull request can bump two packages *and* be missing a change file for a third, and per-state templates force one story to be told.

**Stickiness is what makes a comment acceptable.** Multiple comments are noise; one comment that updates in place is a status indicator. A hidden marker in the body (`<!-- oakum:pr-plan -->`) finds the previous one. Two edge cases worth handling on day one: recreate it if a human deleted it, and if several somehow exist, update the newest and delete the rest rather than leaving duplicates that each claim to be current.

**Authorship (amended 2026-09-01).** Sticky plan comments and version-PR commits should appear as the repository's GitHub App bot, not `github-actions[bot]`. Mint an org App installation token for `ci pr-status` on pull requests and for `ci version-pr` / `release` on default-branch pushes (`./.github/actions/app-token` in this repository). Own sticky comments by marker plus bot authorship (`login` ends with `[bot]`); human quotes of the marker are never owned. Fork pull requests still degrade to the job summary when no token can write; an org App on trusted workflows may post on fork PRs to upstream when secrets are available.

**A trusted poster (amended 2026-10-30, `okm-1t5o`).** `ci post-comment DIR` posts what `--emit-comment DIR` wrote, so the job holding the write token runs no pull-request code. It serves a same-repository split, not forks: the pull request number comes from the posting job's own event (`pull_request`, or a `workflow_run` naming exactly one same-repository pull request), never from the untrusted artifact, and a fork's `workflow_run` names none. A completed `--emit-comment` run leaves exactly one file, the body or a `.none` marker, and one that stops leaves neither, so a lost or partial artifact is unverified rather than "nothing to post". A fork sees the plan only in the emitting job's summary, which `both` and `summary` write; under `comment` it gets neither.

**A plan names what it describes, and a failed run marks it stale (amended 2026-11-02, `okm-fxjb`, `okm-u7eg`).** A run that never happens, cancelled or skipped by a path filter, leaves the last plan standing, so every plan comment opens with the commit it describes, the pull request's head rather than the merge commit Actions checks out, and links the run that wrote it. A run that cannot build the plan (a shallow clone, unreadable tags, a coverage look that could not run, a selection naming no package) still fails with its own exit code; it neither deletes the comment nor leaves it reading as current, but marks it stale: a banner with the reason above the last plan oakum checked, kept collapsed, so a second failure keeps that plan rather than nesting. With no earlier plan the banner stands alone, and a plan too large to keep beside it is left out with a line saying so. The reason is text, escaped, so it cannot pose as the comment's own structure. Only oakum's own comment is touched, and a banner that cannot be posted is a stderr line beside the run's failure. In the split, the emitting run writes `oakum-pr-comment.failed` holding only the reason, and `post-comment` composes the banner, naming the commit and run from its own event, and exits `0` since the emitting job already failed, or `2` when an older copy of the comment could not be removed and may still read as current. A failed run with the comment channel off writes the `.none` marker, as a run that succeeds does. The emitted directory records its format beside the files (`oakum-pr-comment.format`), so a poster older than the emitter, as on a pull request that upgrades oakum, refuses by naming both versions; a poster from before the record (0.8.x) reads a `.failed` artifact as "neither file" and touches nothing, which is the safe direction. The emitting job's upload and the posting job need `if: always()`, or a failed run never reaches the poster.

- [ADR-0006](0006-no-command-execution-in-templates.md) — the template engine both surfaces render through
- [templating prior art](../research/templating-prior-art.md) — how the surveyed tools customize release text
