---
oakum: major
---

### Changed

A manifest version set by hand above its highest tag, with no changelog section for that version, is now treated like an untagged hand-set version: `version` and `ci version-pr` refuse to bump past it (exit 1), and `check` names it as set by hand with the tag to cut, instead of stacking the next release on it. A version above its tag that has its section, as `version` writes, is still stacked on. A `0.0.0` or `0.1.0` manifest is checked against tags only when its changelog has a section for an older version, so a new package versions as before, while one whose changelog has one now needs full history: in a shallow clone `version` exits 2 rather than bump unchecked.
