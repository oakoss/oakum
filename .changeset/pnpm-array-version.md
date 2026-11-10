---
oakum: major
---

### Changed

When `package.json` gives pnpm its version in a `devEngines.packageManager` array, the workflow `init`, `migrate`, and `oakum workflow` print passes that version to `pnpm/action-setup` as written, a range such as `>=10` or `^11` included, quoted where YAML needs it, trimmed, and without a `+<integrity>` suffix on a single version, instead of asking pnpm for one; under pnpm 11 that question resolved the range and wrote `pnpm-lock.yaml`, a file none of the three owns. An empty or blank version there exits 2, naming the declaration to write, since the action reads it as none, and so does one holding characters outside printable ASCII, which a workflow cannot carry.
