---
oakum: major
---

### Changed

A package's `CHANGELOG.md` that is a symlink whose target does not exist no longer reads as absent. `check` reports it under "changelog(s) `oakum version` would refuse to append to" (exit 2), naming the link and the fix, and `migrate` lists it among the remaining steps; `version` already refused to write through it.
