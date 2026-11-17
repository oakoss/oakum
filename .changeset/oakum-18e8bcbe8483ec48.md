---
oakum: patch
---

The `.changeset/README.md` that `init` writes names the two markdown-lint settings oakum's files need (MD041 off for `.changeset/*.md`, MD024 `siblings_only` for changelogs), and `migrate` points there for them; it no longer claims `init` prints them. The same README says a clean `check` prints its `check:` and `coverage:` lines, not nothing. `upgrade` refreshes a README v0.10.0 through v0.12.0 wrote.
