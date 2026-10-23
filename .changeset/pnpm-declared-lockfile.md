---
oakum: patch
---

Discovery no longer writes `pnpm-lock.yaml` into a lock-free pnpm workspace that declares its package manager. pnpm 11 wrote one despite `--config.lockfile=false` under a `devEngines.packageManager` naming a version it accepted, and under a `packageManager` naming a newer pnpm; pnpm 12 did when a `devEngines.packageManager` switched it to an 11. Discovery now has the pnpm on `PATH` answer, so a declaration pnpm would refuse, such as a `devEngines.packageManager` array or `onFail: "error"` naming another pnpm, no longer stops discovery.

The workflow `init` and `migrate` print now takes the pnpm version from a `devEngines.packageManager` array that names an exact one, since `pnpm/action-setup` reads only the object form, instead of asking pnpm. An array naming a pnpm other than the one on `PATH` gets that pnpm as the workflow's `version` input rather than exiting `unverified`. An array whose version is not an exact one, such as a range, still asks pnpm, which under pnpm 11 can write a lockfile when the version matches it.
