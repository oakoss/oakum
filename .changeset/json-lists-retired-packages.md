---
oakum: minor
---

### Added

`status --json` and `check --json` list the packages the workspace no longer has, whose tags stay history: each as `{ "name", "ecosystem", "version", "tag" }` for its newest release. `status --json` carries them as `retired`, with `retired_look` saying whether the tags were read (`ran`, or `failed` when they could not be, or a rename stopped the read); `check --json` carries them on the `tags` look, absent when that look could not answer. Text output does not mention them.
