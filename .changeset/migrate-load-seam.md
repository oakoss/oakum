---
oakum: patch
---

A migration bump file identified by its path rather than an id is accepted, and `migrate` builds its after-plan through the same `load_migration_bump_files` seam the before-plan uses, refusing outright when any bump file is malformed.
