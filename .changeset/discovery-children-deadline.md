---
oakum: patch
---

Discovery's `cargo metadata`, `pnpm root -w` and `pnpm list`, and the `pnpm --version` that `init` and `migrate` run, now run under the same wall-clock deadline as oakum's git children: five minutes, or `OAKUM_REMOTE_DEADLINE` seconds, per child. A child that never answered, for example `cargo metadata` blocked opening a member `Cargo.toml` that is a FIFO, used to hang every command that discovers packages with no output, the CI version job included. Oakum now kills it at the deadline and reports the look as unverified (exit 2), naming the tool and the variable. A malformed `OAKUM_REMOTE_DEADLINE`, which discovery used to ignore, now leaves discovery unverified too, as it already did for git. When one tool times out and the other fails outright, the failure decides (exit 1). If `cargo` or `pnpm` is a wrapper that starts the real tool as its own child, the real tool can outlive the kill.
