---
oakum: major
---

### Changed

A change inside a Cargo path crate the workspace excludes now reaches the packages that take it through `dependencies` or `build-dependencies`, so `check --strict` asks them for coverage where an edit that left `Cargo.lock` unchanged passed. `check` names the crate (`cli (cargo): changed (via helper)`), and `generate` reads it the same way. A crate taken only as a `dev-dependency` asks nothing.
