# PR status channels: what peers actually show a reviewer

- Date: 2026-08-26
- Author: Jace Babin
- Scope: how changesets, bumpy, knope, release-please, and release-plz present a release plan on a contributor pull request (comment, check, job summary), and which command posts it. Not how they format changelogs.

## Question

`okm-961` must layer ADR-0015's three channels. The channels are locked. Which command delivers the comment and the step summary, and what do reviewers see from the peers?

## Sources

- `dmno-dev/bumpy` `docs/cli.md` on `main`, fetched 2026-08-26: <https://raw.githubusercontent.com/dmno-dev/bumpy/main/docs/cli.md>
- `dmno-dev/bumpy` `docs/github-actions.md` on `main`, fetched 2026-08-26: <https://raw.githubusercontent.com/dmno-dev/bumpy/main/docs/github-actions.md>
- `changesets/action` `pr-status/README.md` and `pr-comment/README.md` on `main`, fetched 2026-08-26
- Changesets automating guide, fetched 2026-08-26: <https://changesets.dev/guide/automating>
- Knope bot features, fetched 2026-08-26: <https://knope.tech/reference/knope-bot-github-app/features/>
- Knope `CreatePullRequest`, fetched 2026-08-26: <https://knope.tech/reference/config-file/steps/create-pull-request/>
- `googleapis/release-please-action` README on `main`, fetched 2026-08-26
- release-plz `release-pr` docs (from the 2026-08-26 version-PR survey): <https://release-plz.dev/docs/usage/release-pr>

## Findings

### Two product shapes

| Tool | Where a reviewer sees the plan | Gate on the contributor PR |
|---|---|---|
| changesets | Sticky comment (bot, or `pr-status` + `pr-comment`) | Optional. Default docs are **non-blocking**. Blocking is `changeset status --since main` in CI. |
| bumpy | Sticky comment from `bumpy ci check` | Same command. Local `bumpy check` is the no-GitHub gate. |
| knope CLI | Nowhere on the feature PR. Plan is the **release PR** body (`CreatePullRequest`). | Knope Bot (a GitHub App) is a separate check: "documented with a change file or conventional commit." |
| release-please | Nowhere on the feature PR. Plan is the **Release PR**. | None for changesets-style coverage. |
| release-plz | Nowhere on the feature PR. Plan is `release-pr`. | None. |

release-please and release-plz are not a UX model for this bead. Inferred from the fetched Action README and `release-pr` docs (neither mentions a feature-PR comment): they do not comment on ordinary PRs. knope CLI is the same unless the user installs Knope Bot (`CreatePullRequest` documents the release PR).

### changesets: render and post are different actions

The v2 action splits them (`pr-status/README.md`, `pr-comment/README.md`, fetched 2026-08-26):

- `changesets/action/pr-status` — **no job permissions**. Outputs `comment-body`. Does not post.
- `changesets/action/pr-comment` — needs `pull-requests: write`. Posts or updates one comment (`update-id`).

The automating guide's recommended non-blocking workflow is two jobs: generate the body, then post it. It still offers `changeset-bot` as "the easiest way to prompt for changesets without making them blocking." ADR-0012 already rejected a bundled app.

The guide's default is **do not fail CI** when a changeset is missing. A comment that asks for one is presentation, not the gate.

None of the fetched changesets pages mention `$GITHUB_STEP_SUMMARY`.

### bumpy: one CI command is both gate and comment

Inferred from fetched docs, not a command run here: `bumpy ci check` "Computes the release plan from bump files changed in the current PR and posts/updates a comment" (`docs/cli.md`, fetched 2026-08-26). Local `bumpy check` "No GitHub API needed."

`--comment` forces the comment on or off; default is auto-detect CI. `--strict` / `--no-fail` are the gate knobs on the **same** command.

`docs/github-actions.md` (fetched 2026-08-26) states the fork rule in one sentence: **fork PRs get the check, but not the comment.** The job still goes red on a missing bump file. The two-workflow `emit-comment` + `ci comment` path is optional.

The same page names two reasons to isolate *where* the combined command runs, not to split gate from comment:

1. Put `ci check` as an early step in the test job and a missing bump file **skips tests**.
2. Put `ci check --emit-comment` inside a long workflow and the fork comment waits until lint/test finish.

Their documented default is the combined command, folded into an existing CI job ("the least setup and the right default for most repos"). A dedicated check job is for those two cases.

None of the fetched bumpy pages mention `$GITHUB_STEP_SUMMARY` for the plan.

### knope / release-please / release-plz

Knope Bot keeps a check on non-draft PRs and, for same-repo members, buttons that commit a change file to the branch (`knope.tech` bot features, fetched 2026-08-26). That is an app. The CLI's `CreatePullRequest` overwrites title and body of the **release** PR.

release-please "maintains Release PRs" and updates them as work merges (Action README, fetched 2026-08-26). Feature PRs get no plan comment.

## Conclusions

Reviewers see a **sticky comment on the contributor PR**. Every tool that comments on that PR treats the comment as presentation. The tools that skip it put the plan on a different PR (the version/release PR), which oakum already has as `ci version-pr`.

Bumpy ships the gate and the comment on one command. Their docs call folding that command into an existing CI job the default; a dedicated job is only for slow CI plus fork comments, or when an early gate would skip tests. Oakum's dedicated-job recommendation is not bumpy's default. changesets already split render from post. Oakum already split decide from deliver: `check` writes nothing (ADR-0023), `status` never delivers (ADR-0016).

Job summaries are oakum's fork-safe detail channel (ADR-0015). The peers do not treat them as the primary UX. They complement a comment that cannot post; they are a poor replacement for the timeline.

**Locked 2026-08-26:** `oakum check` stays the gate. `oakum ci pr-status` owns the sticky comment and `$GITHUB_STEP_SUMMARY` as configured by `pr-status`. A token does not change `check`.

## Implications / actions

- `okm-961` implements `oakum ci pr-status`. Do not put delivery on `check` or `status`.
- Do not ship `--emit-comment` / `ci comment` in this slice (`okm-v8d`). Same-repo comment, forks get the check and no comment: that matches bumpy. The loud summary fallback (summary plus a log of why) is ADR-0015, not bumpy's default. Bumpy leaves the missing-bump explanation in job logs.
- Keep the check job separate from tests so a coverage miss does not hide a compile failure. That isolation is oakum's recommendation; bumpy still combines gate and comment.

## Open questions

- Whether the default `pr-status = both` comment should stay silent when `check` is non-strict and there is no plan and no uncovered package (ADR-0015: skip when the diff touches nothing oakum tracks).
- Whether `ci pr-status` should refuse to run outside Actions, or print the verdict to stdout like a local `status`.
