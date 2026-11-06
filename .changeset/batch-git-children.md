---
oakum: patch
---

### Changed

Reading intent from commits, oakum lists the paths of every commit it attributes by path with one `git` child, instead of two per commit, and `check`'s coverage look reads every bump file `HEAD` carries with one `git` child instead of one per file.
