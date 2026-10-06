---
oakum: patch
---

A git call that never answers now leaves the run unverified (exit 2): one killed at `OAKUM_REMOTE_DEADLINE`, or one whose output stalled or could not be read. The exit used to follow the kind of call, so a hung `git log` made `check --strict` exit 1, a hung `merge-base` made `version` and `status` exit 1, and a hung config probe, the first git call a command makes, made `version` exit 1, each reporting a failure where nothing was settled. `ci version-pr` keeps exit 2 when it adds context to a git error, and the `release` changelog read keeps a call that never answered unverified while still refusing one that answered with a failure.

`release` re-checks a `git tag` or `git push` that never answered. When the tag is found locally, or the push on the remote, it stops with exit 1 and names the stage (`tagged` or `pushed`); when the tag is not there, the stage is the one before. Only when the re-check fails too is the stop unverified (exit 2). A `git tag` that never answered no longer reports `completed: none` over a tag it wrote. The detail under an unverified stop says `unverified` once, not twice.

A `git` that cannot be started, or that answered with a failure, keeps its exit.
