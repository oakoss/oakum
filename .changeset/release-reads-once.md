---
oakum: patch
---

### Changed

`release` reads the workspace, the install pin and the reachable tags once per run instead of once per step, and takes an existing tag's commit from that tag listing rather than asking git per tag, so it spawns fewer `cargo`, `pnpm` and `git` children.
