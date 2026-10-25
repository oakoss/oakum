---
oakum: major
---

A git listing that skipped part of what it was asked for is unverified (exit 2) instead of read as complete. Three cases that exited 0 before:

- A tag listing beside `warning: ignoring broken ref` (a tag ref holding garbage, or one git cannot read). git lists the other tags and exits 0, so the missing tag could be the last release.
- A worktree listing beside git's bare `<path>: <system error>` line, such as `beta/src/lib.rs: Permission denied`. `git status` drops a file it cannot `lstat` that way, so a changed package went unseen. Other unprefixed stderr, such as a clean filter's own output, still leaves the listing standing.
- A tag listing that `git refs verify` says could not open a directory under the repository's `refs/tags`, wherever its git directory lives: beside the worktree, inside a submodule's parent, or a separate git dir. git drops the tags in that directory without a word, and an empty listing read as never released. oakum now runs `git refs verify` after every tag listing. A git directory kept under a folder path holding `refs/tags` can't be told apart, so there an unopened directory in another namespace is unverified too. Its warnings about a ref's form, such as a missing newline or a symlink, change nothing. On a git older than 2.47, which has no `refs verify`, the listing stands as before.
