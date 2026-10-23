---
oakum: patch
---

Discovery no longer writes `pnpm-lock.yaml` into a lock-free pnpm workspace that declares its package manager. pnpm 11 wrote one despite `--config.lockfile=false` under a `devEngines.packageManager` it accepted and under a `packageManager` naming a newer pnpm, and pnpm 12 did when a `devEngines.packageManager` switched it to an 11. Discovery now has the pnpm on `PATH` answer, so a declaration pnpm would refuse, such as a `devEngines.packageManager` array or `onFail: "error"` naming another pnpm, no longer stops discovery. The pnpm version `init` and `migrate` read for the printed workflow still refuses an array naming another pnpm, and under pnpm 11 that check can still write a lockfile when an array names the running pnpm.
