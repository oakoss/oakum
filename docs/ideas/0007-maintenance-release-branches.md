# What if oakum handled long-lived maintenance release branches?

- Status: draft
- Date: 2026-08-18
- Author: Jace Babin
- Promoted to:

## The idea

A release-train workflow, described from the user's workplace during the 2026-08-18 design session and deliberately left out of v0:

- Release branches are cut as `release/1.XX`, with the manifest at `1.XX.0`.
- Hotfix work merges to `main` first, then is cherry-picked onto the release branch.
- A hotfix bumps the patch number **on the same release branch** — `1.12.0` to `1.12.1`. No new branch is cut.
- Release branches rarely merge back. Work done on the branch stays there.
- After a release branch is cut, a pull request bumps `main` to the next minor.
- Tags and changelogs exist only on `release/**`. `main` carries neither.

Everything above is manual today.

## Why it might matter

**No surveyed tool handles it.** bumpy lists "maintenance/release branch workflows: hotfix support for older versions" under planned-and-unimplemented in its own comparison docs; changesets has an open discussion; knope has no branch concept beyond `baseBranch`; release-please gets partway with `--target-branch`. That is presumably why it is done by hand.

That makes it a candidate differentiator on the same footing as [ADR-0009](../decisions/0009-delivery-artifacts-always-cascade.md)'s delivery-artifact rule — a real workflow that the tools people already run do not serve.

## Sketch

**One decision is already taken and it is the one that matters.** [ADR-0014](../decisions/0014-tags-are-the-version-source-of-truth.md) reads tags reachable from `HEAD` rather than every tag in the repository. On `release/1.20` that means the tool sees the 1.20.x line and not `v1.21.0` from `main`. Getting that wrong would have precluded this model; getting it right cost one qualifier.

Two consequences fall out without any feature being built. The manual pull request bumping `main` to the next minor is unnecessary, because `main`'s next version is computed from its own reachable tags plus pending change files. And a workplace convention of tagging only release branches is already the default behavior rather than a setting, since a branch with no tags and no change files releases nothing.

**The part with no clean answer is changelog divergence.** Branches that never merge back mean `release/1.20`'s hotfixes exist only there and `main`'s `CHANGELOG.md` never learns about 1.20.3. Every tool that handles prerelease branches assumes eventual promotion by merge, and this workflow has none. The honest model is probably that a non-merging branch owns its own changelog and oakum does not try to reconcile them — but that is a position with real alternatives, not a settled answer.

## Open questions

- Whether it belongs in oakum at all. Every repository the user owns is trunk-based, so nothing exercises this, and it is a branching model rather than a feature — the shape of thing the v0 non-goals exist to exclude.
- Whether introducing a new release tool at work is even available as an option, since that is someone else's repository and someone else's decision.
- What actually goes wrong today when it is manual — a missed bump, a wrong tag, a changelog nobody updated. The answer decides whether the valuable half is the versioning or the release notes, and those want different features.
- Whether the cherry-pick step means a change file gets consumed twice: once on `main` and once on the release branch it is picked onto.

## Related work

- [ADR-0014](../decisions/0014-tags-are-the-version-source-of-truth.md) — the reachable-tags rule that keeps this reachable
- [ADR-0012](../decisions/0012-scope-v0-to-version-math-and-the-github-layer.md) — the v0 scope this sits outside, and the abort condition
