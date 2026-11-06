---
oakum: patch
---

### Fixed

A Cargo member that declares a path dependency under the deprecated `[dev_dependencies]` or `[build_dependencies]` spelling, which Cargo still reads, failed workspace discovery with "missing from manifest", and `version` could not rewrite it. Both spellings are now read.
