# What if the agent skill taught orchestration only?

- Status: draft
- Date: 2026-08-18
- Author: Jace Babin
- Promoted to:

## The idea

Agents will drive oakum, so it may be worth shipping a skill that teaches them to. The shape that seems right is **thin**: run `generate`, review what it produced, fix the parts that need judgment, verify with `check --explain`. Orchestration only.

The anti-pattern to avoid is bumpy's skill, which teaches the model to inspect diffs and identify which packages changed — work `generate` already does deterministically. A skill that re-derives what the tool computes is slower, non-reproducible, and wrong more often than the tool is.

## Why it might matter

- Bump files are written at the moment a change lands, which is increasingly by an agent
- It is the difference between an agent using the planner and an agent reimplementing it badly in prose

## Sketch

The prerequisite is that `--explain` output and error text **are** the agent interface. If an agent has to guess why a package was not bumped, the skill will grow a heuristic to guess it, and the heuristic will be wrong. Getting the explain output right removes the reason for the skill to be thick.

Distribution: the oakoss marketplace, already owned, plus a repository `SKILL.md` for `npx skills add` and `gh skill install`. cargo-dist's `include` may also place it in the npm package — auto-includes are verified to reach npm; `include` is inferred and would take one config line to confirm.

## Open questions

- Whether this is needed at all once `--explain` is good. A tool whose output explains itself may not need a skill teaching agents to read it.
- Whether a skill that ships inside the npm package creates a version-skew problem: the skill describes flags that the installed binary may not have.

## Related work

- [ADR-0013](../decisions/0013-no-plugin-runtime.md) — the JSON-on-stdout interface a skill would build on
- bumpy's agent skill, as the example of what not to do
