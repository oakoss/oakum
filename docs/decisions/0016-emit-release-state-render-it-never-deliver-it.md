# Emit release state as data, render it as text, never deliver it

- Status: accepted
- Date: 2026-08-18
- Deciders: Jace Babin

## Context and Problem Statement

The delivery check in [downstream handoff](../research/downstream-handoff.md) already has to know which packages released at which versions, when, whether each tag exists, whether its run succeeded, and what is still pending. That is exactly the data a release summary needs. Does it stay internal, or does it get a shape other things can consume?

The question came from wanting a release summary without writing a bespoke script for it: *"It could be useful to offer some kind of check so if you want to build a release summary or something you don't have to build your own script."*

## Decision Drivers

- The data is computed either way; the only question is whether it escapes
- The template engine already exists for tags, titles, bodies, and pull-request comments ([ADR-0006](0006-no-command-execution-in-templates.md))
- Integrations are unbounded — there is always one more destination
- A reporting command that grows mutating flags becomes a second, subtly different code path for what `check` already does

## Considered Options

- Keep it internal; consumers query the GitHub API themselves
- `status --json`, emitting the computed state
- `status --json` plus a rendered form through the template engine

## Decision Outcome

Chosen option: **`status --json` plus a rendered form**, with a hard line about where it stops.

The JSON gives programmatic consumers — a dashboard, a notifier, an agent asking what shipped — a stable schema instead of a script re-deriving it from the API. bumpy has the same command, so it is not exotic. The rendered half costs nearly nothing, because a release summary is another template against the same context:

```bash
oakum status --template summary >> "$GITHUB_STEP_SUMMARY"
```

That composes with the step-summary pattern already used throughout this repository, whose `CI Summary` job writes exactly that shape by hand today.

**The line to hold: emit data, render text, never deliver.** The moment it grows a Slack webhook or a mail sender it is acquiring integrations. Delivery is a shell command away across the process boundary settled in [ADR-0013](0013-no-plugin-runtime.md), which is what that boundary is for.

**Tier by what the data requires, so it works in both environments.** Pending change files, the computed plan, and coverage gaps are all derived from the working tree: offline, no token, usable from a git hook or a local "what is pending?" Delivery verification is inherently remote and sits behind an explicit flag. The same split applies to `check` — preconditions are local, the retroactive delivery pass is opt-in.

**Bound the retroactive check.** "Did the last N releases deliver?" needs an N, or it walks the whole tag history on every run. The API cost is irrelevant at this volume; the latency is not, and a check that takes ten seconds is a check people stop running. Last three to five, configurable, off by default in `check`, with CI turning it on.

**Three verbs, three jobs.** `status` reports, `check` decides, `release` acts. Keeping them distinct is what stops `status` from slowly growing flags that mutate.

### Consequences

- Good, because the schema is a byproduct of data the tool already computes rather than a feature built for it
- Good, because the same context object feeds templates, `--json`, and the pull-request surfaces in [ADR-0015](0015-layer-the-pr-status-channels.md), so it is designed once
- Bad, because the context object becomes a public interface the moment anything parses it. It needs a version field, same as the process-boundary contract — and per [ADR-0002](0002-single-crate-until-io.md) that is one of the two triggers for splitting a schema crate out of this one
- Neutral, because nothing in v0 consumes it except the tool itself

## More Information

**Borrow output names where the concept genuinely lines up.** `changesets/action` exposes `published` and `publishedPackages`; release-please exposes `releases_created` and `paths_released`. Matching those for GitHub Actions outputs means someone migrating reads the workflow without a translation step, and it costs nothing.

**bumpy's `status` is the interface reference**, verified 2026-08-18: `--json`, `--packages` (names only, one per line, for scripting), `--filter <names>` with glob support, `--bump <types>`, `--verbose`, `--channel <name>`. Its changelog formatters receive a context of `release`, `bumpFiles`, `date`, and `target` — where `target` is `changelog` or `github-release`, letting one formatter drop the date from a release body that already displays one. That `target` discriminator is worth copying; it is the same problem [ADR-0015](0015-layer-the-pr-status-channels.md) has across comment and summary.

- [ADR-0006](0006-no-command-execution-in-templates.md) — the template engine this reuses
- [ADR-0013](0013-no-plugin-runtime.md) — the process boundary that keeps delivery out
- [idea 0005](../ideas/0005-structured-release-state.md) — the exploratory note this promotes
