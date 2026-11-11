---
oakum: patch
---

### Fixed

A version-managed package that two dependencies cascade into now releases when one of them is excluded from versioning and the other is not. `status`, `version` and `check --explain` dropped it whenever the walk reached it first through the excluded one, since the plan was filtered by the single dependency each package was attributed to. Packages that are not version-managed are now left out while the plan is composed, so nothing cascades through them, and `check --explain` no longer prints `skip (dropped: first reached through a package the plan dropped)`.
