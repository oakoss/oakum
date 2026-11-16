---
oakum: minor
---

### Changed

`check` recognises a version commit by the version it moved and the changelog section it added, not by a bump file it deleted, so a release that consumed no bump file is not read as an uncovered change.
