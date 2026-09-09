---
oakum: patch
---

`release` fills the GitHub release body with the package's changelog section for that version, read at the tagged commit. When there is none, the body is the title and `release` says so on stderr; a changelog that cannot be read at that commit stops the run before any tag is written. `check`'s tag-drift line says it compared local tags and suggests `git fetch --tags`.
