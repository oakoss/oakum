---
oakum: patch
---

When this binary is older than the repository's `tool-version`, the refusal no longer tells you to run `oakum upgrade`. Following that advice rewrote `tool-version` down to the older binary and exited 0, so a contributor with a stale local install downgraded the repository by doing what the error said. The refusal now names the pinned version to install and says `upgrade` would downgrade the repository. A binary newer than the pin still names `oakum upgrade`. Stderr only; the exit code is unchanged.
