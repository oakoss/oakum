---
oakum: patch
---

In a pnpm workspace, a dependency whose range the same-named member's current version does not satisfy, such as `"is-number": "7.0.0"` beside a workspace member `is-number` at `1.0.0`, is no longer read as depending on that member. pnpm installs it from the registry, yet `version` rewrote it to the member's new version on a release that left the range. This covers plain ranges, `npm:` aliases and `catalog:` entries; a `workspace:` declaration is still always an edge.
