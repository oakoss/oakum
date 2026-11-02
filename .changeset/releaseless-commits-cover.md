---
oakum: major
---

With change files and conventional commits both on, a branch's commits that release nothing now cover the packages they reach: a package changed only by `refactor:`, `test:`, `chore:` and other non-releasing commits passes `check --strict` without a bump file, each package judged on its own, so one `fix` commit to another package asks only that package for one. Merge commits are skipped. A git revert of a commit in the same range cancels both, for coverage, `generate` and the commits-only plan alike, so a fix and its revert on one branch no longer release a patch; a revert of a commit outside the range still releases.
