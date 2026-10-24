---
oakum: patch
---

`version` and `ci version-pr` name the lockfile entries left for `pnpm install` once the files are written, so a run refused after planning no longer lists entries it never wrote. The version pull request also lists catalog entries that share a YAML anchor with a bumped pin.
