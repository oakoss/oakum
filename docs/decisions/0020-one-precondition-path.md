# Run one precondition path; `check` stops where `release` continues

- Status: accepted
- Date: 2026-08-18
- Deciders: Jace Babin

## Context and Problem Statement

The value of a validation command is that passing it predicts a successful release. That only holds if it checks the same things the release path checks. Two implementations of "is this repository ready?" will agree on the day they are written and drift afterward, and the drift is invisible until a release fails a check that passed.

## Decision Drivers

- The stated need was to find problems early: run it and be told what is missing "before I run into an issue"
- [ADR-0011](0011-stop-at-the-tag.md) already requires preflighting the whole set, because a partial release cannot be undone
- A validation command that passes and is then contradicted by the release is worse than no validation command

## Considered Options

- A `check` that implements its own validation, tuned for local feedback
- A `check` that shells out to `release --dry-run`
- One precondition path both commands enter, differing only in what happens after it passes

## Decision Outcome

Chosen option: **one precondition path.**

`check` runs the preconditions and stops. `release` runs the same preconditions and then acts. There is one implementation, so the two cannot disagree about what "ready" means — the difference between the commands is what follows, not what is verified.

This is the failure mode to design against, and it is observed rather than hypothetical. bumpy answers "is this published?" three different ways in one tool: `checkIfPublished`'s cascade (a custom `checkPublished` command, then `git tag -l`, then `npm info`), an `alreadyPublished` set derived from draft-release metadata, and `fetchPublishedVersions` on the channel and snapshot paths. Once three answers to the same question exist, the one a given code path happens to call is the one that decides, and nothing makes them agree. (`@varlock/bumpy` 1.18.1 `dist/publish-*.mjs`, read 2026-08-18.)

`check` stays pure under [ADR-0003](0003-write-only-what-a-command-owns.md) — it reports drift and names the fix, never applies it — which is what makes it safe to run anywhere, including the git hook in [idea 0003](../ideas/0003-check-as-a-git-hook.md).

### Consequences

- Good, because a green `check` is a real prediction rather than a similar-looking one
- Good, because a precondition added for the release path is automatically available to `check`, with no second place to remember
- Bad, because the shared path must stay free of side effects to be callable from `check`, which constrains how preconditions can be written — anything that wants to fix what it finds has to live outside it
- Neutral, because the network-tiered split in [ADR-0016](0016-emit-release-state-render-it-never-deliver-it.md) still applies: the local preconditions are shared, and the remote delivery pass is opt-in on both

## More Information

- [ADR-0003](0003-write-only-what-a-command-owns.md) — why `check` cannot repair what it reports
- [ADR-0011](0011-stop-at-the-tag.md) — the preflight that replaces rollback
- [ADR-0016](0016-emit-release-state-render-it-never-deliver-it.md) — `status` reports, `check` decides, `release` acts
