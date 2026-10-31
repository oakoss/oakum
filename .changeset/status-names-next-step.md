---
oakum: minor
---

### Added

`oakum status` and the `ci pr-status` job summary say what to do next. An empty plan with no bump file pending says so and names `oakum add`, and `oakum generate` when conventional commits are on. An uncovered package is followed by the command that covers it, `oakum add --packages <package>:<level>`, plus `oakum generate --from <base>` where `check` would offer it (or a conventional commit naming it, in a commits-only repository), and the pull request comment carries the same line.
