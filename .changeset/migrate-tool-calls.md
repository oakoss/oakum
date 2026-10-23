---
oakum: patch
---

`oakum migrate` runs bumpy, changesets and knope under `OAKUM_REMOTE_DEADLINE` like every other child: one whose answer has not arrived by then is named, and `migrate` exits `2` (unverified) instead of hanging.

`migrate` also finds those tools on `PATH` the way discovery finds pnpm, including a Git Bash `PATH` on Windows (`/c/Users/...`, or `:`-separated entries), which its lookup used to miss.

Both lookups now share one reading of `PATH`. On Windows, a quoted entry such as `"C:\Program Files\knope"` and a verbatim one such as `\\?\C:\tools` are each one directory, and only a name with a `PATHEXT` extension is taken, never an extensionless file. Elsewhere, `PATH` is read as the shell reads it, so `/a/bin` is a directory rather than drive `A:` and a bare `pnpm` wins over a `pnpm.EXE` beside it. On every system an empty `PATH` entry is skipped rather than read as the current directory, so it no longer runs a `pnpm`, `bumpy`, `changeset` or `knope` file from the repository being inspected.
