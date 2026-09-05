# Accept both change files and conventional commits, each fully disableable

- Status: accepted
- Date: 2026-08-18
- Deciders: Jace Babin

## Context and Problem Statement

Release intent can be captured two ways: a change file written deliberately per pull request, or a conventional commit message parsed after the fact. Tools generally pick one and treat the other as a lesser mode. Which does oakum read, and can a repository turn either off?

## Decision Drivers

- Contributors who will not adopt conventional commits are a real constraint, stated directly: "some people don't want to do conventional commits"
- A change file carries a consumer-facing summary; a commit subject usually does not
- One tool across every repository ([ADR-0012](0012-scope-v0-to-version-math-and-the-github-layer.md)) means the repositories will not agree on one convention

## Considered Options

- Change files only, as changesets and bumpy do
- Conventional commits only, as release-please and semantic-release do
- Both, with either disableable

## Decision Outcome

Chosen option: **both, and each fully disableable.**

Symmetry is the part worth writing down, because the obvious implementation is asymmetric and a surveyed tool already shipped that mistake. knope gates commit parsing behind `[changes] ignore_conventional_commits` and gates change files behind nothing — its changeset directory is a hardcoded constant, scanned whenever it exists (verified 2026-08-18). A repository that has settled on conventional commits cannot declare that; it can only decline to create the directory, and a stray change file is still consumed.

Either mechanism alone is a complete configuration. Turning both off is not — a repository with no way to express release intent has nothing to plan from, and `check` should say that rather than reporting a clean run.

### Consequences

- Good, because a repository adopts oakum without first winning an argument about commit conventions
- Good, because `generate` has an honest role: it derives change files from commits, so the two mechanisms converge on one artifact rather than running as parallel code paths
- Bad, because when change files are enabled, commit intent is invisible to `status` / `version` until `generate` (or `add`) materializes a bump file — composition is settled by [ADR-0029](0029-plan-from-one-intent-artifact.md)
- Neutral, because coverage follows the ADR-0029 table: change files enabled → pending bump files only (commits never satisfy `check`); change files disabled and commits enabled → commit-derived intent

## More Information

**Amended 2026-08-21:** composition when both mechanisms are configured is settled by [ADR-0029](0029-plan-from-one-intent-artifact.md). When change files are enabled, the plan reads only those files; commits feed `generate` only (and `generate` itself requires commits enabled too). When change files are disabled and commits are enabled, the plan reads commits. There is no highest-wins merge and no conflict diagnostic at plan time.

**Amended 2026-09-05 (`okm-0er`):** a flagless `oakum init` enables both mechanisms (`change-files = true`, `conventional-commits = true`), matching `OakumConfig::defaults()`. Single-mechanism configs remain fully supported via flags, `--interactive`, or a later config edit. This does not change the disable switches or the both-off refusal.

- [ADR-0029](0029-plan-from-one-intent-artifact.md) — single-artifact plan input
- [specs/bump-files.md](../specs/bump-files.md) — the change-file contract
- [specs/init.md](../specs/init.md) — flagless `init` enables both; either may be turned off afterward
- [bump-file tool interfaces](../research/bump-file-tool-interfaces.md) — `generate`'s flags and its conventional-commit mapping
- [intent-mechanism composition](../research/intent-mechanism-composition.md) — peer survey that informed ADR-0029
