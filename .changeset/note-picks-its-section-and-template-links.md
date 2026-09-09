---
oakum: minor
---

A bump-file note whose first line is a Keep a Changelog heading (`### Added`, `### Changed`, `### Deprecated`, `### Removed`, `### Fixed`, `### Security`) lands under that section, with the line dropped; the level still picks the section otherwise, and `add --section` writes the line. The changelog template sees `changes` (per note: section, file, and the adding commit's sha, pull request number, and author) and `repo` (the GitHub slug), read from git only when the template mentions them.
