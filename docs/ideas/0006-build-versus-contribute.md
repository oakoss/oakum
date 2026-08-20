# Upstream experiment and the abort contingency (not "should we build?")

- Status: active
- Date: 2026-08-18
- Author: Jace Babin
- Promoted to:

## The idea

This note started as an open "build oakum vs contribute to bumpy" question. **Building is no longer open:** [ADR-0012](../decisions/0012-scope-v0-to-version-math-and-the-github-layer.md) and [ADR-0018](../decisions/0018-own-the-plan-engine.md) chose a owned plan engine and a v0 scope. What remains useful is the **contingency** those ADRs already encode, plus an experiment that was recommended and never run.

1. **Abort path.** If the planner cannot reproduce linesmith's release history and the eight silent misses (`okm-vio`), stop sinking effort and upstream the cascade fix to an existing tool rather than shipping an adequate-but-unjustified binary.
2. **Upstream experiment.** Whether bumpy's maintainer merges outside contributions is still unknown. That answer does not reopen "should oakum exist?"; it informs whether cascade work could also land upstream if the abort fires, and whether parallel contribution is cheap insurance.

## Why it might still matter

- The abort condition is the project's main protection against sunk cost. Keeping the contribute path legible makes that exit real rather than rhetorical.
- bumpy's roadmap still lists a standalone binary and non-JS packaging. If those ship, oakum's *distribution* arguments weaken; the differentiator that does not is [ADR-0009](../decisions/0009-delivery-artifacts-always-cascade.md)'s delivery-artifact cascade, which no surveyed tool implements.
- Three validated bumpy fixes from 2026-08-18 were never opened as pull requests. The cost of the experiment is still about an afternoon.

## Sketch

**If `okm-vio` fails:** prefer upstreaming the cascade rule (and the private-package fixes) over rewriting oakum around a weaker planner.

**Independently, still worth doing once:** open the three bumpy patches (private-tag skip, unreachable `finalizeRelease`, phantom npm target on private packages). Responsiveness to *merges* is the signal; issue-tracker speed is not a substitute (see open questions).

Do not treat a green bumpy merge as a reason to delete oakum while ADR-0009 remains unique.

## Open questions

- ~~Whether the three patches were ever upstreamed, and what happened.~~ **Answered 2026-08-19: they were not.** `dmno-dev/bumpy` has zero pull requests from this author, so the experiment has not been run.
- **No outside human has ever opened a pull request there.** Across all 150 pull requests in any state — every one enumerated, not sampled — the authors are `theoephraim` (107), `bumpy-bot` (39), and `github-actions[bot]` (4). A closed-unmerged outside PR would be the unresponsiveness signal; there is none because there is nothing. Of the five currently open, four are the maintainer's and the fifth is the release bot's.
- **The issue tracker is the closest proxy, and it reads favorably.** Eight of nine issues are outside-authored, six are closed, several same-day. That is engagement with strangers, not merge behavior. (GitHub search API, enumerated 2026-08-19.)
- **The plugin-system item has moved from roadmap to code.** [#153](https://github.com/dmno-dev/bumpy/pull/153), open since 2026-07-31, moves *away* from [ADR-0013](../decisions/0013-no-plugin-runtime.md).
- Whether, on abort, the right upstream target is bumpy, knope, or changesets — depends on who accepts a delivery-artifact cascade rule.

## Related work

- [ADR-0012](../decisions/0012-scope-v0-to-version-math-and-the-github-layer.md) — abort condition and v0 scope (build is decided)
- [ADR-0018](../decisions/0018-own-the-plan-engine.md) — own the plan engine
- [ADR-0009](../decisions/0009-delivery-artifacts-always-cascade.md) — differentiator that survives either path
- [Bump-file tool interfaces](../research/bump-file-tool-interfaces.md) — what bumpy ships today
