---
oakum: minor
---

### Added

A migration guide, at [docs/guide/migrating.md](https://github.com/oakoss/oakum/blob/main/docs/guide/migrating.md). It covers which source settings carry and which do not — and the three different reasons a setting does not carry, since "no counterpart, nothing lost" and "no counterpart, but your release commits change author" want different responses from a reader. It puts the remaining steps in dependency order rather than in the order they print, because pinning the binary before pasting the workflow is the difference between a clean cutover and meeting a refusal with the migration already applied.

It also names the one part of a cutover that cannot be ported: a pipeline built on a plan/act split, where one job computes a mode and downstream jobs run guarded by `--expect-mode`, has no equivalent here. oakum has no plan step — `ci version-pr` and `release` each derive their own precondition from repository state when they run — so there is no window between deciding and acting to guard. That is a different safety argument rather than a weaker one, but it means the workflow is rewritten, not translated.

The guide leads with the acceptance test, which is **`oakum status` names every package you expect to release** — not "`check --strict` exits 0". One real cutover was given the second and satisfied it immediately with a config that managed nothing. oakum has since closed the specific hole that cutover fell into: a config whose only packages are private now exits 2 and names the fix. One shape still passes deliberately — an `include`/`exclude` that selects nothing exits 0, because emptying a selection is a decision the config states and a gate must not refuse one. The argument is narrower for that, and it still holds: a plan is a positive statement about what will happen, a gate exiting 0 is the absence of a complaint, and reading the plan needs no guard to have been written in advance for the particular way your config is wrong.

`oakum migrate` now names what the repository can retire, instead of leaving it to be found in the schema. `extra-files` declares a JSON file that carries a version — a plugin manifest, a marketplace entry — and `oakum version` writes it in the same pass as the manifest and under the same rollback, which retires a sync script, its drift job and its tests. v1 writes JSON only, so a chart or a README badge is refused at config parse rather than written. The reporter who found this called it the strongest argument for the migration, and found it by noticing a phrase in `oakum version --help` and then reading `_schema.json`.

### Changed

The GitHub Actions guide says why the printed workflow carries no `concurrency:` block, and does not tell you to omit one. Whether a group is load-bearing depends on whether the pipeline carries a decision between jobs: one that does needs protecting from a cancellation window, and one that re-derives its preconditions per command does not. This repository's own workflow carries a group and the printed one does not, and both are correct for that reason. The earlier guidance would have generalised another tool's failure without checking whether the mechanism carries over.
