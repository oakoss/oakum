---
oakum: major
---

### Changed

`init`, `migrate`, and `oakum workflow` exit 2 when `package.json` gives pnpm a range or an empty version in a `devEngines.packageManager` array (`>=10`, `^11`, `""`), naming the exact declaration to write, where they asked pnpm for a version to pin instead. `pnpm/action-setup` does not read the array, and pnpm 11 answered by resolving the range and writing `pnpm-lock.yaml`, a file none of the three owns.
