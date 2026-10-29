---
oakum: major
---

### Changed

In a `CHANGELOG.md` that more than one managed package writes, such as a crate and an npm package in one directory, only a `## <name>@<version>` heading is a package's section, and `version` writes that heading there. A bare `## 0.3.0` no longer makes a hand-set sibling at 0.3.0 read as released by `version`, and each release body is its own entry rather than the first match. A bare heading for a package's version in a shared changelog, as earlier releases wrote, now stops `version`, commits-only planning and `release` (exit 1) with the heading to write, and `check` reports the version as set by hand. A template whose heading would read as any package's is refused there.

A version heading inside an HTML comment or a `<details>` block no longer counts as a section, and a bump-file note or template entry that leaves either open is refused like an open code fence. A changelog that leaves one open, which hides every heading after it, is named with its line by `check`, and `version`, commits-only planning and `release` refuse it (exit 1) until it is closed.
