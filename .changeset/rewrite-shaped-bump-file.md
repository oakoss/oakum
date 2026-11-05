---
oakum: patch
---

### Fixed

A bump file named like the consume marker's staging copy, such as `.version.oakum-consume.1.2.0.oakum-write.4.md`, made an interrupted consume whose writes had landed read as a rollback, so `version` advised reverting writes that had landed correctly.
