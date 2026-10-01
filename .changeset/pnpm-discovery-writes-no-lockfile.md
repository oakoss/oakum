---
oakum: patch
---

Under pnpm 12, discovery no longer writes `pnpm-lock.yaml` into a pnpm workspace, or rewrites the one it has. Every pnpm call oakum makes (`pnpm list`, `pnpm root -w`, and the `pnpm --version` that `oakum init` runs) created a lockfile, mode 0600, in a lock-free workspace that declares its package manager, so `oakum status`, and any other command that discovers packages, left an untracked file behind; under an array-form `devEngines.packageManager`, `init` also rewrote an existing lockfile. Oakum now passes `--config.lockfile=false` on each call. Measured on pnpm 8.15.9, 9.15.9, 10.34.5, 11.25.0, 12.5.1 and 12.6.0: each accepts the flag and prints the same output with it. pnpm 11 still writes one under a `devEngines.packageManager` declaration or a `packageManager` naming pnpm 12; the flag does not stop it there. Output and exit codes are unchanged.
