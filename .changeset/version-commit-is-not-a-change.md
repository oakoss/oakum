---
oakum: patch
---

`check` and `status` no longer count a version commit's own writes as an uncovered change. A range holding the commit that ran `version` reported its packages as changed with no covering intent, and the suggested fix would have bumped them a second time. A commit that deletes a bump file now has its manifest, changelog, lockfile and catalog edits left out of coverage in each package directory whose `CHANGELOG.md` it also wrote, whatever its subject says. Its other edits, a merge's, and a later commit's still count.
