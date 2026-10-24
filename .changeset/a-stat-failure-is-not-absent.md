---
oakum: patch
---

When `version` fails creating a file and rolls back, a file it cannot check is named instead of being taken as never created. A stat that failed for any reason but the file being absent, before the create or after it, was read as nothing to clean up, so the failure report described a tree it had not checked. It now says it could not check whether the run created the file, and why, and to remove it only if the run did; the consume marker stays so the next `version` refuses rather than plan over it. A create refused because the file already exists names nothing, since it created nothing.
