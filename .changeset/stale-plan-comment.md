---
oakum: minor
---

The pull-request plan comment opens with the commit it describes, the pull request's head commit rather than the merge commit Actions checks out, and links the run that wrote it, so a plan left by an older run reads as older.

A `ci pr-status` run that cannot build the plan (a shallow clone, unreadable tags, a coverage look that could not run, a selection naming no package) still fails as before, and now marks oakum's comment stale instead of leaving it as current: a warning naming the commit and run with the reason, above the last plan oakum checked, collapsed. With `--emit-comment` it writes the reason to `oakum-pr-comment.failed` (artifact format 2), and `ci post-comment` composes the same banner and exits 0. The posting job and the artifact upload need `if: always()` for the banner to reach the pull request.

`ci pr-status --emit-comment` writes `oakum-pr-comment.format` beside the comment, recording the artifact's format and the oakum that wrote it, and `ci post-comment` refuses an artifact newer than it can read by naming both versions (exit 2, nothing touched) instead of misreading it. A directory without the record reads as format 1, so artifacts from earlier oakum still post.
