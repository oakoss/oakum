---
oakum: major
---

`check` and `release` exit `unverified` when `.changeset/_config.toml` is absent, naming `oakum init` and `oakum migrate`; `status` keeps defaults and says so. A malformed bump file fails `check`, `status`, `version`, and every other reader by name with its parse error instead of being skipped. `migrate` without a terminal requires `--yes` and otherwise prints the plan and refuses. The workflow printed by `init` and `migrate` runs `oakum check --strict`.
