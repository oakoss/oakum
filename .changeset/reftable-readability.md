---
oakum: patch
---

### Fixed

In a repository using git's reftable ref format, git loads the ref store whole: one table or `tables.list` it cannot read, in the shared store or a linked worktree's own, makes git list no tags at exit 0. Every command that reads tags now checks each table first, read by path as git reads it, and reports one that cannot be opened, is not a regular file, or does not start like a reftable table as unverified (exit 2), naming the file. Before, the run stayed unverified only because `git refs verify` crashed on that store, and the message told you to run that same crashing command.
