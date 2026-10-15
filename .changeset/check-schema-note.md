---
oakum: minor
---

`check` now reports `.changeset/_schema.json` when its bytes differ from what the running oakum generates, or when it cannot be read, whatever `_config.toml`'s `#:schema` line says; a missing one is reported only when that line names it. The stale and missing notes name `oakum upgrade`, after settling the install pins when they disagree, cannot be read, or are not exact versions. When the install pins agree on a version other than the running binary's, `check` does not compare and says so, naming both versions. Each note is a stderr line and, under `--json`, a `schema` look with outcome `reported`; a current schema, or an absent one the config does not name, reads `ok` and adds nothing. The exit code does not change in any case, `--strict` included: the binary validates the config on every run, so a stale schema affects editor hints only. The `check` scope line now lists `schema` among the looks.
