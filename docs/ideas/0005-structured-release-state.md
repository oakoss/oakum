# What if release state had a stable shape others could consume?

- Status: promoted
- Date: 2026-08-18
- Author: Jace Babin
- Promoted to: [ADR-0016](../decisions/0016-emit-release-state-render-it-never-deliver-it.md)

## The idea

**Settled — see [ADR-0016](../decisions/0016-emit-release-state-render-it-never-deliver-it.md).**

Recovered from the design session of 2026-08-18, where the idea was raised by the user and its network-tiered form confirmed. The recovery pass initially mis-filed it here as an unconfirmed recommendation.

The retroactive delivery check already has to know which packages released at what versions, when, whether each tag exists, whether its run succeeded, and what is still pending. That is exactly the data a summary needs. The only question is whether it stays internal or gets a stable shape.

Three options: keep it internal and let users query the GitHub API themselves; add `status --json` emitting the computed state; or that plus a rendered form through the template engine.

The third was chosen.

## Why it might matter

`status --json` gives programmatic consumers — a dashboard, a notifier, an agent asking "what shipped?" — a stable schema instead of a bespoke script re-deriving it from the API. bumpy has precedent with its own `status --json`, so it is not exotic.

The rendered half costs almost nothing, because the template engine already exists for tags, titles, bodies, and PR comments. A release summary is another template against the same context:

```bash
oakum status --template summary >> "$GITHUB_STEP_SUMMARY"
```

That composes with the step-summary pattern already used everywhere here — this repository's own `CI Summary` job writes exactly that shape by hand today.

## Sketch

**The line to hold: emit data, render text, never deliver.** The moment it grows a Slack webhook or an email sender it is acquiring integrations, and integrations are unbounded — there is always one more destination. Delivery is a shell command away through the process boundary settled in [ADR-0013](../decisions/0013-no-plugin-runtime.md), which is exactly what that boundary is for.

## Open questions

- Whether `status --json`'s schema is a public interface the moment anything consumes it — which is one of [ADR-0002](../decisions/0002-single-crate-until-io.md)'s two crate-split triggers, so answering yes has structural consequences.
- Whether the rendered form needs its own template context or reuses the release context unchanged.

## Related work

- [ADR-0013](../decisions/0013-no-plugin-runtime.md) — the process boundary that keeps delivery out
- [ADR-0006](../decisions/0006-no-command-execution-in-templates.md) — the template engine this would reuse
