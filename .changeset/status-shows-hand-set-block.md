---
oakum: major
---

### Changed

`status`, `status --json` and the `ci pr-status` comment now show when the plan bumps a version set by hand, which `version` refuses until it is tagged. The render names it under "Blocked by a version set by hand" with the fix `version` prints, and the PR comment lists it before the packages that would release. `status --json` and the templates carry it as `hand_set`, one entry per package with `name`, `ecosystem`, `version` and `fix`. `status` still exits 0 on a block. When it cannot read the tags or a planned package's changelog it needs to tell, such as in a shallow clone, it now exits 2 (`unverified`) rather than show the plan as unblocked, naming any version set by hand it did find; a plan with no version that could have been set by hand reads no tags, as before.
