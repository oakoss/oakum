---
oakum: minor
---

### Changed

`init` ends with next steps: `oakum workflow` to save the workflow, `oakum add` for each change (conventional commits under `--change-files false`), `oakum status` to see the plan, and `.changeset/_config.toml` as where `versioning`, `change-files`, and `conventional-commits` are changed. An already-initialized rerun names `oakum workflow` too. `init` and `migrate` no longer print the `--interactive` hint after their writes, when the wizard could set nothing; `oakum init --help` names it.
