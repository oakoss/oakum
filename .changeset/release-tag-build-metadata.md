---
oakum: patch
---

### Fixed

`release` reads a tag that differs from the version only in build metadata as that version's tag, as `check` does: a `tag-format` that adds build metadata (`v{{ version }}+ci`) releases where `release` refused it as rendering a different version.
