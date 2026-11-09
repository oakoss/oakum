---
oakum: minor
---

### Added

`check --explain` traces the plan the pending intent makes, after the looks: each planned bump, with the zero-major setting named when it changed the version, and each dependency onto a bumped package, whether it cascades and why, with the range its manifest declares when a range decided it (the reasons for a skip: the range still admits the new version, a development dependency, a dependent that is not version-managed, or one dropped with the package it was first reached through). When the plan cannot be built, it traces nothing and says so. It is text only and cannot be combined with `--json`.
