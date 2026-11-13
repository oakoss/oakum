---
oakum: patch
---

### Fixed

`oakum version` no longer writes a dependency range that excludes the version it just released. A pin on a workspace package written as `>0.9.0` was rewritten to `>1.1.0`, and `<2.0.0` to `<1.1.0`, so pnpm or cargo stopped linking the workspace package. A `>` bound now becomes `>=` the new version, and a `<` bound is kept while the new version meets it and becomes `<=` the new version when it does not, in `package.json`, catalogs and `Cargo.toml` alike.
