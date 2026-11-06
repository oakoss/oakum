---
oakum: patch
---

### Fixed

`_schema.json` now says what `tag-format` defaults to when unset: `v{{ version }}` while one package is tag-managed, `{{ package }}/v{{ version }}` when several are. `oakum upgrade` rewrites the file.
