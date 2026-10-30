---
oakum: major
---

### Changed

`check --strict` coverage counts only a branch's own intent. A bump file already pending on the base branch no longer covers a pull request that changes the package it names; the branch needs its own bump file (`oakum add`), or an edit to the pending one. Under conventional commits only, only the branch's commits cover it, not unreleased commits on the base branch. `status`, `status --json` (`uncovered`) and the `ci pr-status` comment report coverage the same way. A base sharing no history with `HEAD` leaves coverage unverified (exit 2).
