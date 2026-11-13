---
oakum: major
---

### Changed

`release` reads a tag that differs from the version only in build metadata as that version's tag when it looks for tags its resume never reads. With `v0.1.1+b` only on the remote, or on history unreachable from `HEAD`, releasing demo 0.1.1 created a second tag `v0.1.1` and a GitHub release beside it; it now refuses as `unverified`, naming `v0.1.1+b` as a tag for demo 0.1.1. A pending package is looked at too, so any unread tag naming the version it is about to tag refuses.
