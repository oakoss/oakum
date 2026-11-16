---
oakum: minor
---

### Added

`dependency-updates = "release"` in `.changeset/_config.toml` releases each versioned package a dependency update reached since its last tag, with no bump file: a build at `patch`, a library at `patch` for compatible requirement moves and the next major otherwise, under a note naming what moved (`serde ^1.0.100 → ^1.0.200`). The default, `"ignore"`, releases nothing for them.

### Changed

`check` recognises a version commit by the version it moved and the changelog section it added, not by a bump file it deleted, so a release that consumed no bump file is not read as an uncovered change.
