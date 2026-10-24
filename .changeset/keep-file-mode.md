---
oakum: patch
---

Files oakum rewrites keep their permissions. `version`, a rollback, `upgrade` and `migrate` replace a file by renaming a new copy over it, and that copy was created with default permissions, so an executable manifest came back `644` (a mode change in `git diff`). The new copy now takes the old file's permissions; on a filesystem that cannot keep them, the file is still written and a warning says so. A file's owner and inode still change, and a symlink is still replaced by a regular file.
