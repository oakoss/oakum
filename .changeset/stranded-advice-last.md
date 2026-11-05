---
oakum: patch
---

### Fixed

When `version` rolls back and cannot move consumed bump files back, the advice to move them back names every one, where it stopped at five and said the rest were named in a list that also stopped at five. It now prints last, after any directories the rollback could not check, so it stays on screen.
