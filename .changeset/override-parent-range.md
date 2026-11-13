---
oakum: major
---

### Changed

`check`'s lockfiles look reads the parent's version range in a pnpm `-` override such as `'@x/b@2>@x/a': '-'`. pnpm removes the dependency only under a parent whose version satisfies that range, so with `@x/b` at `1.0.0` the override does nothing; `check` still counted it as covering a dependency the lockfile lacks, and passed a lockfile a frozen install refuses. It now reports that drift. Measured on pnpm 12.16.1: a range, even `*`, needs the parent to have a version that satisfies it, so a parent without a string `version` or a range pnpm cannot parse never matches; a parent named without a range matches any version. A `package.json` whose `name` is not a string, or appears twice, is now read as pnpm reads it (no name, or the last one) rather than refused, and one the look cannot read names the parse error in place of "whose dependency ranges are strings".
