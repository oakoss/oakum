---
oakum: minor
---

### Added

`migrate` from changesets or knope now lists a remaining step to check any commit or CI gate that reads `.changeset/*.md`. oakum writes its own bump files there (an unscoped changesets key such as `"core": patch` becomes `core: patch`), so a gate written for the old tool's files may reject them. oakum does not look for such a gate and says so; the exit code is unchanged.
