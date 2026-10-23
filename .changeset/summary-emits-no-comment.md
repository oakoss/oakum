---
oakum: patch
---

`oakum ci pr-status --emit-comment <dir>` no longer writes the comment file when `pr-status = "summary"`. It wrote `oakum-pr-comment.md` anyway, so a trusted job that posts whatever it finds there posted a comment the config had turned off. It now writes the job summary as configured, removes a comment file left by an earlier run, says on stderr that `summary` turned the comment off, and exits 0.
