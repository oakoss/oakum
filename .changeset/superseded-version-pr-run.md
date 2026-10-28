---
oakum: major
---

`oakum ci version-pr` exits 0 and writes nothing when a newer push has moved the default branch past the run's checkout: GitHub's compare shows the branch tip descending from `HEAD`, and stdout names both commits. Back-to-back merges no longer leave a failed version-pr job behind the run that rebuilt the pull request. A tip that is behind or has diverged from `HEAD`, or a compare GitHub answers with 404, still exits 1, now naming which; a compare that could not be read exits 2.
