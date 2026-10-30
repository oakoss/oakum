---
oakum: minor
---

### Changed

Help text states the `--from` default from one source: the merge-base with `origin/main`, else `main`, else `master`, with commit-derived intent starting at the highest reachable tag of each package oakum tags. `version` and `ci version-pr` say they read `--from` only for commit-derived intent. Options no longer cite ADR numbers, `check --strict` names both looks it gates (coverage and notes), and `version` describes itself in one line.
