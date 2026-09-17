---
oakum: patch
---

### Fixed

The version pull request is titled `chore(release): version packages`, the same sentence as the version commit. Where a repository takes its squash subject from the pull request title (`squash_merge_commit_title = PR_TITLE`), the old default — `Version Packages` — landed on the default branch instead, and a conventional-commit gate reading there refused the one commit oakum generates; the conventional message it wrote never appeared. GitHub's own default, `COMMIT_OR_PR_TITLE`, takes the subject from the single commit, so a repository left at that setting never saw this, and neither did one merging or rebasing, where the version commit lands carrying its own message. The two defaults sat four lines apart in the same file and disagreed either way. Set `title` in `.changeset/_config.toml` to keep the old string; its schema description now says what it falls back to and that a configured `commit-message` does not move it.
