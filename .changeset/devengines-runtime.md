---
oakum: patch
---

### Fixed

`check`'s lockfiles look skips a recorded `runtime:` entry that no dependency section declares when the manifest has an `engines.runtime` or `devEngines.runtime`, from which pnpm writes such entries. A root declaring node 24.21.0 under `devEngines.runtime` failed `check --strict` with `` `importers/./devDependencies/node` records `runtime:24.21.0`, which `package.json` does not declare ``, though pnpm's frozen install accepts that lockfile. The skipped entry is counted in the look's line, as an override's is. Every other `runtime:` entry is compared as before. The runtime `engines` or `devEngines` declares is not read, so its entry going stale is not reported.
