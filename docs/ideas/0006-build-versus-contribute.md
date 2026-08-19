# What if the right move is contributing to bumpy rather than building oakum?

- Status: draft
- Date: 2026-08-18
- Author: Jace Babin
- Promoted to:

## The idea

Recovered from the design session of 2026-08-18, where it was raised and never resolved. It has since been reached a second time independently, from bumpy's published roadmap, which is reason enough to keep it written down.

bumpy's roadmap lists a standalone binary for use outside JS projects and better support for versioning non-JS packages. Those are oakum's distribution and polyglot arguments. If bumpy ships them, oakum is a competitor to a tool that does most of what it wants.

## Why it might matter

The case against building was put as: if bumpy ships those two items, the distribution and polyglot arguments evaporate.

The case for building: a roadmap is not a shipped feature and carries no date; the plugin-system item on it moves *away* from this design ([ADR-0013](../decisions/0013-no-plugin-runtime.md)); templating is not on it at all; and three bugs found in bumpy in a single day suggest the private-package path is thinly exercised, which is a shaky base to add polyglot support onto.

Neither argument settles it, which is why the recommendation was an experiment rather than a decision.

## Sketch

**Run the experiment before deciding.** There is a validated patch for the private-tag bug sitting in a clone, plus two more bugs found the same day — an unreachable `finalizeRelease` and a phantom npm target on private packages. Upstream all three.

That costs an afternoon already budgeted, and the response answers the question that actually matters: **is this maintainer responsive to outside contributions?**

If yes, contributing templating and a Cargo adapter to a tool whose roadmap already wants them is dramatically cheaper than building, and it means shaping the thing rather than competing with it. If the pull requests sit for two months the way knope's cascade PR has, that is the answer too, and the build proceeds with confidence instead of doubt.

The experiment was recommended to run *before* the implementation questions get built out, precisely so the answer arrives while it can still change the plan.

## Open questions

- ~~Whether the three patches were ever upstreamed, and what happened.~~ **Answered 2026-08-19: they were not.** `dmno-dev/bumpy` has zero pull requests from this author, so the experiment the sketch above recommends has not been run, and the question it was meant to answer is still open at no cost so far.
- **No outside human has ever opened a pull request there.** Across all 150 pull requests in any state — every one enumerated, not sampled — the authors are `theoephraim` (107), `bumpy-bot` (39), and `github-actions[bot]` (4). Stating it over every state rather than over merges matters: a closed-unmerged outside pull request would be the unresponsiveness signal this idea is hunting for, and there is none because there is nothing. Of the five currently open, four are the maintainer's and the fifth is the release bot's.
- **The issue tracker is the closest proxy, and it reads favorably.** Eight of nine issues are outside-authored, six are closed, and several closed the same day they opened — `#148` and `#96` within hours, `#123` same-day, `#117` in four days. That is engagement with outsiders, not merge behavior, which is exactly the gap the experiment would close. So the experiment is cheaper to justify than "no signal at all" would suggest: the maintainer answers strangers, and nobody has yet tested whether they merge from them. (GitHub search API, enumerated 2026-08-19.)
- **The plugin-system item has moved from roadmap to code.** [#153](https://github.com/dmno-dev/bumpy/pull/153), *"feat: publish-target plugin system (npm / jsr / pypi / vscode-marketplace / open-vsx)"*, has been open since 2026-07-31. The sketch above counts that item as moving away from this design ([ADR-0013](../decisions/0013-no-plugin-runtime.md)); it is now a live branch rather than a listed intention, which strengthens that half of the argument.
- Whether oakum's genuine differentiator survives either way. [ADR-0009](../decisions/0009-delivery-artifacts-always-cascade.md)'s delivery-artifact rule is not on bumpy's roadmap and no surveyed tool implements it — that, not polyglot support, may be the thing worth shipping.

## Related work

- [ADR-0012](../decisions/0012-scope-v0-to-version-math-and-the-github-layer.md) — the abort condition, which is the other half of this question: reproduce linesmith's history within two weekends or stop and upstream the cascade fix
- [Bump-file tool interfaces](../research/bump-file-tool-interfaces.md) — what bumpy actually ships today
