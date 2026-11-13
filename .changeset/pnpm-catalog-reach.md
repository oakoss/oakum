---
oakum: major
---

### Changed

A change to a pnpm catalog entry in `pnpm-workspace.yaml` now reaches the npm members whose `dependencies`, `optionalDependencies` or `peerDependencies` take it with `catalog:`, so `check --strict` asks those members for coverage where it reported no version-managed package changed. `check` names the entry (`changed (via pnpm-workspace.yaml: left-pad)`), and `generate` reads it the same way. The catalog pin `version` writes, an entry installing the same workspace package on both sides with its range retargeted at that package's version, asks nothing, and a `pnpm-workspace.yaml` that does not parse on either side is `unverified`.
