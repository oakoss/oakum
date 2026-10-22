---
oakum: patch
---

`version` and `ci version-pr` now rewrite the `pnpm-lock.yaml` entries a workspace range rewrite invalidates, so a frozen install (`pnpm install --frozen-lockfile`, or any install under `CI=true`) accepts the lockfile they leave. An entry they cannot rewrite, for example because the lockfile had already drifted from the manifest or the range resolves from the registry, is left as it was and named on stderr with `pnpm install` as the fix; `ci version-pr` also lists it in the version pull request body. A lockfile in a format oakum has not measured is left whole and named the same way.
