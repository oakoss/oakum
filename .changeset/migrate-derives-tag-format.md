---
oakum: minor
---

### Added

`migrate` derives `tag-format` from the tags a repository already carries, so a cutover from a tool that tags `name@version` no longer meets oakum's `name/v1.0.0` default at the first release. Derivation is structural rather than a list: a shape is written only when one template explains every tag, and only when oakum's own tag reader can read that shape back. Tags that disagree, or that no split explains, write nothing and are named among the remaining steps, because refusing beats cutting a tag the repository does not use; the shapes this repository could adopt are listed beside the ask, since which one is right depends on the tags it could not reconcile. That list drops any shape the workspace cannot use — above one tag-managed package the bare `v{{ version }}` is absent, because a release reads such a tag as leftover ambiguity, and the step that refuses a bare shape must not then offer it. A repository whose tags cannot be read is told so rather than treated as having none.

`migrate` also names the install pin among its remaining steps when the repository has none, quoting the version it just wrote to `tool-version` and the install command for the ecosystem it detected — the same one the workflow it prints below uses, rather than both ecosystems' commands regardless. Without a pin every later oakum command refuses, and a reader who installed globally would otherwise meet that refusal with the migration already applied.
