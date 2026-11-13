---
oakum: major
---

### Changed

A `pnpm-lock.yaml` entry added, removed or resolved anew now reaches the npm packages that build it in, those with a `bin` or `resolves-dependencies-at = "build"` whose runtime dependencies lead to it, so `check --strict` asks them for coverage where it reported no version-managed package changed. A library is not asked, since it publishes no lockfile, and `version`'s own lockfile writes ask nothing. `check` names the direct dependency it came through (`changed (via pnpm-lock.yaml: is-odd)`), `generate` reads it the same way, and a lockfile that does not parse on either side is `unverified`. `check`, `add` and `generate` now read `resolves-dependencies-at` as `status` and `version` do.
