---
oakum: patch
---

### Fixed

`oakum upgrade` refreshes `.changeset/README.md` when it is the text an earlier oakum release wrote, so a corrected README reaches repositories that already had one, and `init` and `migrate` replace such a README where they kept it before. `check` notes one, naming `oakum upgrade`, without changing its exit code. A README of your own is left alone, and one oakum cannot read never stops `upgrade`.
