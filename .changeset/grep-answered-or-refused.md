---
oakum: patch
---

### Fixed

`oakum migrate` no longer reads a `git grep` that exited 0 without naming a file as a search that found nothing. Real git cannot answer that way — a match prints the path, and both no-match and an empty index exit 1 in silence (measured on git 2.55.0 and Apple Git 2.54.0) — so the shape only ever comes from a wrapper that failed to look, and the gate find it produced was a look nobody completed. Such a run now refuses as `unverified` and exits 2 where it printed that no file names the old bump-file directory and exited 0. No repository running real git changes behavior.
