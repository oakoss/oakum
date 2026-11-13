---
oakum: major
---

### Changed

`migrate`'s look for hooks that gate on the old bump-file directory now reads receive-pack's hooks where git runs them. Under a relative `core.hooksPath`, git resolves the path against the git directory for the hooks a push into the repository runs (`pre-receive`, `update`, `proc-receive`, `post-receive`, `post-update`, `push-to-checkout`, and `reference-transaction` during a push), and against the worktree root for every other hook. `migrate` read only the worktree root's, so a `.git/<path>/pre-receive` naming `.bumpy` went unreported and `migrate` exited 0. It now names such a hook, reading from a linked worktree both its own git directory and the main worktree's, each under that worktree's own `core.hooksPath` when `extensions.worktreeConfig` sets one, says `no hook in` each directory when none holds one, and exits 2 (unverified) when one of receive-pack's hooks or its directory cannot be read.
