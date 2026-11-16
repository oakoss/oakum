---
oakum: major
---

A change that only moves dependencies needs no intent, whatever its commit says. `check` reports a package every path of which is a `Cargo.lock` row or `pnpm-lock.yaml` entry for a dependency it takes at runtime, a `[workspace.dependencies]` or catalog entry it resolves differently, or an existing entry's requirement, features, `default-features` or source in its own manifest as `covered by a dependency update` (`"kind": "dependency-update"` in `check --json`), so a Renovate or Dependabot pull request committed as `fix(deps)` passes `check --strict` without a bump file. An edit confined to a package's own development dependencies asks nothing. A root package holding a lockfile or `pnpm-workspace.yaml` is reached by their entries rather than as files in its directory.
