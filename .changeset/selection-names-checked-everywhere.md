---
oakum: major
---

`oakum add`, `generate` and `plan-intent` now refuse an `include` or `exclude` entry that names no package in the workspace, as `status`, `check`, `version` and `release` already did. They used to accept it, so `add` and `generate` wrote a bump file that every later step then rejected. The refusal, from every command, now names `.changeset/_config.toml` and lists the workspace packages to choose from (the first 20).

Commits now count only toward packages the config versions, both in `generate` and when commits are the plan's intent: a package `include`/`exclude` leaves out, or a private package without `private-packages.version = true`, is skipped. A commit touching only such a package no longer makes `version` refuse in commits-only mode, and `generate` writes no bump file for it, naming the packages it skipped and the setting to change; a commit touching several packages names only the versioned ones.
