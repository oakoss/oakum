---
oakum: minor
---

### Added

`oakum ci post-comment DIR` posts the comment `ci pr-status --emit-comment DIR` wrote, from a job that holds the write token and has no checkout. It updates oakum's plan comment, or removes it when the emitting run had none, under the same ownership rule `pr-status` uses. The pull request comes from that job's own event (`pull_request`, or a same-repository `workflow_run`), never from the artifact. The body must start with oakum's marker, end with the closing line `--emit-comment` writes, and fit GitHub's 65,536-character limit; one that does not start with the marker or is over the limit is refused (exit 1).

`--emit-comment` now always leaves exactly one file, written whole: `oakum-pr-comment.md`, or `oakum-pr-comment.none` when the run has no comment. A run that stops first leaves neither. `post-comment` refuses as unverified (exit 2) when the artifact cannot be trusted whole (neither file or both, a body cut short, an entry that is not a regular file) or the token cannot write, so a lost artifact never reads as nothing to post and an earlier run's comment is never reposted.
