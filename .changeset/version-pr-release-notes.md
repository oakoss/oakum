---
oakum: minor
---

### Added

The version pull request body from `oakum ci version-pr` now quotes each package's changelog entry under **Release notes**, one collapsible block per package, so a reader on the Conversation tab or in email sees what ships, not only what bumps. The entry is read back from the changelog the branch writes. An entry that would take the body past GitHub's 65,536-character limit, or that holds raw HTML which could break the page, is named with its `CHANGELOG.md` path instead of being cut, and a body still over the limit stops the run before anything is pushed.
