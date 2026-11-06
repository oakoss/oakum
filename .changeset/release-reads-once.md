---
oakum: patch
---

### Changed

`release` reads the workspace, the install pin and the reachable tags once per run instead of once per step, takes an existing tag's commit from that tag listing rather than asking git per tag, and checks an ordinary tag name itself rather than running `git check-ref-format` for each, so it spawns fewer `cargo`, `pnpm` and `git` children.
