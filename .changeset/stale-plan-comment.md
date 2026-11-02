---
oakum: minor
---

The pull-request plan comment opens with the commit it describes, the pull request's head commit rather than the merge commit Actions checks out, and links the run that wrote it, so a plan left by an older run reads as older.

`ci pr-status --emit-comment` writes `oakum-pr-comment.format` beside the comment, recording the artifact's format and the oakum that wrote it, and `ci post-comment` refuses an artifact newer than it can read by naming both versions (exit 2, nothing touched) instead of misreading it. A directory without the record reads as format 1, so artifacts from earlier oakum still post.
