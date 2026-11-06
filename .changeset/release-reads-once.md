---
oakum: patch
---

### Changed

`release` reads the workspace, the install pin and the reachable tags once per run instead of once per step, so it spawns fewer `cargo`, `pnpm` and `git` children.
