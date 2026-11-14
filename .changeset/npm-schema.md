---
oakum: minor
---

### Added

The `@oakoss/oakum` npm package, and every release archive, now ships `schema.json`, the config schema the binary generates. An npm workspace that installs before it lints may point `_config.toml` at it with `#:schema ../node_modules/@oakoss/oakum/schema.json` and delete `.changeset/_schema.json`. `upgrade` then writes no `_schema.json` while the file is absent and the `#:schema` line names any schema other than that file (a URL included), and still regenerates one that exists. The committed `_schema.json` stays the default, because taplo fails to load a `node_modules` schema until the install has run.
